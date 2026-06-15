use crate::core::{Agent, AgentKind, DeliveryState, Route};
use crate::delivery::{DeliveryAction, DeliveryDecision};
use crate::delivery_transaction::{DirectDeliveryError, execute_direct_delivery};
use crate::local_status::{LocalStatusResponse, is_loopback_bind};
use crate::logging::StructuredLogger;
use crate::peers::{
    SshInventoryFetcher, SshPeerAgentReadFetcher, SshPeerAgentResumeFetcher, SshPeerMessageSender,
};
use crate::providers::DefaultProviderRouter;
use crate::services;
use crate::state::{AppState, StateStore};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Debug, serde::Serialize)]
struct ApiRequestStartedEventFields {
    request_id: String,
    method: String,
    path: String,
    route_template: String,
    route_family: String,
    query_keys: Vec<String>,
    content_length: usize,
    peer_addr_loopback: bool,
}

#[derive(Debug, serde::Serialize)]
struct ApiRequestFinishedEventFields {
    request_id: String,
    method: String,
    path: String,
    route_template: String,
    route_family: String,
    query_keys: Vec<String>,
    content_length: usize,
    peer_addr_loopback: bool,
    status_code: u16,
    ok: bool,
    duration_ms: u128,
    response_content_type: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedHttpRequest {
    method: String,
    path: String,
    query: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
    body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentThreadQuery {
    options: services::AgentReadOptions,
}

#[derive(Debug, serde::Deserialize)]
struct PostMessageRequest {
    target_agent: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    source_agent: Option<String>,
    #[serde(default)]
    source_node: Option<String>,
    #[serde(default)]
    correlation_id: Option<String>,
    #[serde(default)]
    message_type: Option<String>,
    #[serde(default)]
    strict: Option<bool>,
    #[serde(default)]
    expected_delivery: Option<String>,
    #[serde(default)]
    delivery_action: Option<String>,
    #[serde(default)]
    receipt_nonce: Option<String>,
    #[serde(default)]
    codex_program: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct PostAgentRequest {
    name: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    thread_source: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default, alias = "threadId")]
    thread_id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    model_provider: Option<String>,
    #[serde(default)]
    effort: Option<String>,
    #[serde(default)]
    speed: Option<String>,
    #[serde(default)]
    service_tier: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct PostAgentModelRequest {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    model_provider: Option<String>,
    #[serde(default)]
    effort: Option<String>,
    #[serde(default)]
    speed: Option<String>,
    #[serde(default)]
    service_tier: Option<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct PostAgentResumeRequest {
    #[serde(default)]
    codex_stdio: bool,
    #[serde(default)]
    codex_program: Option<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct PostDiscoveryLocalRunRequest {
    #[serde(default, alias = "codexHome")]
    codex_home: Option<String>,
    #[serde(default)]
    promote_approved: bool,
}

#[derive(Debug, serde::Deserialize)]
struct PostPeerRequest {
    name: String,
    #[serde(default)]
    ssh: Option<String>,
    #[serde(default)]
    ssh_target: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    key_path: Option<String>,
    #[serde(default)]
    remote_root: Option<String>,
}

pub fn handle_local_api_http_request(
    store: &StateStore,
    logger: &StructuredLogger,
    state: &mut AppState,
    runtime: &mut services::DaemonRuntime,
    peer_addr: &str,
    raw_request: &str,
) -> LocalStatusResponse {
    if !is_loopback_bind(peer_addr) {
        return LocalStatusResponse::forbidden_text(
            "local API endpoints are loopback-only; remote clients are forbidden",
        );
    }

    let request = match parse_http_request(raw_request) {
        Ok(request) => request,
        Err(message) => return LocalStatusResponse::bad_request_text(message),
    };
    if !method_allowed(&request) {
        return LocalStatusResponse::method_not_allowed_text(
            "local API allows GET plus authenticated POST routes for agents, discovery, messages, model settings, and peers",
        );
    }
    if !is_authorized(store, &request) {
        return LocalStatusResponse::unauthorized_text("missing or invalid local API bearer token");
    }

    let request_id = Uuid::new_v4().to_string();
    let started_at = Instant::now();
    if let Err(error) = log_api_request_started(logger, &request_id, &request) {
        return LocalStatusResponse::internal_server_error_text(format!(
            "failed to log local API request start: {error}"
        ));
    }
    let response = handle_authorized_local_api_request(store, logger, state, runtime, &request);
    if let Err(error) =
        log_api_request_finished(logger, &request_id, &request, &response, started_at)
    {
        return LocalStatusResponse::internal_server_error_text(format!(
            "failed to log local API request finish: {error}"
        ));
    }
    response
}

pub fn handle_local_api_http_request_with_shared_state(
    store: &StateStore,
    logger: &StructuredLogger,
    state: &Arc<Mutex<AppState>>,
    runtime: &Arc<Mutex<services::DaemonRuntime>>,
    peer_addr: &str,
    raw_request: &str,
) -> LocalStatusResponse {
    if !is_loopback_bind(peer_addr) {
        return LocalStatusResponse::forbidden_text(
            "local API endpoints are loopback-only; remote clients are forbidden",
        );
    }

    let request = match parse_http_request(raw_request) {
        Ok(request) => request,
        Err(message) => return LocalStatusResponse::bad_request_text(message),
    };
    if !method_allowed(&request) {
        return LocalStatusResponse::method_not_allowed_text(
            "local API allows GET plus authenticated POST routes for agents, discovery, messages, model settings, and peers",
        );
    }
    if !is_authorized(store, &request) {
        return LocalStatusResponse::unauthorized_text("missing or invalid local API bearer token");
    }

    let request_id = Uuid::new_v4().to_string();
    let started_at = Instant::now();
    if let Err(error) = log_api_request_started(logger, &request_id, &request) {
        return LocalStatusResponse::internal_server_error_text(format!(
            "failed to log local API request start: {error}"
        ));
    }
    let response = if request.method == "GET" && request.path == "/v1/inbox" {
        let state_snapshot = match state.lock() {
            Ok(state) => state.clone(),
            Err(_) => {
                return LocalStatusResponse::internal_server_error_text(
                    "daemon state lock was poisoned before inbox read".to_string(),
                );
            }
        };
        handle_get_inbox_snapshot(store, logger, &state_snapshot, &request)
    } else {
        let mut state = match state.lock() {
            Ok(state) => state,
            Err(_) => {
                return LocalStatusResponse::internal_server_error_text(
                    "daemon state lock was poisoned before local API request".to_string(),
                );
            }
        };
        let mut runtime = match runtime.lock() {
            Ok(runtime) => runtime,
            Err(_) => {
                return LocalStatusResponse::internal_server_error_text(
                    "daemon runtime lock was poisoned before local API request".to_string(),
                );
            }
        };
        handle_authorized_local_api_request(store, logger, &mut state, &mut runtime, &request)
    };
    if let Err(error) =
        log_api_request_finished(logger, &request_id, &request, &response, started_at)
    {
        return LocalStatusResponse::internal_server_error_text(format!(
            "failed to log local API request finish: {error}"
        ));
    }
    response
}

fn handle_authorized_local_api_request(
    store: &StateStore,
    logger: &StructuredLogger,
    state: &mut AppState,
    runtime: &mut services::DaemonRuntime,
    request: &ParsedHttpRequest,
) -> LocalStatusResponse {
    if request.method == "POST" && request.path == "/v1/agents" {
        return handle_post_agent(store, logger, state, request);
    }
    if request.method == "POST" {
        if let Some(name) = agent_model_route_name(&request.path) {
            return handle_post_agent_model(store, logger, state, name, request);
        }
    }
    if request.method == "POST" {
        if let Some(name) = agent_resume_route_name(&request.path) {
            return handle_post_agent_resume(store, logger, state, runtime, name, request);
        }
    }
    if request.method == "POST" && request.path == "/v1/messages" {
        return handle_post_message(store, logger, state, runtime, request);
    }
    if request.method == "POST" && request.path == "/v1/discovery/local:run" {
        return handle_post_discovery_local_run(store, logger, state, request);
    }
    if request.method == "POST" && request.path == "/shutdown" {
        return handle_post_shutdown(store, logger, state);
    }
    if request.method == "POST" && request.path == "/v1/peers" {
        return handle_post_peer(store, logger, state, request);
    }
    if request.method == "POST" && request.path == "/v1/peers:sync" {
        return handle_post_peers_sync(store, logger, state, request);
    }
    if request.method == "POST" {
        if let Some(name) = peer_sync_route_name(&request.path) {
            return handle_post_peer_sync(store, logger, state, name);
        }
    }

    match request.path.as_str() {
        "/v1/agents" => json_response(&services::list_agents(state)),
        path if agent_thread_route_name(path).is_some() => {
            let Some(name) = agent_thread_route_name(path) else {
                return LocalStatusResponse::bad_request_text(
                    "agent thread route could not be parsed after route match",
                );
            };
            return handle_get_agent_thread(logger, state, name, request);
        }
        path if path.starts_with("/v1/agents/") => {
            let name = path.trim_start_matches("/v1/agents/");
            if name.is_empty() || name.contains('/') {
                return LocalStatusResponse::not_found_text("unknown local API path");
            }
            return handle_get_agent(logger, state, name);
        }
        "/v1/inbox" => handle_get_inbox(store, logger, state, request),
        "/v1/logs" => handle_get_logs(store, request),
        "/v1/peers" => json_response(&services::list_peers(state)),
        "/v1/inventory" => {
            let inventory = services::export_inventory(state);
            json_response(&inventory)
        }
        _ => LocalStatusResponse::not_found_text("unknown local API path"),
    }
}

fn log_api_request_started(
    logger: &StructuredLogger,
    request_id: &str,
    request: &ParsedHttpRequest,
) -> Result<(), crate::errors::CamError> {
    logger.event(
        "api.request.started",
        format!("local API {} {} started", request.method, request.path),
        serde_json::to_value(ApiRequestStartedEventFields {
            request_id: request_id.to_string(),
            method: request.method.clone(),
            path: request.path.clone(),
            route_template: api_route_template(request).to_string(),
            route_family: api_route_family(request).to_string(),
            query_keys: request.query.keys().cloned().collect(),
            content_length: request.body.len(),
            peer_addr_loopback: true,
        })?,
    )
}

fn log_api_request_finished(
    logger: &StructuredLogger,
    request_id: &str,
    request: &ParsedHttpRequest,
    response: &LocalStatusResponse,
    started_at: Instant,
) -> Result<(), crate::errors::CamError> {
    logger.event(
        "api.request.finished",
        format!(
            "local API {} {} finished with HTTP {}",
            request.method, request.path, response.status_code
        ),
        serde_json::to_value(ApiRequestFinishedEventFields {
            request_id: request_id.to_string(),
            method: request.method.clone(),
            path: request.path.clone(),
            route_template: api_route_template(request).to_string(),
            route_family: api_route_family(request).to_string(),
            query_keys: request.query.keys().cloned().collect(),
            content_length: request.body.len(),
            peer_addr_loopback: true,
            status_code: response.status_code,
            ok: response.status_code < 400,
            duration_ms: started_at.elapsed().as_millis(),
            response_content_type: response.content_type,
        })?,
    )
}

fn api_route_template(request: &ParsedHttpRequest) -> &'static str {
    match request.path.as_str() {
        "/v1/agents" => "/v1/agents",
        "/v1/messages" => "/v1/messages",
        "/v1/inbox" => "/v1/inbox",
        "/v1/logs" => "/v1/logs",
        "/v1/discovery/local:run" => "/v1/discovery/local:run",
        "/v1/peers" => "/v1/peers",
        "/v1/peers:sync" => "/v1/peers:sync",
        "/v1/inventory" => "/v1/inventory",
        "/shutdown" => "/shutdown",
        path if agent_thread_route_name(path).is_some() => "/v1/agents/{name}/thread",
        path if agent_resume_route_name(path).is_some() => "/v1/agents/{name}/resume",
        path if agent_model_route_name(path).is_some() => "/v1/agents/{name}/model",
        path if peer_sync_route_name(path).is_some() => "/v1/peers/{name}/sync",
        path if path.starts_with("/v1/agents/") => "/v1/agents/{name}",
        _ => "unknown",
    }
}

fn api_route_family(request: &ParsedHttpRequest) -> &'static str {
    match request.path.as_str() {
        "/v1/agents" => "agents",
        "/v1/messages" => "messages",
        "/v1/inbox" => "inbox",
        "/v1/logs" => "logs",
        "/v1/discovery/local:run" => "discovery",
        "/v1/peers" | "/v1/peers:sync" => "peers",
        "/v1/inventory" => "inventory",
        "/shutdown" => "shutdown",
        path if path.starts_with("/v1/agents/") => "agents",
        path if path.starts_with("/v1/peers/") => "peers",
        _ => "unknown",
    }
}

fn method_allowed(request: &ParsedHttpRequest) -> bool {
    request.method == "GET"
        || (request.method == "POST"
            && (request.path == "/v1/agents"
                || request.path == "/v1/discovery/local:run"
                || request.path == "/v1/messages"
                || request.path == "/v1/peers"
                || request.path == "/v1/peers:sync"
                || request.path == "/shutdown"
                || peer_sync_route_name(&request.path).is_some()
                || agent_resume_route_name(&request.path).is_some()
                || agent_model_route_name(&request.path).is_some()))
}

fn handle_get_inbox(
    store: &StateStore,
    logger: &StructuredLogger,
    state: &mut AppState,
    request: &ParsedHttpRequest,
) -> LocalStatusResponse {
    let agent_filter = request.query.get("agent").map(String::as_str);
    if let Some(name) = agent_filter {
        if !is_canonical_name(name) {
            return LocalStatusResponse::bad_request_text(
                "agent filter must be canonical and contain no whitespace",
            );
        }
    }

    let wait_seconds = match request.query.get("wait") {
        Some(value) => match services::parse_inbox_wait_seconds(value) {
            Ok(seconds) => seconds,
            Err(error) => return LocalStatusResponse::bad_request_text(error.to_string()),
        },
        None => 0,
    };

    let immediate_messages = services::read_mailbox(state, agent_filter);
    let result = if !immediate_messages.is_empty() || wait_seconds == 0 {
        services::MailboxWaitResult {
            timed_out: immediate_messages.is_empty() && wait_seconds > 0,
            messages: immediate_messages,
            wait_seconds,
        }
    } else {
        match services::read_mailbox_with_wait(
            store,
            agent_filter,
            wait_seconds,
            Duration::from_millis(100),
        ) {
            Ok(result) => {
                if !result.messages.is_empty() {
                    match store.load_existing() {
                        Ok(latest_state) => {
                            state.mailbox = latest_state.mailbox;
                        }
                        Err(error) => {
                            return LocalStatusResponse::internal_server_error_text(format!(
                                "inbox wait found persisted messages but failed to refresh daemon mailbox: {error}"
                            ));
                        }
                    }
                }
                result
            }
            Err(error) => {
                return LocalStatusResponse::internal_server_error_text(format!(
                    "failed to read inbox: {error}"
                ));
            }
        }
    };

    if let Err(error) = logger.event(
        "inbox.read",
        "inbox read",
        match serde_json::to_value(services::inbox_read_event_fields(&result, agent_filter)) {
            Ok(value) => value,
            Err(error) => {
                return LocalStatusResponse::internal_server_error_text(format!(
                    "failed to serialize inbox read event: {error}"
                ));
            }
        },
    ) {
        return LocalStatusResponse::internal_server_error_text(format!(
            "failed to log inbox read: {error}"
        ));
    }

    json_response(&result)
}

fn handle_get_inbox_snapshot(
    store: &StateStore,
    logger: &StructuredLogger,
    state: &AppState,
    request: &ParsedHttpRequest,
) -> LocalStatusResponse {
    let agent_filter = request.query.get("agent").map(String::as_str);
    if let Some(name) = agent_filter {
        if !is_canonical_name(name) {
            return LocalStatusResponse::bad_request_text(
                "agent filter must be canonical and contain no whitespace",
            );
        }
    }

    let wait_seconds = match request.query.get("wait") {
        Some(value) => match services::parse_inbox_wait_seconds(value) {
            Ok(seconds) => seconds,
            Err(error) => return LocalStatusResponse::bad_request_text(error.to_string()),
        },
        None => 0,
    };

    let immediate_messages = services::read_mailbox(state, agent_filter);
    let result = if !immediate_messages.is_empty() || wait_seconds == 0 {
        services::MailboxWaitResult {
            timed_out: immediate_messages.is_empty() && wait_seconds > 0,
            messages: immediate_messages,
            wait_seconds,
        }
    } else {
        match services::read_mailbox_with_wait(
            store,
            agent_filter,
            wait_seconds,
            Duration::from_millis(100),
        ) {
            Ok(result) => result,
            Err(error) => {
                return LocalStatusResponse::internal_server_error_text(format!(
                    "failed to read inbox: {error}"
                ));
            }
        }
    };

    if let Err(error) = logger.event(
        "inbox.read",
        "inbox read",
        match serde_json::to_value(services::inbox_read_event_fields(&result, agent_filter)) {
            Ok(value) => value,
            Err(error) => {
                return LocalStatusResponse::internal_server_error_text(format!(
                    "failed to serialize inbox read event: {error}"
                ));
            }
        },
    ) {
        return LocalStatusResponse::internal_server_error_text(format!(
            "failed to log inbox read: {error}"
        ));
    }

    json_response(&result)
}

fn handle_get_logs(store: &StateStore, request: &ParsedHttpRequest) -> LocalStatusResponse {
    let limit = match request.query.get("limit") {
        Some(value) => match services::parse_log_limit(value) {
            Ok(limit) => Some(limit),
            Err(error) => return LocalStatusResponse::bad_request_text(error.to_string()),
        },
        None => None,
    };
    match services::read_logs(store, limit) {
        Ok(events) => json_response(&events),
        Err(error) => LocalStatusResponse::internal_server_error_text(format!(
            "failed to read local events: {error}"
        )),
    }
}

fn handle_post_shutdown(
    store: &StateStore,
    logger: &StructuredLogger,
    state: &mut AppState,
) -> LocalStatusResponse {
    let status = services::request_daemon_stop(state);
    if let Err(error) = store.save_daemon(&state.daemon) {
        return LocalStatusResponse::internal_server_error_text(format!(
            "failed to persist daemon shutdown request: {error}"
        ));
    }
    let log_warning = if let Err(error) = logger.event(
        "daemon.stop.requested",
        "daemon shutdown requested through local API",
        match serde_json::to_value(services::daemon_stop_requested_event_fields(&status)) {
            Ok(fields) => fields,
            Err(error) => {
                return LocalStatusResponse::internal_server_error_text(format!(
                    "failed to serialize daemon shutdown event fields: {error}"
                ));
            }
        },
    ) {
        Some(format!("failed to log daemon shutdown request: {error}"))
    } else {
        None
    };
    if let Some(warning) = log_warning {
        return json_response(&serde_json::json!({
            "ok": status.ok,
            "shutdown_persisted": true,
            "logging_warning": warning,
            "status": status,
        }));
    }
    json_response(&status)
}

fn handle_post_agent(
    store: &StateStore,
    logger: &StructuredLogger,
    state: &mut AppState,
    request: &ParsedHttpRequest,
) -> LocalStatusResponse {
    let body: PostAgentRequest = match serde_json::from_str(&request.body) {
        Ok(body) => body,
        Err(error) => {
            return LocalStatusResponse::bad_request_text(format!(
                "invalid /v1/agents JSON body: {error}"
            ));
        }
    };
    let name = body.name.clone();
    let agent_request =
        match services::build_create_agent_request(services::BuildCreateAgentRequest {
            name: body.name,
            source: body.source.or(body.thread_source),
            cwd: body.cwd,
            thread_id: body.thread_id,
            model: body.model,
            model_provider: body.model_provider,
            effort: body.effort,
            speed: body.speed,
            service_tier: body.service_tier,
        }) {
            Ok(request) => request,
            Err(error) => return LocalStatusResponse::bad_request_text(error.to_string()),
        };

    let agent = match services::create_agent(state, agent_request) {
        Ok(agent) => agent,
        Err(error) => return LocalStatusResponse::bad_request_text(error.to_string()),
    };
    if let Err(error) = store.save_all(state) {
        return LocalStatusResponse::internal_server_error_text(format!(
            "failed to persist created agent: {error}"
        ));
    }
    if let Err(error) = logger.event(
        "agent.created",
        format!("agent `{name}` created by local API"),
        match serde_json::to_value(services::agent_created_event_fields(&agent)) {
            Ok(fields) => fields,
            Err(error) => {
                return LocalStatusResponse::internal_server_error_text(format!(
                    "failed to serialize agent created event fields: {error}"
                ));
            }
        },
    ) {
        return LocalStatusResponse::internal_server_error_text(format!(
            "failed to log agent creation: {error}"
        ));
    }
    json_response(&agent)
}

fn handle_get_agent(
    logger: &StructuredLogger,
    state: &AppState,
    name: &str,
) -> LocalStatusResponse {
    if !is_canonical_name(name) {
        return LocalStatusResponse::bad_request_text(
            "agent name must be canonical and contain no whitespace",
        );
    }
    let agent = match services::get_agent(state, name) {
        Ok(agent) => agent,
        Err(_) => return LocalStatusResponse::not_found_text(format!("agent `{name}` not found")),
    };
    if let Err(error) = logger.event(
        "agent.inspected",
        format!("agent `{name}` inspected by local API"),
        match serde_json::to_value(services::agent_inspected_event_fields(&agent)) {
            Ok(fields) => fields,
            Err(error) => {
                return LocalStatusResponse::internal_server_error_text(format!(
                    "failed to serialize agent inspected event fields: {error}"
                ));
            }
        },
    ) {
        return LocalStatusResponse::internal_server_error_text(format!(
            "failed to log agent inspection: {error}"
        ));
    }
    json_response(&agent)
}

fn handle_post_agent_model(
    store: &StateStore,
    logger: &StructuredLogger,
    state: &mut AppState,
    name: &str,
    request: &ParsedHttpRequest,
) -> LocalStatusResponse {
    if !is_canonical_name(name) {
        return LocalStatusResponse::bad_request_text(
            "agent name must be canonical and contain no whitespace",
        );
    }
    let body: PostAgentModelRequest = match serde_json::from_str(&request.body) {
        Ok(body) => body,
        Err(error) => {
            return LocalStatusResponse::bad_request_text(format!(
                "invalid /v1/agents/{name}/model JSON body: {error}"
            ));
        }
    };
    let update = match services::build_set_model_update(services::BuildSetModelUpdateRequest {
        model: body.model,
        model_provider: body.model_provider,
        effort: body.effort,
        speed: body.speed,
        service_tier: body.service_tier,
    }) {
        Ok(update) => update,
        Err(error) => return LocalStatusResponse::bad_request_text(error.to_string()),
    };
    let updated = match services::set_agent_model(state, name, update) {
        Ok(agent) => agent,
        Err(error) => return LocalStatusResponse::bad_request_text(error.to_string()),
    };
    if let Err(error) = store.save_all(state) {
        return LocalStatusResponse::internal_server_error_text(format!(
            "failed to persist agent model update: {error}"
        ));
    }
    if let Err(error) = logger.event(
        "agent.model_updated",
        format!("agent `{name}` model settings updated by local API"),
        match serde_json::to_value(services::agent_model_updated_event_fields(&updated)) {
            Ok(fields) => fields,
            Err(error) => {
                return LocalStatusResponse::internal_server_error_text(format!(
                    "failed to serialize agent model event fields: {error}"
                ));
            }
        },
    ) {
        return LocalStatusResponse::internal_server_error_text(format!(
            "failed to log agent model update: {error}"
        ));
    }
    json_response(&updated)
}

fn handle_post_agent_resume(
    store: &StateStore,
    logger: &StructuredLogger,
    state: &mut AppState,
    runtime: &mut services::DaemonRuntime,
    name: &str,
    request: &ParsedHttpRequest,
) -> LocalStatusResponse {
    if !is_canonical_name(name) {
        return LocalStatusResponse::bad_request_text(
            "agent name must be canonical and contain no whitespace",
        );
    }
    let body = if request.body.trim().is_empty() {
        PostAgentResumeRequest::default()
    } else {
        match serde_json::from_str(&request.body) {
            Ok(body) => body,
            Err(error) => {
                return LocalStatusResponse::bad_request_text(format!(
                    "invalid /v1/agents/{name}/resume JSON body: {error}"
                ));
            }
        }
    };
    if body.codex_stdio || body.codex_program.is_some() {
        return LocalStatusResponse::bad_request_text(
            "daemon API resume does not use one-shot Codex stdio; use daemon-owned message delivery for active attention routing".to_string(),
        );
    }
    let application = match state.find_agent(name).cloned() {
        Some(agent) if agent.kind == AgentKind::Codex && agent.route == Route::Local => {
            match runtime.resume_local_codex_with_owner_runtime(state, name) {
                Ok(application) => application,
                Err(error) => return LocalStatusResponse::bad_request_text(error.to_string()),
            }
        }
        _ => match services::resume_agent_with_peer_fetcher(
            state,
            name,
            &DefaultProviderRouter,
            &SshPeerAgentResumeFetcher,
        ) {
            Ok(application) => application,
            Err(error) => return LocalStatusResponse::bad_request_text(error.to_string()),
        },
    };
    if application.persist_state {
        if let Err(error) = store.save_all(state) {
            return LocalStatusResponse::internal_server_error_text(format!(
                "failed to persist agent resume result: {error}"
            ));
        }
    }
    let result = application.result;
    if let Err(error) = logger.event(
        result.event_type(),
        format!("agent `{name}` resume checked by local API"),
        match serde_json::to_value(services::agent_resume_event_fields(&result)) {
            Ok(fields) => fields,
            Err(error) => {
                return LocalStatusResponse::internal_server_error_text(format!(
                    "failed to serialize agent resume event fields: {error}"
                ));
            }
        },
    ) {
        return LocalStatusResponse::internal_server_error_text(format!(
            "failed to log agent resume: {error}"
        ));
    }
    json_response(&result)
}

fn handle_get_agent_thread(
    logger: &StructuredLogger,
    state: &AppState,
    name: &str,
    request: &ParsedHttpRequest,
) -> LocalStatusResponse {
    if !is_canonical_name(name) {
        return LocalStatusResponse::bad_request_text(
            "agent name must be canonical and contain no whitespace",
        );
    }
    let query = match parse_agent_thread_query(&request.query) {
        Ok(query) => query,
        Err(message) => return LocalStatusResponse::bad_request_text(message),
    };
    let snapshot = match services::read_agent_snapshot_with_peer_fetcher(
        state,
        name,
        query.options,
        &DefaultProviderRouter,
        &SshPeerAgentReadFetcher,
    ) {
        Ok(snapshot) => snapshot,
        Err(error) => return LocalStatusResponse::bad_request_text(error.to_string()),
    };
    if let Err(error) = logger.event(
        "agent.read",
        format!("agent `{name}` thread read by local API"),
        match serde_json::to_value(services::agent_read_event_fields(&snapshot)) {
            Ok(fields) => fields,
            Err(error) => {
                return LocalStatusResponse::internal_server_error_text(format!(
                    "failed to serialize agent read event fields: {error}"
                ));
            }
        },
    ) {
        return LocalStatusResponse::internal_server_error_text(format!(
            "failed to log agent thread read: {error}"
        ));
    }
    json_response(&snapshot)
}

fn parse_agent_thread_query(query: &BTreeMap<String, String>) -> Result<AgentThreadQuery, String> {
    for key in query.keys() {
        if !matches!(
            key.as_str(),
            "latest" | "include_turns" | "turns" | "wait_seconds"
        ) {
            return Err(format!(
                "unsupported /v1/agents/{{name}}/thread query parameter `{key}`"
            ));
        }
    }
    let latest_only = match query.get("latest") {
        Some(value) => parse_bool_query_value("latest", value)?,
        None => false,
    };
    let include_turns = match query.get("include_turns") {
        Some(value) => parse_bool_query_value("include_turns", value)?,
        None => false,
    };
    let turn_limit = match query.get("turns") {
        Some(value) => Some(
            services::parse_agent_read_turn_limit(value)
                .map_err(|error| error.to_string().replace("--turns", "turns"))?,
        ),
        None => None,
    };
    let wait_seconds = match query.get("wait_seconds") {
        Some(value) => Some(
            services::parse_agent_read_wait_seconds(value)
                .map_err(|error| error.to_string().replace("--wait-seconds", "wait_seconds"))?,
        ),
        None => None,
    };
    Ok(AgentThreadQuery {
        options: services::AgentReadOptions {
            latest_only,
            include_turns: include_turns || turn_limit.is_some(),
            turn_limit,
            wait_seconds,
        },
    })
}

fn parse_bool_query_value(name: &str, value: &str) -> Result<bool, String> {
    match value {
        "" | "1" | "true" | "yes" => Ok(true),
        "0" | "false" | "no" => Ok(false),
        _ => Err(format!(
            "/v1/agents/{{name}}/thread query parameter `{name}` must be true or false"
        )),
    }
}

fn handle_post_discovery_local_run(
    store: &StateStore,
    logger: &StructuredLogger,
    state: &mut AppState,
    request: &ParsedHttpRequest,
) -> LocalStatusResponse {
    let body = if request.body.trim().is_empty() {
        PostDiscoveryLocalRunRequest::default()
    } else {
        match serde_json::from_str(&request.body) {
            Ok(body) => body,
            Err(error) => {
                return LocalStatusResponse::bad_request_text(format!(
                    "invalid /v1/discovery/local:run JSON body: {error}"
                ));
            }
        }
    };
    if let Err(error) = services::validate_command_optional_nonempty_trimmed(
        "codex_home",
        body.codex_home.as_deref(),
    ) {
        return LocalStatusResponse::bad_request_text(error.to_string());
    }
    let application = match services::run_local_discovery(
        state,
        body.codex_home.map(PathBuf::from),
        body.promote_approved,
    ) {
        Ok(application) => application,
        Err(error) => return LocalStatusResponse::bad_request_text(error.to_string()),
    };
    if application.persist_state {
        if let Err(error) = store.save_all(state) {
            return LocalStatusResponse::internal_server_error_text(format!(
                "failed to persist local discovery result: {error}"
            ));
        }
    }
    let source_path_present = application.source_path.is_some();
    if let Err(error) = logger.event(
        "discovery.local.scanned",
        if source_path_present {
            "local discovery scan completed by local API"
        } else {
            "local discovery scan completed with no Codex home by local API"
        },
        match serde_json::to_value(services::discovery_local_scanned_event_fields(&application)) {
            Ok(fields) => fields,
            Err(error) => {
                return LocalStatusResponse::internal_server_error_text(format!(
                    "failed to serialize local discovery event fields: {error}"
                ));
            }
        },
    ) {
        return LocalStatusResponse::internal_server_error_text(format!(
            "failed to log local discovery: {error}"
        ));
    }
    json_response(&application.summary)
}

fn handle_post_peer(
    store: &StateStore,
    logger: &StructuredLogger,
    state: &mut AppState,
    request: &ParsedHttpRequest,
) -> LocalStatusResponse {
    let body: PostPeerRequest = match serde_json::from_str(&request.body) {
        Ok(body) => body,
        Err(error) => {
            return LocalStatusResponse::bad_request_text(format!(
                "invalid /v1/peers JSON body: {error}"
            ));
        }
    };
    let name = body.name.clone();
    let peer_request = match services::build_enroll_peer_request(services::BuildEnrollPeerRequest {
        peer_names: vec![body.name],
        ssh_target: body.ssh_target.or(body.ssh),
        key_path: body.key_path.or(body.key),
        remote_root: body.remote_root,
    }) {
        Ok(request) => request,
        Err(error) => return LocalStatusResponse::bad_request_text(error.to_string()),
    };
    let outcome = match services::enroll_peer(state, peer_request) {
        Ok(outcome) => outcome,
        Err(error) => return LocalStatusResponse::bad_request_text(error.to_string()),
    };
    if let Err(error) = store.save_all(state) {
        return LocalStatusResponse::internal_server_error_text(format!(
            "failed to persist peer enrollment: {error}"
        ));
    }
    let event_type = if outcome.updated_existing {
        "peer.updated"
    } else {
        "peer.added"
    };
    if let Err(error) = logger.event(
        event_type,
        format!("peer `{name}` enrolled or updated by local API"),
        match serde_json::to_value(services::peer_enrollment_event_fields(&outcome)) {
            Ok(fields) => fields,
            Err(error) => {
                return LocalStatusResponse::internal_server_error_text(format!(
                    "failed to serialize peer enrollment event fields: {error}"
                ));
            }
        },
    ) {
        return LocalStatusResponse::internal_server_error_text(format!(
            "failed to log peer enrollment: {error}"
        ));
    }
    json_response(&outcome.peer)
}

fn handle_post_peers_sync(
    store: &StateStore,
    logger: &StructuredLogger,
    state: &mut AppState,
    request: &ParsedHttpRequest,
) -> LocalStatusResponse {
    if !request.body.trim().is_empty() {
        return LocalStatusResponse::bad_request_text(
            "/v1/peers:sync does not accept a request body".to_string(),
        );
    }
    let aggregate = services::sync_all_peer_inventories_from_fetcher(state, &SshInventoryFetcher);
    if let Err(error) = store.save_all(state) {
        return LocalStatusResponse::internal_server_error_text(format!(
            "failed to persist peer sync-all result: {error}"
        ));
    }
    for result in &aggregate.results {
        if let Err(error) = log_api_peer_sync_result(logger, result) {
            return LocalStatusResponse::internal_server_error_text(format!(
                "failed to log peer sync result: {error}"
            ));
        }
    }
    if let Err(error) = log_api_peer_sync_all_result(logger, &aggregate) {
        return LocalStatusResponse::internal_server_error_text(format!(
            "failed to log peer sync-all result: {error}"
        ));
    }
    json_response(&aggregate)
}

fn handle_post_peer_sync(
    store: &StateStore,
    logger: &StructuredLogger,
    state: &mut AppState,
    name: &str,
) -> LocalStatusResponse {
    if !is_canonical_name(name) {
        return LocalStatusResponse::bad_request_text(
            "peer name must be canonical and contain no whitespace",
        );
    }
    if state.find_peer(name).is_none() {
        return LocalStatusResponse::not_found_text(format!("peer `{name}` not found"));
    }

    let result = match services::sync_peer_inventory_from_fetcher(state, name, &SshInventoryFetcher)
    {
        Ok(result) => result,
        Err(error) => return LocalStatusResponse::bad_request_text(error.to_string()),
    };
    if let Err(error) = store.save_all(state) {
        return LocalStatusResponse::internal_server_error_text(format!(
            "failed to persist peer sync result: {error}"
        ));
    }
    if let Err(error) = log_api_peer_sync_result(logger, &result) {
        return LocalStatusResponse::internal_server_error_text(format!(
            "failed to log peer sync result: {error}"
        ));
    }
    json_response(&result)
}

fn handle_post_message(
    store: &StateStore,
    logger: &StructuredLogger,
    state: &mut AppState,
    runtime: &mut services::DaemonRuntime,
    request: &ParsedHttpRequest,
) -> LocalStatusResponse {
    let body: PostMessageRequest = match serde_json::from_str(&request.body) {
        Ok(body) => body,
        Err(error) => {
            return LocalStatusResponse::bad_request_text(format!(
                "invalid /v1/messages JSON body: {error}"
            ));
        }
    };
    let message_body = match body.body.or(body.message) {
        Some(message) => message,
        None => {
            return LocalStatusResponse::bad_request_text(
                "/v1/messages requires `body` or `message`".to_string(),
            );
        }
    };
    let built = match services::build_send_request(services::BuildSendRequest {
        positionals: vec![body.target_agent.clone(), message_body],
        source_agent: body.source_agent,
        source_node: body.source_node,
        correlation_id: body.correlation_id,
        message_type: body.message_type,
        receipt_nonce: body.receipt_nonce,
        expected_delivery: body.expected_delivery,
        delivery_action: body.delivery_action,
        strict: body.strict.unwrap_or(true),
        use_codex_stdio: true,
        codex_program: body.codex_program,
    }) {
        Ok(request) => request,
        Err(error) => return LocalStatusResponse::bad_request_text(error.to_string()),
    };

    let Some(target) = state.find_agent(&body.target_agent).cloned() else {
        return apply_local_send_decision(store, logger, state, built);
    };
    if target.kind == AgentKind::Codex && target.route == Route::Local {
        let receipt_nonce = built.receipt_nonce.clone();
        let codex_model = target.model.clone();
        let failure_context = codex_runtime_failure_context(state, &target, built.clone());
        return match runtime.send_local_codex_with_owner_runtime(state, built, codex_model) {
            Ok(application) => {
                if let Err(error) = store.save_all(state) {
                    return LocalStatusResponse::internal_server_error_text(format!(
                        "failed to persist Codex send result: {error}"
                    ));
                }
                if let Err(error) = log_api_message_event(
                    logger,
                    match message_success_event_type(&application.message) {
                        Ok(event_type) => event_type,
                        Err(error) => {
                            return LocalStatusResponse::internal_server_error_text(
                                error.to_string(),
                            );
                        }
                    },
                    &application.message,
                    Some(&application.target),
                    &application.decision,
                    &application.decision.reason,
                    None,
                ) {
                    return LocalStatusResponse::internal_server_error_text(format!(
                        "failed to log Codex send result: {error}"
                    ));
                }
                let mut result = services::SendResult::from_message(&application.message);
                result.receipt_nonce = receipt_nonce;
                result.apply_message(&application.message, true, None);
                json_response(&result)
            }
            Err(error) => {
                let failed = services::apply_direct_delivery_failure(
                    state,
                    target,
                    failure_context.decision,
                    failure_context.message,
                    &error,
                    services::DirectDeliveryFailureKind::Recoverable,
                );
                if failed.persist_state {
                    if let Err(error) = store.save_all(state) {
                        return LocalStatusResponse::internal_server_error_text(format!(
                            "failed to persist Codex send failure result: {error}"
                        ));
                    }
                }
                if let Err(log_error) = log_api_message_event(
                    logger,
                    failed.event_type,
                    &failed.message,
                    failed.target.as_ref(),
                    &failed.decision,
                    failed
                        .result_error
                        .as_deref()
                        .unwrap_or(failed.decision.reason.as_str()),
                    Some(&error),
                ) {
                    return LocalStatusResponse::internal_server_error_text(format!(
                        "failed to log Codex send failure: {log_error}"
                    ));
                }
                let mut result = services::SendResult::from_message(&failed.message);
                result.receipt_nonce = receipt_nonce;
                result.apply_message_with_error(
                    &failed.message,
                    failed.ok,
                    failed.result_error,
                    Some(&error),
                );
                json_response(&result)
            }
        };
    }

    apply_local_send_decision(store, logger, state, built)
}

fn agent_model_route_name(path: &str) -> Option<&str> {
    let name = path.strip_prefix("/v1/agents/")?.strip_suffix("/model")?;
    (!name.is_empty() && !name.contains('/')).then_some(name)
}

fn agent_resume_route_name(path: &str) -> Option<&str> {
    let name = path.strip_prefix("/v1/agents/")?.strip_suffix("/resume")?;
    (!name.is_empty() && !name.contains('/')).then_some(name)
}

fn agent_thread_route_name(path: &str) -> Option<&str> {
    let name = path.strip_prefix("/v1/agents/")?.strip_suffix("/thread")?;
    (!name.is_empty() && !name.contains('/')).then_some(name)
}

fn peer_sync_route_name(path: &str) -> Option<&str> {
    let name = path.strip_prefix("/v1/peers/")?.strip_suffix("/sync")?;
    (!name.is_empty() && !name.contains('/')).then_some(name)
}

struct RuntimeFailureContext {
    decision: DeliveryDecision,
    message: crate::core::Message,
}

fn codex_runtime_failure_context(
    state: &AppState,
    target: &Agent,
    built: services::BuiltSendRequest,
) -> RuntimeFailureContext {
    if target.thread_id.is_none() {
        let mut message = crate::core::Message::new(
            built.prepare.target_agent.clone(),
            built.prepare.body.clone(),
        );
        message.source_agent = built.prepare.source_agent;
        message.source_node = Some(
            built
                .prepare
                .source_node
                .unwrap_or_else(|| state.config.node_name.clone()),
        );
        message.correlation_id = built.prepare.correlation_id;
        message.message_type = built.prepare.message_type;
        message.strict = built.prepare.strict;
        return RuntimeFailureContext {
            decision: DeliveryDecision {
                state: DeliveryState::Delivered,
                action: DeliveryAction::WakeKnownSession,
                reason: "daemon owner runtime is creating a Codex thread before delivery"
                    .to_string(),
            },
            message,
        };
    }

    let prepared = services::prepare_send(state, built.prepare);
    RuntimeFailureContext {
        decision: prepared.decision,
        message: prepared.message,
    }
}

fn apply_local_send_decision(
    store: &StateStore,
    logger: &StructuredLogger,
    state: &mut AppState,
    built: services::BuiltSendRequest,
) -> LocalStatusResponse {
    let prepared = services::prepare_send(state, built.prepare);
    if let Err(error) = services::validate_send_contract_metadata(
        &prepared.decision,
        built.expected_delivery.as_ref(),
        built.delivery_action.as_ref(),
    ) {
        return LocalStatusResponse::bad_request_text(error.to_string());
    }
    let mut result = services::SendResult::from_message(&prepared.message);
    result.receipt_nonce = built.receipt_nonce;
    let application = match services::apply_prepared_send_decision(state, prepared) {
        Ok(application) => application,
        Err(error) => {
            return LocalStatusResponse::internal_server_error_text(format!(
                "failed to apply send decision: {error}"
            ));
        }
    };

    match application {
        services::SendApplication::Local(application) => {
            let local_error = services::local_send_decision_error(&application);
            result.apply_message_with_error(
                &application.message,
                application.ok,
                application.result_error.clone(),
                local_error.as_ref(),
            );
            if application.persist_state {
                if let Err(error) = store.save_all(state) {
                    return LocalStatusResponse::internal_server_error_text(format!(
                        "failed to persist local send result: {error}"
                    ));
                }
            }
            if let Err(error) = log_api_message_event(
                logger,
                application.event_type,
                &application.message,
                application.target.as_ref(),
                &application.decision,
                application
                    .result_error
                    .as_deref()
                    .unwrap_or(application.decision.reason.as_str()),
                local_error.as_ref(),
            ) {
                return LocalStatusResponse::internal_server_error_text(format!(
                    "failed to log local send result: {error}"
                ));
            }
            json_response(&result)
        }
        services::SendApplication::Direct(application) => {
            let agent = application.target;
            let decision = application.decision;
            let mut message = application.message;
            let delivery = execute_direct_delivery(
                state,
                &agent,
                &mut message,
                &decision,
                &DefaultProviderRouter,
                &SshPeerMessageSender,
            );
            match delivery {
                Ok(event_type) => {
                    if let Err(error) = store.save_all(state) {
                        return LocalStatusResponse::internal_server_error_text(format!(
                            "failed to persist direct send result: {error}"
                        ));
                    }
                    result.apply_message(&message, true, None);
                    if let Err(error) = log_api_message_event(
                        logger,
                        event_type,
                        &message,
                        Some(&agent),
                        &decision,
                        &decision.reason,
                        None,
                    ) {
                        return LocalStatusResponse::internal_server_error_text(format!(
                            "failed to log direct send result: {error}"
                        ));
                    }
                    json_response(&result)
                }
                Err(DirectDeliveryError::Terminal(error)) => {
                    LocalStatusResponse::internal_server_error_text(format!(
                        "direct send failed before a recoverable delivery outcome: {error}"
                    ))
                }
                Err(DirectDeliveryError::Unqueueable(error)) => {
                    let failed = services::apply_direct_delivery_failure(
                        state,
                        agent,
                        decision,
                        message,
                        &error,
                        services::DirectDeliveryFailureKind::Unqueueable,
                    );
                    result.apply_message_with_error(
                        &failed.message,
                        failed.ok,
                        failed.result_error.clone(),
                        Some(&error),
                    );
                    if failed.persist_state {
                        if let Err(error) = store.save_all(state) {
                            return LocalStatusResponse::internal_server_error_text(format!(
                                "failed to persist direct send failure: {error}"
                            ));
                        }
                    }
                    if let Err(log_error) = log_api_message_event(
                        logger,
                        failed.event_type,
                        &failed.message,
                        failed.target.as_ref(),
                        &failed.decision,
                        failed
                            .result_error
                            .as_deref()
                            .unwrap_or(failed.decision.reason.as_str()),
                        Some(&error),
                    ) {
                        return LocalStatusResponse::internal_server_error_text(format!(
                            "failed to log direct send failure: {log_error}"
                        ));
                    }
                    json_response(&result)
                }
                Err(DirectDeliveryError::Recoverable(error)) => {
                    let failed = services::apply_direct_delivery_failure(
                        state,
                        agent,
                        decision,
                        message,
                        &error,
                        services::DirectDeliveryFailureKind::Recoverable,
                    );
                    result.apply_message_with_error(
                        &failed.message,
                        failed.ok,
                        failed.result_error.clone(),
                        Some(&error),
                    );
                    if failed.persist_state {
                        if let Err(error) = store.save_all(state) {
                            return LocalStatusResponse::internal_server_error_text(format!(
                                "failed to persist direct send failure: {error}"
                            ));
                        }
                    }
                    if let Err(log_error) = log_api_message_event(
                        logger,
                        failed.event_type,
                        &failed.message,
                        failed.target.as_ref(),
                        &failed.decision,
                        failed
                            .result_error
                            .as_deref()
                            .unwrap_or(failed.decision.reason.as_str()),
                        Some(&error),
                    ) {
                        return LocalStatusResponse::internal_server_error_text(format!(
                            "failed to log direct send failure: {log_error}"
                        ));
                    }
                    json_response(&result)
                }
            }
        }
    }
}

fn log_api_message_event(
    logger: &StructuredLogger,
    event_type: &str,
    message: &crate::core::Message,
    target: Option<&crate::core::Agent>,
    decision: &crate::delivery::DeliveryDecision,
    reason: &str,
    error: Option<&crate::errors::CamError>,
) -> Result<(), crate::errors::CamError> {
    logger.event(
        event_type,
        format!(
            "message `{}` is {}",
            message.message_id,
            services::delivery_state_label(&message.delivery)
        ),
        serde_json::to_value(services::message_event_fields(
            message, target, decision, reason, error,
        ))?,
    )
}

fn log_api_peer_sync_result(
    logger: &StructuredLogger,
    result: &services::PeerSyncResult,
) -> Result<(), crate::errors::CamError> {
    let event_type = if result.ok {
        "peer.sync.completed"
    } else {
        "peer.sync.failed"
    };
    let message = if result.ok {
        format!(
            "peer `{}` sync completed from trusted inventory export",
            result.peer
        )
    } else if result.remote_command_attempted {
        format!(
            "peer `{}` sync failed loudly after remote execution",
            result.peer
        )
    } else {
        format!(
            "peer `{}` sync failed loudly before remote execution",
            result.peer
        )
    };
    logger.event(
        event_type,
        message,
        serde_json::to_value(services::peer_sync_event_fields(result))?,
    )
}

fn log_api_peer_sync_all_result(
    logger: &StructuredLogger,
    aggregate: &services::PeerSyncAllResult,
) -> Result<(), crate::errors::CamError> {
    let event_type = if aggregate.peers_requested == 0 {
        "peer.sync.all.noop"
    } else if aggregate.peers_failed == 0 {
        "peer.sync.all.completed"
    } else {
        "peer.sync.all.failed"
    };
    let message = if aggregate.peers_requested == 0 {
        "peer sync-all had no enrolled peers"
    } else {
        "peer sync-all completed trusted inventory attempts"
    };
    logger.event(
        event_type,
        message,
        serde_json::to_value(services::peer_sync_all_event_fields(aggregate))?,
    )
}

fn message_success_event_type(
    message: &crate::core::Message,
) -> Result<&'static str, crate::errors::CamError> {
    match message.delivery {
        crate::core::DeliveryState::Steered => Ok("message.steered"),
        crate::core::DeliveryState::Delivered => Ok("message.delivered"),
        ref other => Err(crate::errors::CamError::InvalidState(format!(
            "direct API send success cannot log delivery state `{}`",
            services::delivery_state_label(other)
        ))),
    }
}

fn json_response<T: serde::Serialize>(value: &T) -> LocalStatusResponse {
    match serde_json::to_string_pretty(value) {
        Ok(body) => LocalStatusResponse::ok_json(body),
        Err(error) => LocalStatusResponse::internal_server_error_text(format!(
            "failed to serialize local API response: {error}"
        )),
    }
}

fn is_authorized(store: &StateStore, request: &ParsedHttpRequest) -> bool {
    let Ok(expected) = store.read_local_api_token() else {
        return false;
    };
    let Some(header) = request.headers.get("authorization") else {
        return false;
    };
    let Some(token) = header.strip_prefix("Bearer ") else {
        return false;
    };
    token == expected
}

fn is_canonical_name(name: &str) -> bool {
    !name.is_empty() && name.trim() == name && !name.contains(char::is_whitespace)
}

fn parse_http_request(raw_request: &str) -> Result<ParsedHttpRequest, String> {
    let (head, body) = split_http_head_and_body(raw_request);
    let mut lines = head.lines();
    let Some(request_line) = lines.next().map(str::trim) else {
        return Err("missing HTTP request line".to_string());
    };
    let mut parts = request_line.split_whitespace();
    let Some(method) = parts.next() else {
        return Err("missing HTTP method".to_string());
    };
    let Some(raw_path) = parts.next() else {
        return Err("missing HTTP path".to_string());
    };
    let Some(version) = parts.next() else {
        return Err("missing HTTP version".to_string());
    };
    if parts.next().is_some() || !version.starts_with("HTTP/") {
        return Err("malformed HTTP request line".to_string());
    }

    let (path, query) = parse_path_and_query(raw_path);
    let mut headers = BTreeMap::new();
    for line in lines {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err("malformed HTTP header".to_string());
        };
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }

    Ok(ParsedHttpRequest {
        method: method.to_string(),
        path,
        query,
        headers,
        body,
    })
}

fn split_http_head_and_body(raw_request: &str) -> (&str, String) {
    if let Some((head, body)) = raw_request.split_once("\r\n\r\n") {
        return (head, body.to_string());
    }
    if let Some((head, body)) = raw_request.split_once("\n\n") {
        return (head, body.to_string());
    }
    (raw_request, String::new())
}

fn parse_path_and_query(raw_path: &str) -> (String, BTreeMap<String, String>) {
    let Some((path, raw_query)) = raw_path.split_once('?') else {
        return (raw_path.to_string(), BTreeMap::new());
    };
    let mut query = BTreeMap::new();
    for pair in raw_query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        query.insert(key.to_string(), value.to_string());
    }
    (path.to_string(), query)
}
