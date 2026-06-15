use crate::core::{Agent, AgentKind, Route};
use crate::errors::CamError;
use crate::local_status::is_loopback_bind;
use crate::logging::StoredLogEvent;
use crate::services;
use crate::state::AppState;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

fn is_canonical_name(name: &str) -> bool {
    !name.is_empty() && name.trim() == name && !name.contains(char::is_whitespace)
}

fn format_kind(kind: &AgentKind) -> &'static str {
    match kind {
        AgentKind::Codex => "codex",
        AgentKind::VirtualInbox => "virtual_inbox",
        AgentKind::AgySession => "agy_session",
        AgentKind::RemoteMirror => "remote_mirror",
    }
}

fn format_route(route: &Route) -> String {
    match route {
        Route::Local => "local".to_string(),
        Route::Peer { peer_name } => format!("peer:{peer_name}"),
    }
}

pub fn validate_event_log_invariants(
    state: &AppState,
    events: &[StoredLogEvent],
) -> Result<(), CamError> {
    let mut previous_at = None;
    let mut open_command = None;
    let mut recent_peer_sync_events = Vec::new();

    for event in events {
        let at = parse_event_timestamp(&event.at, &event.event_type)?;
        if let Some(previous_at) = previous_at {
            if at < previous_at {
                return Err(CamError::InvalidState(format!(
                    "event `{}` timestamp moved backwards",
                    event.event_type
                )));
            }
        }
        previous_at = Some(at);

        if !is_canonical_event_type(&event.event_type) {
            return Err(CamError::InvalidState(format!(
                "event type `{}` must be lowercase dot-separated words",
                event.event_type
            )));
        }
        if !has_known_event_prefix(&event.event_type) {
            return Err(CamError::InvalidState(format!(
                "event type `{}` has unknown prefix",
                event.event_type
            )));
        }
        if event.message.trim().is_empty() {
            return Err(CamError::InvalidState(format!(
                "event `{}` has empty message",
                event.event_type
            )));
        }
        if !event.fields.is_object() {
            return Err(CamError::InvalidState(format!(
                "event `{}` fields must be a JSON object",
                event.event_type
            )));
        }
        validate_event_payload_contract(state, event)?;
        match event.event_type.as_str() {
            "peer.sync.completed" | "peer.sync.failed" => {
                recent_peer_sync_events.push(event.clone());
            }
            "peer.sync.all.completed" | "peer.sync.all.noop" | "peer.sync.all.failed" => {
                validate_peer_sync_aggregate_matches_recent_events(
                    event,
                    &recent_peer_sync_events,
                )?;
                recent_peer_sync_events.clear();
            }
            event_type if !event_type.starts_with("peer.sync.") => {
                recent_peer_sync_events.clear();
            }
            _ => {}
        }
        if matches!(
            event.event_type.as_str(),
            "command.started" | "command.finished" | "command.failed"
        ) {
            let command = event
                .fields
                .get("command")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if command.trim().is_empty() || command.trim() != command {
                return Err(CamError::InvalidState(format!(
                    "event `{}` is missing canonical command field",
                    event.event_type
                )));
            }
            if !is_known_command(command) {
                return Err(CamError::InvalidState(format!(
                    "event `{}` has unknown command `{command}`",
                    event.event_type
                )));
            }
            if event.event_type == "command.started" {
                if let Some(open_command) = open_command.as_deref() {
                    return Err(CamError::InvalidState(format!(
                        "command `{command}` started before command `{open_command}` finished"
                    )));
                }
                open_command = Some(command.to_string());
            } else {
                let terminal_verb = if event.event_type == "command.failed" {
                    "failed"
                } else {
                    "finished"
                };
                let Some(started) = open_command.take() else {
                    return Err(CamError::InvalidState(format!(
                        "command `{command}` {terminal_verb} without a matching start"
                    )));
                };
                if started != command {
                    return Err(CamError::InvalidState(format!(
                        "command `{command}` {terminal_verb} but `{started}` was open"
                    )));
                }
            }
        }
    }
    if let Some(open_command) = open_command {
        if open_command != "doctor" {
            return Err(CamError::InvalidState(format!(
                "command `{open_command}` started without a matching finish"
            )));
        }
    }
    Ok(())
}

fn validate_peer_sync_aggregate_matches_recent_events(
    aggregate: &StoredLogEvent,
    recent_peer_events: &[StoredLogEvent],
) -> Result<(), CamError> {
    let peers_requested = require_usize_field(aggregate, "peers_requested")?;
    if peers_requested != recent_peer_events.len() {
        return Err(CamError::InvalidState(format!(
            "event `{}` peer count does not match immediately preceding peer sync events",
            aggregate.event_type
        )));
    }
    let mut peers_synced = 0;
    let mut peers_failed = 0;
    let mut peers_degraded = 0;
    let mut agents_imported = 0;
    let mut remote_agents_seen = 0;
    let mut mirrored_agents_added = 0;
    let mut mirrored_agents_updated = 0;
    let mut mirrored_agents_skipped = 0;
    let mut mirrored_agents_stale = 0;
    let mut collision_count = 0;
    let mut remote_command_attempted = false;
    let mut failed_error_kinds = Vec::new();

    for event in recent_peer_events {
        let ok = require_bool_field(event, "ok")?;
        if ok {
            peers_synced += 1;
        } else {
            peers_failed += 1;
            failed_error_kinds.push(require_string_field(event, "error_kind")?.to_string());
        }
        if require_bool_field(event, "degraded")? {
            peers_degraded += 1;
        }
        if require_bool_field(event, "remote_command_attempted")? {
            remote_command_attempted = true;
        }
        agents_imported += require_usize_field(event, "agents_imported")?;
        remote_agents_seen += require_usize_field(event, "remote_agents_seen")?;
        mirrored_agents_added += require_usize_field(event, "mirrored_agents_added")?;
        mirrored_agents_updated += require_usize_field(event, "mirrored_agents_updated")?;
        mirrored_agents_skipped += require_usize_field(event, "mirrored_agents_skipped")?;
        mirrored_agents_stale += require_usize_field(event, "mirrored_agents_stale")?;
        collision_count += require_usize_field(event, "collision_count")?;
    }

    failed_error_kinds.sort();
    failed_error_kinds.dedup();
    require_usize_field_value(aggregate, "peers_synced", peers_synced)?;
    require_usize_field_value(aggregate, "peers_failed", peers_failed)?;
    require_usize_field_value(aggregate, "peers_degraded", peers_degraded)?;
    require_usize_field_value(aggregate, "agents_imported", agents_imported)?;
    require_usize_field_value(aggregate, "remote_agents_seen", remote_agents_seen)?;
    require_usize_field_value(aggregate, "mirrored_agents_added", mirrored_agents_added)?;
    require_usize_field_value(
        aggregate,
        "mirrored_agents_updated",
        mirrored_agents_updated,
    )?;
    require_usize_field_value(
        aggregate,
        "mirrored_agents_skipped",
        mirrored_agents_skipped,
    )?;
    require_usize_field_value(aggregate, "mirrored_agents_stale", mirrored_agents_stale)?;
    require_usize_field_value(aggregate, "collision_count", collision_count)?;
    require_bool_field_value(
        aggregate,
        "remote_command_attempted",
        remote_command_attempted,
    )?;
    require_string_array_field_value(aggregate, "failed_error_kinds", &failed_error_kinds)
}

pub fn validate_event_mirror(
    events: &[StoredLogEvent],
    daemon_events: &[StoredLogEvent],
) -> Result<(), CamError> {
    if events != daemon_events {
        return Err(CamError::InvalidState(
            "logs/daemon.log does not exactly mirror events.jsonl".to_string(),
        ));
    }
    Ok(())
}

fn validate_event_payload_contract(
    state: &AppState,
    event: &StoredLogEvent,
) -> Result<(), CamError> {
    if is_loud_failure_event(&event.event_type) && !has_error_like_payload(&event.fields) {
        return Err(CamError::InvalidState(format!(
            "event `{}` is missing loud failure payload",
            event.event_type
        )));
    }

    if event.event_type.starts_with("message.") {
        validate_message_event_payload(state, event)?;
    }

    match event.event_type.as_str() {
        "doctor.failed" => {
            require_error_kind_field(event)?;
            require_string_field(event, "error")?;
        }
        "doctor.providers.checked" => {
            validate_doctor_providers_checked_event_payload(event)?;
        }
        "command.failed" => {
            require_string_field(event, "command")?;
            require_bool_field_value(event, "ok", false)?;
            require_error_kind_field(event)?;
            require_string_field(event, "error")?;
        }
        "send.daemon.used" | "send.daemon.fallback" | "send.daemon.failed" => {
            validate_send_daemon_route_event_payload(event)?;
        }
        "resume.daemon.used" | "resume.daemon.fallback" | "resume.daemon.failed" => {
            validate_resume_daemon_route_event_payload(event)?;
        }
        "agent.create.daemon.used"
        | "agent.create.daemon.fallback"
        | "agent.create.daemon.failed" => {
            validate_agent_create_daemon_route_event_payload(event)?;
        }
        "agent.set_model.daemon.used"
        | "agent.set_model.daemon.fallback"
        | "agent.set_model.daemon.failed" => {
            validate_agent_set_model_daemon_route_event_payload(event)?;
        }
        "agent.status.daemon.used"
        | "agent.status.daemon.fallback"
        | "agent.status.daemon.failed" => {
            validate_agent_status_daemon_route_event_payload(event)?;
        }
        "agent.read.daemon.used" | "agent.read.daemon.fallback" | "agent.read.daemon.failed" => {
            validate_agent_read_daemon_route_event_payload(event)?;
        }
        "api.request.started" => {
            validate_api_request_started_event_payload(event)?;
        }
        "api.request.finished" => {
            validate_api_request_finished_event_payload(event)?;
        }
        "daemon.status.checked" | "daemon.start.blocked" | "daemon.stop.requested" => {
            validate_daemon_lifecycle_event_payload(event)?;
            if event.event_type == "daemon.start.blocked" {
                require_string_field(event, "error")?;
            }
        }
        "daemon.health.daemon.used"
        | "daemon.health.daemon.fallback"
        | "daemon.health.daemon.failed" => {
            validate_daemon_health_route_event_payload(event)?;
        }
        "peer.sync.daemon.used" | "peer.sync.daemon.fallback" | "peer.sync.daemon.failed" => {
            validate_peer_sync_daemon_route_event_payload(event)?;
        }
        "peer.add.daemon.used" | "peer.add.daemon.fallback" | "peer.add.daemon.failed" => {
            validate_peer_add_daemon_route_event_payload(event)?;
        }
        "discovery.local.daemon.used"
        | "discovery.local.daemon.fallback"
        | "discovery.local.daemon.failed" => {
            validate_discovery_local_daemon_route_event_payload(event)?;
        }
        "daemon.endpoint.configured" => {
            validate_daemon_endpoint_configured_event_payload(event)?;
        }
        "daemon.run.identity_checked" => {
            validate_daemon_run_identity_event_payload(event)?;
        }
        "daemon.http.listening" => {
            validate_daemon_http_listening_event_payload(event)?;
        }
        "daemon.http.accept_failed" => {
            validate_daemon_http_accept_failed_event_payload(event)?;
        }
        "daemon.http.read_failed" => {
            validate_daemon_http_read_failed_event_payload(event)?;
        }
        "daemon.http.response_write_failed" => {
            validate_daemon_http_response_write_failed_event_payload(event)?;
        }
        "daemon.http.worker.rejected" => {
            validate_daemon_http_worker_rejected_event_payload(event)?;
        }
        "daemon.health.rendered" => {
            validate_local_status_event_payload(event)?;
            validate_daemon_lifecycle_event_payload(event)?;
        }
        "daemon.status_ui.rendered" => {
            validate_local_status_event_payload(event)?;
            require_bool_field(event, "headless")?;
        }
        "discovery.local.scanned" => {
            validate_discovery_local_event_payload(event)?;
        }
        "agent.created" => {
            validate_agent_created_event_payload(event)?;
        }
        "agent.inspected" => {
            validate_agent_inspected_event_payload(event)?;
        }
        "agent.model_updated" => {
            validate_agent_model_updated_event_payload(event)?;
        }
        event_type if event_type.starts_with("agent.resume.") => {
            validate_agent_resume_event_payload(event)?;
        }
        "agent.read" => {
            validate_agent_read_event_payload(event)?;
        }
        "inbox.read" => {
            validate_inbox_read_event_payload(event)?;
        }
        "peer.added" | "peer.updated" => {
            validate_peer_enrollment_event_payload(event)?;
        }
        "peer.sync.completed" => {
            validate_peer_sync_event_payload(event, true)?;
        }
        "peer.sync.failed" => {
            validate_peer_sync_event_payload(event, false)?;
        }
        "peer.sync.all.completed" => {
            let payload = validate_peer_sync_all_event_payload(event)?;
            require_bool_field_value(event, "ok", true)?;
            require_bool_field_value(event, "implemented", true)?;
            if payload.peers_requested == 0 {
                return Err(CamError::InvalidState(
                    "peer.sync.all.completed event must request at least one peer".to_string(),
                ));
            }
            if payload.peers_failed != 0 || payload.peers_synced != payload.peers_requested {
                return Err(CamError::InvalidState(
                    "peer.sync.all.completed event must sync every requested peer".to_string(),
                ));
            }
            if payload.peers_degraded > payload.peers_synced {
                return Err(CamError::InvalidState(
                    "peer.sync.all.completed event reports more degraded peers than synced peers"
                        .to_string(),
                ));
            }
            require_empty_array_field(event, "failed_error_kinds")?;
        }
        "peer.sync.all.noop" => {
            let payload = validate_peer_sync_all_event_payload(event)?;
            require_bool_field_value(event, "ok", true)?;
            require_bool_field_value(event, "implemented", false)?;
            require_bool_field_value(event, "remote_command_attempted", false)?;
            payload.require_zero_counters(event)?;
            require_string_field(event, "warning")?;
            require_empty_array_field(event, "failed_error_kinds")?;
        }
        "peer.sync.all.failed" => {
            let payload = validate_peer_sync_all_event_payload(event)?;
            require_bool_field_value(event, "ok", false)?;
            require_bool_field_value(event, "implemented", true)?;
            if payload.peers_requested == 0 {
                return Err(CamError::InvalidState(
                    "peer.sync.all.failed event must request at least one peer".to_string(),
                ));
            }
            if payload.peers_failed == 0 {
                return Err(CamError::InvalidState(
                    "peer.sync.all.failed event must report at least one failed peer".to_string(),
                ));
            }
            if payload.peers_degraded > payload.peers_synced {
                return Err(CamError::InvalidState(
                    "peer.sync.all.failed event reports more degraded peers than synced peers"
                        .to_string(),
                ));
            }
            require_string_field(event, "warning")?;
            require_error_kind_array(event, "failed_error_kinds")?;
        }
        "inventory.exported" => {
            require_number_field(event, "inventory_version")?;
            require_string_field(event, "node_name")?;
            require_bool_field_value(event, "mailbox_exported", false)?;
        }
        _ => {}
    }

    Ok(())
}

fn validate_inbox_read_event_payload(event: &StoredLogEvent) -> Result<(), CamError> {
    require_nullable_string_field(event, "filter")?;
    require_usize_field(event, "message_count")?;
    require_usize_field(event, "wait_seconds")?;
    require_bool_field(event, "timed_out")?;
    Ok(())
}

fn validate_api_request_started_event_payload(event: &StoredLogEvent) -> Result<(), CamError> {
    validate_api_request_common_event_payload(event)
}

fn validate_api_request_finished_event_payload(event: &StoredLogEvent) -> Result<(), CamError> {
    validate_api_request_common_event_payload(event)?;
    let status_code = require_usize_field(event, "status_code")?;
    if !(100..=599).contains(&status_code) {
        return Err(CamError::InvalidState(format!(
            "event `{}` has invalid HTTP status_code `{status_code}`",
            event.event_type
        )));
    }
    let ok = require_bool_field(event, "ok")?;
    if ok != (status_code < 400) {
        return Err(CamError::InvalidState(format!(
            "event `{}` ok must match status_code success range",
            event.event_type
        )));
    }
    require_usize_field(event, "duration_ms")?;
    require_string_field(event, "response_content_type")?;
    Ok(())
}

fn validate_daemon_http_listening_event_payload(event: &StoredLogEvent) -> Result<(), CamError> {
    let bind = require_string_field(event, "bind")?;
    if !is_loopback_bind(bind) {
        return Err(CamError::InvalidState(format!(
            "event `{}` bind `{bind}` is not loopback",
            event.event_type
        )));
    }
    let port = require_usize_field(event, "port")?;
    if !(1..=65535).contains(&port) {
        return Err(CamError::InvalidState(format!(
            "event `{}` port `{port}` is outside the TCP port range",
            event.event_type
        )));
    }
    require_bool_field(event, "headless")?;
    require_usize_field(event, "pid")?;
    Ok(())
}

fn validate_daemon_health_route_event_payload(event: &StoredLogEvent) -> Result<(), CamError> {
    require_bool_field(event, "daemon_running")?;
    let daemon_bind = require_string_field(event, "daemon_bind")?;
    if !is_loopback_bind(daemon_bind) {
        return Err(CamError::InvalidState(format!(
            "event `{}` daemon_bind `{daemon_bind}` is not loopback",
            event.event_type
        )));
    }
    require_daemon_port_field(event)?;
    let attempted = require_bool_field(event, "attempted")?;
    require_bool_field(event, "used")?;
    require_bool_field(event, "fallback_to_direct")?;
    optional_usize_field(event, "status_code")?;
    optional_error_kind_field(event)?;
    optional_nonempty_trimmed_string_field(event, "error")?;

    match event.event_type.as_str() {
        "daemon.health.daemon.used" => {
            require_bool_field_value(event, "attempted", true)?;
            require_bool_field_value(event, "used", true)?;
            require_bool_field_value(event, "fallback_to_direct", false)?;
            let status_code = require_usize_field(event, "status_code")?;
            if !(200..300).contains(&status_code) {
                return Err(CamError::InvalidState(format!(
                    "event `{}` used daemon with non-success status",
                    event.event_type
                )));
            }
            require_null_field(event, "error_kind")?;
            require_null_field(event, "error")?;
        }
        "daemon.health.daemon.fallback" => {
            require_bool_field_value(event, "used", false)?;
            require_bool_field_value(event, "fallback_to_direct", true)?;
            if attempted {
                require_error_kind_field(event)?;
            }
            require_string_field(event, "error")?;
        }
        "daemon.health.daemon.failed" => {
            require_bool_field_value(event, "used", false)?;
            require_bool_field_value(event, "fallback_to_direct", false)?;
            require_error_kind_field(event)?;
            require_string_field(event, "error")?;
        }
        _ => {}
    }
    Ok(())
}

fn validate_peer_sync_daemon_route_event_payload(event: &StoredLogEvent) -> Result<(), CamError> {
    let mode = require_string_field(event, "mode")?;
    if !matches!(mode, "single" | "all") {
        return Err(CamError::InvalidState(format!(
            "event `{}` has invalid mode `{mode}`",
            event.event_type
        )));
    }
    if mode == "single" {
        require_canonical_agent_field(event, "peer")?;
    } else {
        require_null_field(event, "peer")?;
    }
    require_bool_field(event, "daemon_running")?;
    let daemon_bind = require_string_field(event, "daemon_bind")?;
    if !is_loopback_bind(daemon_bind) {
        return Err(CamError::InvalidState(format!(
            "event `{}` daemon_bind `{daemon_bind}` is not loopback",
            event.event_type
        )));
    }
    require_daemon_port_field(event)?;
    let attempted = require_bool_field(event, "attempted")?;
    require_bool_field(event, "used")?;
    require_bool_field(event, "fallback_to_direct")?;
    optional_usize_field(event, "status_code")?;
    optional_error_kind_field(event)?;
    optional_nonempty_trimmed_string_field(event, "error")?;

    match event.event_type.as_str() {
        "peer.sync.daemon.used" => {
            require_bool_field_value(event, "attempted", true)?;
            require_bool_field_value(event, "used", true)?;
            require_bool_field_value(event, "fallback_to_direct", false)?;
            let status_code = require_usize_field(event, "status_code")?;
            if !(200..300).contains(&status_code) {
                return Err(CamError::InvalidState(format!(
                    "event `{}` used daemon with non-success status",
                    event.event_type
                )));
            }
            require_null_field(event, "error_kind")?;
            require_null_field(event, "error")?;
        }
        "peer.sync.daemon.fallback" => {
            require_bool_field_value(event, "used", false)?;
            require_bool_field_value(event, "fallback_to_direct", true)?;
            if attempted {
                require_error_kind_field(event)?;
            }
            require_string_field(event, "error")?;
        }
        "peer.sync.daemon.failed" => {
            require_bool_field_value(event, "used", false)?;
            require_bool_field_value(event, "fallback_to_direct", false)?;
            require_error_kind_field(event)?;
            require_string_field(event, "error")?;
        }
        _ => {}
    }
    Ok(())
}

fn validate_peer_add_daemon_route_event_payload(event: &StoredLogEvent) -> Result<(), CamError> {
    require_canonical_agent_field(event, "peer")?;
    require_bool_field(event, "daemon_running")?;
    let daemon_bind = require_string_field(event, "daemon_bind")?;
    if !is_loopback_bind(daemon_bind) {
        return Err(CamError::InvalidState(format!(
            "event `{}` daemon_bind `{daemon_bind}` is not loopback",
            event.event_type
        )));
    }
    require_daemon_port_field(event)?;
    require_bool_field_value(event, "ssh_present", true)?;
    require_bool_field(event, "key_present")?;
    require_bool_field(event, "remote_root_present")?;
    let attempted = require_bool_field(event, "attempted")?;
    require_bool_field(event, "used")?;
    require_bool_field(event, "fallback_to_direct")?;
    optional_usize_field(event, "status_code")?;
    optional_error_kind_field(event)?;
    optional_nonempty_trimmed_string_field(event, "error")?;

    match event.event_type.as_str() {
        "peer.add.daemon.used" => {
            require_bool_field_value(event, "attempted", true)?;
            require_bool_field_value(event, "used", true)?;
            require_bool_field_value(event, "fallback_to_direct", false)?;
            let status_code = require_usize_field(event, "status_code")?;
            if !(200..300).contains(&status_code) {
                return Err(CamError::InvalidState(format!(
                    "event `{}` used daemon with non-success status",
                    event.event_type
                )));
            }
            require_null_field(event, "error_kind")?;
            require_null_field(event, "error")?;
        }
        "peer.add.daemon.fallback" => {
            require_bool_field_value(event, "used", false)?;
            require_bool_field_value(event, "fallback_to_direct", true)?;
            if attempted {
                require_error_kind_field(event)?;
            }
            require_string_field(event, "error")?;
        }
        "peer.add.daemon.failed" => {
            require_bool_field_value(event, "used", false)?;
            require_bool_field_value(event, "fallback_to_direct", false)?;
            require_error_kind_field(event)?;
            require_string_field(event, "error")?;
        }
        _ => {}
    }
    Ok(())
}

fn validate_discovery_local_daemon_route_event_payload(
    event: &StoredLogEvent,
) -> Result<(), CamError> {
    require_bool_field(event, "daemon_running")?;
    let daemon_bind = require_string_field(event, "daemon_bind")?;
    if !is_loopback_bind(daemon_bind) {
        return Err(CamError::InvalidState(format!(
            "event `{}` daemon_bind `{daemon_bind}` is not loopback",
            event.event_type
        )));
    }
    require_daemon_port_field(event)?;
    require_bool_field(event, "codex_home_present")?;
    require_bool_field(event, "promote_approved")?;
    let attempted = require_bool_field(event, "attempted")?;
    require_bool_field(event, "used")?;
    require_bool_field(event, "fallback_to_direct")?;
    optional_usize_field(event, "status_code")?;
    optional_error_kind_field(event)?;
    optional_nonempty_trimmed_string_field(event, "error")?;

    match event.event_type.as_str() {
        "discovery.local.daemon.used" => {
            require_bool_field_value(event, "attempted", true)?;
            require_bool_field_value(event, "used", true)?;
            require_bool_field_value(event, "fallback_to_direct", false)?;
            let status_code = require_usize_field(event, "status_code")?;
            if !(200..300).contains(&status_code) {
                return Err(CamError::InvalidState(format!(
                    "event `{}` used daemon with non-success status",
                    event.event_type
                )));
            }
            require_null_field(event, "error_kind")?;
            require_null_field(event, "error")?;
        }
        "discovery.local.daemon.fallback" => {
            require_bool_field_value(event, "used", false)?;
            require_bool_field_value(event, "fallback_to_direct", true)?;
            if attempted {
                require_error_kind_field(event)?;
            }
            require_string_field(event, "error")?;
        }
        "discovery.local.daemon.failed" => {
            require_bool_field_value(event, "used", false)?;
            require_bool_field_value(event, "fallback_to_direct", false)?;
            require_error_kind_field(event)?;
            require_string_field(event, "error")?;
        }
        _ => {}
    }
    Ok(())
}

fn validate_daemon_endpoint_configured_event_payload(
    event: &StoredLogEvent,
) -> Result<(), CamError> {
    let bind = require_string_field(event, "bind")?;
    if !is_loopback_bind(bind) {
        return Err(CamError::InvalidState(format!(
            "event `{}` bind `{bind}` is not loopback",
            event.event_type
        )));
    }
    let port = require_usize_field(event, "port")?;
    if !(1..=65535).contains(&port) {
        return Err(CamError::InvalidState(format!(
            "event `{}` port `{port}` is outside the TCP port range",
            event.event_type
        )));
    }
    require_bool_field_value(event, "loopback_only", true)?;
    Ok(())
}

fn require_daemon_port_field(event: &StoredLogEvent) -> Result<usize, CamError> {
    let port = require_usize_field(event, "daemon_port")?;
    if !(1..=65535).contains(&port) {
        return Err(CamError::InvalidState(format!(
            "event `{}` daemon_port `{port}` is outside the TCP port range",
            event.event_type
        )));
    }
    Ok(port)
}

fn validate_daemon_http_accept_failed_event_payload(
    event: &StoredLogEvent,
) -> Result<(), CamError> {
    require_string_field_value(event, "error_kind", "io")?;
    require_string_field(event, "error")?;
    Ok(())
}

fn validate_daemon_http_read_failed_event_payload(event: &StoredLogEvent) -> Result<(), CamError> {
    require_string_field(event, "peer_addr")?;
    let phase = require_string_field(event, "phase")?;
    if !matches!(
        phase,
        "configure_read_timeout" | "read_request" | "persist_heartbeat" | "lock_state"
    ) {
        return Err(CamError::InvalidState(format!(
            "event `{}` has unknown read failure phase `{phase}`",
            event.event_type
        )));
    }
    require_string_field(event, "error_kind")?;
    require_string_field(event, "error")?;
    Ok(())
}

fn validate_daemon_http_response_write_failed_event_payload(
    event: &StoredLogEvent,
) -> Result<(), CamError> {
    require_string_field(event, "peer_addr")?;
    let status_code = require_usize_field(event, "status_code")?;
    if !(100..=599).contains(&status_code) {
        return Err(CamError::InvalidState(format!(
            "event `{}` has invalid HTTP status_code `{status_code}`",
            event.event_type
        )));
    }
    require_string_field(event, "content_type")?;
    require_string_field_value(event, "error_kind", "io")?;
    require_string_field(event, "error")?;
    Ok(())
}

fn validate_daemon_http_worker_rejected_event_payload(
    event: &StoredLogEvent,
) -> Result<(), CamError> {
    require_string_field(event, "peer_addr")?;
    require_bool_field_value(event, "ok", false)?;
    let status_code = require_usize_field(event, "status_code")?;
    if status_code != 503 {
        return Err(CamError::InvalidState(format!(
            "event `{}` must use HTTP 503 for worker rejection",
            event.event_type
        )));
    }
    let active_workers = require_usize_field(event, "active_workers")?;
    let max_workers = require_usize_field(event, "max_workers")?;
    if max_workers != services::MAX_LOOPBACK_WORKERS {
        return Err(CamError::InvalidState(format!(
            "event `{}` max_workers must match the loopback worker admission limit",
            event.event_type
        )));
    }
    let error_kind = require_string_field(event, "error_kind")?;
    if error_kind == "worker_limit_reached" && active_workers < max_workers {
        return Err(CamError::InvalidState(format!(
            "event `{}` rejected below worker limit",
            event.event_type
        )));
    }
    if !matches!(error_kind, "worker_limit_reached" | "worker_spawn_failed") {
        return Err(CamError::InvalidState(format!(
            "event `{}` has unknown worker rejection error_kind `{error_kind}`",
            event.event_type
        )));
    }
    require_string_field(event, "error")?;
    Ok(())
}

fn validate_doctor_providers_checked_event_payload(event: &StoredLogEvent) -> Result<(), CamError> {
    require_string_field(event, "codex_program")?;
    require_bool_field_value(event, "codex_cli_probe_attempted", true)?;
    require_bool_field(event, "codex_cli_available")?;
    require_nullable_string_field(event, "codex_cli_version")?;
    require_nullable_string_field(event, "codex_cli_error")?;
    require_bool_field_value(event, "codex_auth_probe_attempted", true)?;
    require_nullable_bool_field(event, "codex_auth_available")?;
    require_nullable_string_field(event, "codex_auth_status")?;
    require_nullable_string_field(event, "codex_auth_error")?;
    require_bool_field(event, "codex_app_server_stdio_probe_attempted")?;
    require_bool_field(event, "codex_app_server_initialized")?;
    require_nullable_string_field(event, "codex_app_server_error")?;
    require_bool_field_value(event, "agy_delivery_primitive_verified", false)?;
    Ok(())
}

fn validate_api_request_common_event_payload(event: &StoredLogEvent) -> Result<(), CamError> {
    let request_id = require_string_field(event, "request_id")?;
    Uuid::parse_str(request_id).map_err(|_| {
        CamError::InvalidState(format!(
            "event `{}` request_id must be a UUID",
            event.event_type
        ))
    })?;
    let method = require_string_field(event, "method")?;
    if !matches!(method, "GET" | "POST") {
        return Err(CamError::InvalidState(format!(
            "event `{}` has unsupported local API method `{method}`",
            event.event_type
        )));
    }
    let path = require_string_field(event, "path")?;
    if !path.starts_with('/') || path.contains('?') {
        return Err(CamError::InvalidState(format!(
            "event `{}` path must be sanitized and must not include query values",
            event.event_type
        )));
    }
    let route_template = require_string_field(event, "route_template")?;
    if !route_template.starts_with('/') && route_template != "unknown" {
        return Err(CamError::InvalidState(format!(
            "event `{}` route_template must be sanitized",
            event.event_type
        )));
    }
    let route_family = require_string_field(event, "route_family")?;
    if !matches!(
        route_family,
        "agents"
            | "messages"
            | "inbox"
            | "logs"
            | "discovery"
            | "peers"
            | "inventory"
            | "shutdown"
            | "unknown"
    ) {
        return Err(CamError::InvalidState(format!(
            "event `{}` has unknown route_family `{route_family}`",
            event.event_type
        )));
    }
    require_string_array_field(event, "query_keys")?;
    require_usize_field(event, "content_length")?;
    require_bool_field_value(event, "peer_addr_loopback", true)
}

fn validate_peer_sync_event_payload(
    event: &StoredLogEvent,
    expected_ok: bool,
) -> Result<(), CamError> {
    require_string_field(event, "peer")?;
    require_string_field(event, "transport")?;
    require_nullable_string_field(event, "ssh_target")?;
    require_bool_field_value(event, "ok", expected_ok)?;
    require_bool_field_value(event, "implemented", true)?;
    require_bool_field(event, "remote_command_attempted")?;
    require_usize_field(event, "agents_imported")?;
    require_usize_field(event, "remote_agents_seen")?;
    require_usize_field(event, "mirrored_agents_added")?;
    require_usize_field(event, "mirrored_agents_updated")?;
    require_usize_field(event, "mirrored_agents_skipped")?;
    require_usize_field(event, "mirrored_agents_stale")?;
    require_usize_field(event, "collision_count")?;
    require_bool_field(event, "degraded")?;
    if expected_ok {
        require_null_field(event, "error_kind")?;
        require_null_field(event, "error")?;
    } else {
        require_error_kind_field(event)?;
        require_string_field(event, "error")?;
    }
    Ok(())
}

struct PeerSyncAllPayload {
    peers_requested: usize,
    peers_synced: usize,
    peers_failed: usize,
    peers_degraded: usize,
    agents_imported: usize,
    remote_agents_seen: usize,
    mirrored_agents_added: usize,
    mirrored_agents_updated: usize,
    mirrored_agents_skipped: usize,
    mirrored_agents_stale: usize,
    collision_count: usize,
}

impl PeerSyncAllPayload {
    fn require_zero_counters(&self, event: &StoredLogEvent) -> Result<(), CamError> {
        if self.peers_requested != 0
            || self.peers_synced != 0
            || self.peers_failed != 0
            || self.peers_degraded != 0
            || self.agents_imported != 0
            || self.remote_agents_seen != 0
            || self.mirrored_agents_added != 0
            || self.mirrored_agents_updated != 0
            || self.mirrored_agents_skipped != 0
            || self.mirrored_agents_stale != 0
            || self.collision_count != 0
        {
            return Err(CamError::InvalidState(format!(
                "event `{}` no-op peer sync must report all counters as zero",
                event.event_type
            )));
        }
        Ok(())
    }
}

fn validate_peer_sync_all_event_payload(
    event: &StoredLogEvent,
) -> Result<PeerSyncAllPayload, CamError> {
    require_string_field_value(event, "mode", "all")?;
    require_bool_field(event, "ok")?;
    require_bool_field(event, "implemented")?;
    let peers_requested = require_usize_field(event, "peers_requested")?;
    let peers_synced = require_usize_field(event, "peers_synced")?;
    let peers_failed = require_usize_field(event, "peers_failed")?;
    let peers_degraded = require_usize_field(event, "peers_degraded")?;
    let agents_imported = require_usize_field(event, "agents_imported")?;
    let remote_agents_seen = require_usize_field(event, "remote_agents_seen")?;
    let mirrored_agents_added = require_usize_field(event, "mirrored_agents_added")?;
    let mirrored_agents_updated = require_usize_field(event, "mirrored_agents_updated")?;
    let mirrored_agents_skipped = require_usize_field(event, "mirrored_agents_skipped")?;
    let mirrored_agents_stale = require_usize_field(event, "mirrored_agents_stale")?;
    let collision_count = require_usize_field(event, "collision_count")?;
    require_bool_field(event, "remote_command_attempted")?;
    require_nullable_string_field(event, "warning")?;
    if peers_synced + peers_failed != peers_requested {
        return Err(CamError::InvalidState(format!(
            "event `{}` peer sync counters must match requested peers",
            event.event_type
        )));
    }
    Ok(PeerSyncAllPayload {
        peers_requested,
        peers_synced,
        peers_failed,
        peers_degraded,
        agents_imported,
        remote_agents_seen,
        mirrored_agents_added,
        mirrored_agents_updated,
        mirrored_agents_skipped,
        mirrored_agents_stale,
        collision_count,
    })
}

fn validate_peer_enrollment_event_payload(event: &StoredLogEvent) -> Result<(), CamError> {
    require_string_field(event, "peer")?;
    require_string_field_value(event, "transport", "ssh")?;
    require_nullable_string_field(event, "ssh_target")?;
    require_bool_field(event, "key_path_present")?;
    require_nullable_string_field(event, "remote_root")?;
    require_bool_field_value(event, "network_probe_attempted", false)?;
    Ok(())
}

fn validate_agent_read_event_payload(event: &StoredLogEvent) -> Result<(), CamError> {
    require_string_field(event, "agent")?;
    require_string_field(event, "evidence_scope")?;
    require_string_field(event, "snapshot_source")?;
    require_string_field(event, "transcript_source")?;
    require_usize_field(event, "mailbox_messages")?;
    require_bool_field(event, "mailbox_evidence_available")?;
    require_usize_field(event, "mailbox_message_count")?;
    require_usize_field(event, "mailbox_messages_total")?;
    let checked = require_bool_field(event, "provider_transcript_checked")?;
    require_bool_field(event, "latest_only")?;
    require_bool_field(event, "include_turns")?;
    let requested_turn_limit = optional_usize_field(event, "requested_turn_limit")?;
    validate_optional_wait_seconds(event, "requested_wait_seconds")?;
    if let Some(limit) = requested_turn_limit
        && limit == 0
    {
        return Err(CamError::InvalidState(format!(
            "event `{}` requested_turn_limit must be greater than zero when present",
            event.event_type
        )));
    }
    if let Some(limit) = requested_turn_limit
        && limit > services::MAX_AGENT_READ_TURNS
    {
        return Err(CamError::InvalidState(format!(
            "event `{}` requested_turn_limit exceeds MAX_AGENT_READ_TURNS",
            event.event_type
        )));
    }
    let available = require_bool_field(event, "provider_transcript_available")?;
    let status = require_string_field(event, "provider_transcript_status")?;
    require_nullable_string_field(event, "provider_transcript_error_kind")?;
    require_nullable_string_field(event, "provider_transcript_latest_turn_id")?;

    if !matches!(
        status,
        "provider_transcript_unavailable"
            | "not_applicable"
            | "available"
            | "unavailable"
            | "error"
    ) {
        return Err(CamError::InvalidState(format!(
            "event `{}` has unknown provider_transcript_status `{status}`",
            event.event_type
        )));
    }
    if available && !checked {
        return Err(CamError::InvalidState(format!(
            "event `{}` cannot report provider transcript availability without provider check",
            event.event_type
        )));
    }
    if available && status != "available" {
        return Err(CamError::InvalidState(format!(
            "event `{}` provider transcript availability contradicts status `{status}`",
            event.event_type
        )));
    }
    if status == "available" && !available {
        return Err(CamError::InvalidState(format!(
            "event `{}` provider transcript status available requires availability",
            event.event_type
        )));
    }
    if status == "error"
        && event
            .fields
            .get("provider_transcript_error_kind")
            .is_none_or(serde_json::Value::is_null)
    {
        return Err(CamError::InvalidState(format!(
            "event `{}` provider transcript error status requires error kind",
            event.event_type
        )));
    }
    Ok(())
}

fn validate_agent_created_event_payload(event: &StoredLogEvent) -> Result<(), CamError> {
    require_canonical_agent_field(event, "agent")?;
    validate_agent_kind_label(event, require_string_field(event, "kind")?)?;
    require_bool_field(event, "thread_id_present")?;
    require_bool_field(event, "cwd_present")?;
    Ok(())
}

fn validate_agent_inspected_event_payload(event: &StoredLogEvent) -> Result<(), CamError> {
    require_canonical_agent_field(event, "agent")?;
    validate_agent_kind_label(event, require_string_field(event, "kind")?)?;
    validate_agent_status_label(event, require_string_field(event, "status")?)?;
    validate_agent_route_label(event, require_string_field(event, "route")?)?;
    require_bool_field(event, "thread_id_present")?;
    require_bool_field(event, "active_turn_id_present")?;
    require_bool_field(event, "last_turn_id_present")?;
    require_bool_field(event, "model_present")?;
    require_bool_field(event, "model_provider_present")?;
    Ok(())
}

fn validate_agent_model_updated_event_payload(event: &StoredLogEvent) -> Result<(), CamError> {
    require_canonical_agent_field(event, "agent")?;
    require_nullable_string_field(event, "thread_id")?;
    optional_nonempty_trimmed_string_field(event, "model")?;
    optional_nonempty_trimmed_string_field(event, "model_provider")?;
    optional_nonempty_trimmed_string_field(event, "service_tier")?;
    validate_optional_effort_field(event, "effort")?;
    Ok(())
}

fn validate_daemon_lifecycle_event_payload(event: &StoredLogEvent) -> Result<(), CamError> {
    require_string_field_value(event, "state_source", services::DAEMON_STATE_SOURCE)?;
    let live_probe_attempted = require_bool_field(event, "live_probe_attempted")?;
    let process_supervisor_wired = require_bool_field(event, "process_supervisor_wired")?;
    let instance_id_present = require_bool_field(event, "instance_id_present")?;
    let identity_nonce_ref_present = require_bool_field(event, "identity_nonce_ref_present")?;
    let instance_id_matched = require_bool_field(event, "instance_id_matched")?;
    let node_name_matched = require_bool_field(event, "node_name_matched")?;
    let version_matched = require_bool_field(event, "version_matched")?;
    let process_identity_verified = require_bool_field(event, "process_identity_verified")?;
    let implemented = require_bool_field(event, "implemented")?;
    let running = require_bool_field(event, "running")?;
    let process_exists = optional_bool_field(event, "process_exists")?;
    require_nullable_string_field(event, "identity_mismatch")?;
    require_nullable_string_field(event, "live_probe_error")?;

    if !matches!(
        event.event_type.as_str(),
        "daemon.status.checked" | "daemon.health.rendered"
    ) && live_probe_attempted
    {
        return Err(CamError::InvalidState(format!(
            "event `{}` cannot report live_probe_attempted before supervisor actions are wired",
            event.event_type
        )));
    }
    if !live_probe_attempted
        && event
            .fields
            .get("process_exists")
            .is_none_or(|value| !value.is_null())
    {
        return Err(CamError::InvalidState(format!(
            "event `{}` cannot report process_exists without live_probe_attempted",
            event.event_type
        )));
    }
    if instance_id_matched && !instance_id_present {
        return Err(CamError::InvalidState(format!(
            "event `{}` cannot report instance_id_matched without instance_id_present",
            event.event_type
        )));
    }
    if process_identity_verified && !identity_nonce_ref_present {
        return Err(CamError::InvalidState(format!(
            "event `{}` cannot report process_identity_verified without identity_nonce_ref_present",
            event.event_type
        )));
    }
    if instance_id_matched && process_exists != Some(true) {
        return Err(CamError::InvalidState(format!(
            "event `{}` cannot report instance_id_matched without process_exists proof",
            event.event_type
        )));
    }
    if process_identity_verified
        != (instance_id_matched
            && identity_nonce_ref_present
            && node_name_matched
            && version_matched
            && process_exists == Some(true))
    {
        return Err(CamError::InvalidState(format!(
            "event `{}` process_identity_verified must match nonce ref, instance, node, version, and process proof",
            event.event_type
        )));
    }
    if running && !process_identity_verified {
        return Err(CamError::InvalidState(format!(
            "event `{}` cannot report running without process_identity_verified",
            event.event_type
        )));
    }
    if !process_supervisor_wired {
        if implemented {
            return Err(CamError::InvalidState(format!(
                "event `{}` cannot claim implemented daemon supervisor without a wired process supervisor",
                event.event_type
            )));
        }
        if event
            .fields
            .get("pid")
            .is_some_and(serde_json::Value::is_null)
        {
            return Ok(());
        }
        if event.event_type == "daemon.status.checked" && live_probe_attempted {
            require_number_field(event, "pid")?;
            return Ok(());
        }
        return Err(CamError::InvalidState(format!(
            "event `{}` field `pid` must be null unless status is reporting live probe evidence",
            event.event_type
        )));
    }
    Ok(())
}

fn validate_daemon_run_identity_event_payload(event: &StoredLogEvent) -> Result<(), CamError> {
    require_string_field_value(event, "state_source", services::DAEMON_STATE_SOURCE)?;
    require_string_field_value(event, "surface", services::DAEMON_RUN_IDENTITY_SURFACE)?;
    let ok = require_bool_field(event, "ok")?;
    require_bool_field_value(event, "would_run", false)?;
    require_bool_field_value(event, "process_supervisor_wired", false)?;
    require_number_field(event, "pid")?;
    let identity_challenge_present = require_bool_field(event, "identity_challenge_present")?;
    require_string_field_value(
        event,
        "nonce_proof_algorithm",
        services::DAEMON_IDENTITY_PROOF_ALGORITHM,
    )?;
    let nonce_proof_present = require_bool_field(event, "nonce_proof_present")?;
    require_string_field(event, "node_name")?;
    require_string_field(event, "version")?;
    let instance_id_present = require_bool_field(event, "instance_id_present")?;
    let identity_nonce_ref_present = require_bool_field(event, "identity_nonce_ref_present")?;
    let identity_nonce_available = require_bool_field(event, "identity_nonce_available")?;
    require_bool_field_value(event, "nonce_value_exposed", false)?;
    let identity_ready = require_bool_field(event, "identity_ready")?;
    require_nullable_string_field(event, "identity_error")?;

    if event.fields.get("nonce").is_some()
        || event.fields.get("identity_nonce").is_some()
        || event.fields.get("nonce_proof").is_some()
    {
        return Err(CamError::InvalidState(format!(
            "event `{}` must not expose daemon identity nonce or proof value",
            event.event_type
        )));
    }
    let expected_ready = instance_id_present
        && identity_nonce_ref_present
        && identity_nonce_available
        && identity_challenge_present
        && nonce_proof_present;
    if identity_ready != expected_ready {
        return Err(CamError::InvalidState(format!(
            "event `{}` identity_ready must match identity proof inputs",
            event.event_type
        )));
    }
    if ok != identity_ready {
        return Err(CamError::InvalidState(format!(
            "event `{}` ok must match identity_ready",
            event.event_type
        )));
    }
    if !ok
        && event
            .fields
            .get("identity_error")
            .is_none_or(serde_json::Value::is_null)
    {
        return Err(CamError::InvalidState(format!(
            "event `{}` failed identity check must include identity_error",
            event.event_type
        )));
    }
    Ok(())
}

fn validate_local_status_event_payload(event: &StoredLogEvent) -> Result<(), CamError> {
    require_string_field_value(event, "state_source", services::DAEMON_STATE_SOURCE)?;
    require_number_field(event, "status_code")?;
    require_string_field(event, "content_type")?;
    require_bool_field_value(event, "public_network_enabled", false)?;
    require_bool_field_value(event, "mutation_routes_enabled", false)?;
    require_bool_field(event, "loopback_only")?;
    Ok(())
}

fn validate_discovery_local_event_payload(event: &StoredLogEvent) -> Result<(), CamError> {
    require_string_field_value(event, "scanner", "codex_local_sources")?;
    require_nullable_string_field(event, "source_path")?;
    let rows_discovered = require_usize_field(event, "rows_discovered")?;
    let approved = require_usize_field(event, "approved")?;
    let candidate = require_usize_field(event, "candidate")?;
    let quarantined = require_usize_field(event, "quarantined")?;
    let rejected = require_usize_field(event, "rejected")?;
    if approved + candidate + quarantined + rejected != rows_discovered {
        return Err(CamError::InvalidState(format!(
            "event `{}` discovery disposition counters do not match discovered rows",
            event.event_type
        )));
    }
    let promoted = require_usize_field(event, "promoted")?;
    let skipped_existing_thread = require_usize_field(event, "skipped_existing_thread")?;
    let skipped_name_collision = require_usize_field(event, "skipped_name_collision")?;
    let skipped_not_approved = require_usize_field(event, "skipped_not_approved")?;
    let skipped_promotion_not_requested =
        require_usize_field(event, "skipped_promotion_not_requested")?;
    let skipped_invalid_after_reclassify =
        require_usize_field(event, "skipped_invalid_after_reclassify")?;
    require_nullable_string_field(event, "warning")?;
    let decisions = event
        .fields
        .get("promotion_decisions")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            CamError::InvalidState(format!(
                "event `{}` is missing promotion_decisions array",
                event.event_type
            ))
        })?;
    if decisions.len() != rows_discovered {
        return Err(CamError::InvalidState(format!(
            "event `{}` promotion decision count does not match discovered rows",
            event.event_type
        )));
    }

    let mut actual_promoted = 0;
    let mut actual_existing_thread = 0;
    let mut actual_name_collision = 0;
    let mut actual_not_approved = 0;
    let mut actual_promotion_not_requested = 0;
    let mut actual_invalid = 0;
    for (expected_row_index, decision) in decisions.iter().enumerate() {
        let Some(decision) = decision.as_object() else {
            return Err(CamError::InvalidState(format!(
                "event `{}` promotion decision must be an object",
                event.event_type
            )));
        };
        let row_index = decision
            .get("row_index")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| {
                CamError::InvalidState(format!(
                    "event `{}` promotion decision is missing row_index",
                    event.event_type
                ))
            })?;
        if row_index as usize != expected_row_index {
            return Err(CamError::InvalidState(format!(
                "event `{}` promotion decision row_index does not match row order",
                event.event_type
            )));
        }
        let Some(kind) = decision.get("decision").and_then(serde_json::Value::as_str) else {
            return Err(CamError::InvalidState(format!(
                "event `{}` promotion decision is missing decision kind",
                event.event_type
            )));
        };
        for field in ["source", "route", "thread_source", "classified_disposition"] {
            if decision
                .get(field)
                .and_then(serde_json::Value::as_str)
                .is_none_or(str::is_empty)
            {
                return Err(CamError::InvalidState(format!(
                    "event `{}` promotion decision is missing {field}",
                    event.event_type
                )));
            }
        }
        if decision
            .get("title")
            .and_then(serde_json::Value::as_str)
            .is_none_or(|title| title.trim().is_empty())
        {
            return Err(CamError::InvalidState(format!(
                "event `{}` promotion decision is missing title",
                event.event_type
            )));
        }
        if decision
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .is_none_or(|reason| reason.trim().is_empty())
        {
            return Err(CamError::InvalidState(format!(
                "event `{}` promotion decision is missing reason",
                event.event_type
            )));
        }
        match kind {
            "promoted" => {
                actual_promoted += 1;
                require_promotion_decision_string(decision, event, "thread_id")?;
                require_promotion_decision_string(decision, event, "generated_name")?;
                require_promotion_decision_value(decision, event, "route", "local")?;
                require_promotion_decision_value(decision, event, "thread_source", "codex")?;
            }
            "skipped_existing_thread" => {
                actual_existing_thread += 1;
                require_promotion_decision_string(decision, event, "thread_id")?;
                require_promotion_decision_string(decision, event, "existing_agent")?;
            }
            "skipped_name_collision" => {
                actual_name_collision += 1;
                require_promotion_decision_string(decision, event, "generated_name")?;
                require_promotion_decision_string(decision, event, "existing_agent")?;
            }
            "skipped_not_approved" => actual_not_approved += 1,
            "skipped_promotion_not_requested" => {
                actual_promotion_not_requested += 1;
                require_promotion_decision_string(decision, event, "generated_name")?;
                require_promotion_decision_value(
                    decision,
                    event,
                    "classified_disposition",
                    "approved",
                )?;
            }
            "skipped_invalid_after_reclassify" => actual_invalid += 1,
            _ => {
                return Err(CamError::InvalidState(format!(
                    "event `{}` has unknown promotion decision `{kind}`",
                    event.event_type
                )));
            }
        }
    }

    if promoted != actual_promoted
        || skipped_existing_thread != actual_existing_thread
        || skipped_name_collision != actual_name_collision
        || skipped_not_approved != actual_not_approved
        || skipped_promotion_not_requested != actual_promotion_not_requested
        || skipped_invalid_after_reclassify != actual_invalid
    {
        return Err(CamError::InvalidState(format!(
            "event `{}` promotion decision counters do not match decision list",
            event.event_type
        )));
    }
    Ok(())
}

fn require_promotion_decision_string(
    decision: &serde_json::Map<String, serde_json::Value>,
    event: &StoredLogEvent,
    field: &str,
) -> Result<(), CamError> {
    if decision
        .get(field)
        .and_then(serde_json::Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Ok(());
    }
    Err(CamError::InvalidState(format!(
        "event `{}` promotion decision is missing {field} proof",
        event.event_type
    )))
}

fn require_promotion_decision_value(
    decision: &serde_json::Map<String, serde_json::Value>,
    event: &StoredLogEvent,
    field: &str,
    expected: &str,
) -> Result<(), CamError> {
    let actual = decision
        .get(field)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if actual == expected {
        return Ok(());
    }
    Err(CamError::InvalidState(format!(
        "event `{}` promotion decision field `{field}` expected `{expected}` but found `{actual}`",
        event.event_type
    )))
}

fn validate_agent_resume_event_payload(event: &StoredLogEvent) -> Result<(), CamError> {
    require_string_field(event, "agent")?;
    require_string_field(event, "kind")?;
    require_string_field(event, "route")?;
    require_bool_field(event, "thread_id_present")?;
    let ok = require_bool_field(event, "ok")?;
    require_bool_field(event, "implemented")?;
    let ready = require_bool_field(event, "ready")?;
    let provider_checked = require_bool_field(event, "provider_checked")?;
    let provider_resumed = require_bool_field(event, "provider_resumed")?;
    let readiness_ready = optional_bool_field(event, "readiness_ready")?;
    require_nullable_string_field(event, "readiness_error")?;
    let resume_attempted = require_bool_field(event, "resume_attempted")?;
    require_nullable_string_field(event, "active_turn_id")?;
    require_nullable_string_field(event, "last_turn_id")?;
    require_bool_field_value(event, "transcript_mutated", false)?;
    require_string_field(event, "reason")?;
    require_nullable_string_field(event, "error")?;
    require_nullable_string_field(event, "warning")?;

    if provider_resumed && !resume_attempted {
        return Err(CamError::InvalidState(format!(
            "event `{}` cannot report provider_resumed without resume_attempted",
            event.event_type
        )));
    }
    if provider_resumed && !provider_checked {
        return Err(CamError::InvalidState(format!(
            "event `{}` cannot report provider_resumed without provider_checked",
            event.event_type
        )));
    }
    if readiness_ready == Some(true) && !provider_checked {
        return Err(CamError::InvalidState(format!(
            "event `{}` cannot report readiness_ready without provider_checked",
            event.event_type
        )));
    }
    if ok && !ready {
        return Err(CamError::InvalidState(format!(
            "event `{}` cannot be ok without ready",
            event.event_type
        )));
    }
    if ready
        && event
            .fields
            .get("active_turn_id")
            .is_none_or(serde_json::Value::is_null)
        && event
            .fields
            .get("last_turn_id")
            .is_none_or(serde_json::Value::is_null)
    {
        return Err(CamError::InvalidState(format!(
            "event `{}` ready resume must include active_turn_id or last_turn_id proof",
            event.event_type
        )));
    }
    if !ok
        && event
            .fields
            .get("error")
            .is_none_or(serde_json::Value::is_null)
    {
        return Err(CamError::InvalidState(format!(
            "event `{}` blocked/rejected resume must include error",
            event.event_type
        )));
    }
    if !resume_attempted
        && event.event_type == "agent.resume.ready"
        && readiness_ready != Some(true)
        && event.fields.get("kind").and_then(serde_json::Value::as_str) != Some("virtual_inbox")
    {
        return Err(CamError::InvalidState(format!(
            "event `{}` ready provider resume without resume_attempted requires readiness_ready proof",
            event.event_type
        )));
    }
    Ok(())
}

fn validate_message_event_payload(
    state: &AppState,
    event: &StoredLogEvent,
) -> Result<(), CamError> {
    let expected_delivery = event
        .event_type
        .strip_prefix("message.")
        .unwrap_or_default();
    if !matches!(
        expected_delivery,
        "received" | "queued" | "failed" | "delivered" | "steered"
    ) {
        return Ok(());
    }

    let message_id = require_string_field(event, "message_id")?;
    let parsed_id = Uuid::parse_str(message_id).map_err(|_| {
        CamError::InvalidState(format!(
            "event `{}` has invalid message_id",
            event.event_type
        ))
    })?;
    if parsed_id.to_string() != message_id {
        return Err(CamError::InvalidState(format!(
            "event `{}` has non-canonical message_id",
            event.event_type
        )));
    }

    let target_agent = require_string_field(event, "target_agent")?;
    if !is_canonical_name(target_agent) {
        return Err(CamError::InvalidState(format!(
            "event `{}` has invalid target_agent",
            event.event_type
        )));
    }

    let delivery = require_string_field(event, "delivery")?;
    if delivery != expected_delivery {
        return Err(CamError::InvalidState(format!(
            "event `{}` delivery `{delivery}` does not match event suffix `{expected_delivery}`",
            event.event_type
        )));
    }
    let strict = require_bool_field(event, "strict")?;
    if expected_delivery == "queued" && strict {
        return Err(CamError::InvalidState(format!(
            "event `{}` cannot queue while strict delivery is enabled",
            event.event_type
        )));
    }
    require_string_field(event, "reason")?;
    let thread_id_present = require_bool_field(event, "thread_id_present")?;
    let turn_id_present = require_bool_field(event, "turn_id_present")?;
    validate_optional_message_event_proof_flag(event, "thread_id", thread_id_present)?;
    validate_optional_message_event_proof_flag(event, "turn_id", turn_id_present)?;
    if matches!(expected_delivery, "delivered" | "steered") {
        let target = state.find_agent(target_agent).ok_or_else(|| {
            CamError::InvalidState(format!(
                "event `{}` direct delivery target `{target_agent}` is not a registered agent",
                event.event_type
            ))
        })?;
        validate_direct_message_event_target_proof(event, target)?;
        if !thread_id_present {
            return Err(CamError::InvalidState(format!(
                "event `{}` direct delivery is missing thread_id proof",
                event.event_type
            )));
        }
        if !turn_id_present {
            return Err(CamError::InvalidState(format!(
                "event `{}` direct delivery is missing turn_id proof",
                event.event_type
            )));
        }
        validate_delivery_proof_token("event", event, "thread_id")?;
        validate_delivery_proof_token("event", event, "turn_id")?;
        validate_remote_mirror_direct_event_payload(event, target_agent)?;
    }
    validate_message_event_plan(event, delivery)?;
    validate_message_event_error_classification(event, delivery, strict)?;
    Ok(())
}

fn validate_send_daemon_route_event_payload(event: &StoredLogEvent) -> Result<(), CamError> {
    require_canonical_agent_field(event, "target_agent")?;
    require_bool_field(event, "daemon_running")?;
    let daemon_bind = require_string_field(event, "daemon_bind")?;
    if !is_loopback_bind(daemon_bind) {
        return Err(CamError::InvalidState(format!(
            "event `{}` daemon_bind `{daemon_bind}` is not loopback",
            event.event_type
        )));
    }
    require_daemon_port_field(event)?;
    let attempted = require_bool_field(event, "attempted")?;
    require_bool_field(event, "used")?;
    require_bool_field(event, "fallback_to_direct")?;
    optional_usize_field(event, "status_code")?;
    optional_error_kind_field(event)?;
    optional_nonempty_trimmed_string_field(event, "error")?;

    match event.event_type.as_str() {
        "send.daemon.used" => {
            require_bool_field_value(event, "attempted", true)?;
            require_bool_field_value(event, "used", true)?;
            require_bool_field_value(event, "fallback_to_direct", false)?;
            let status_code = require_usize_field(event, "status_code")?;
            if !(200..300).contains(&status_code) {
                return Err(CamError::InvalidState(format!(
                    "event `{}` used daemon with non-success status",
                    event.event_type
                )));
            }
            require_null_field(event, "error_kind")?;
            require_null_field(event, "error")?;
        }
        "send.daemon.fallback" => {
            require_bool_field_value(event, "used", false)?;
            require_bool_field_value(event, "fallback_to_direct", true)?;
            if attempted {
                require_error_kind_field(event)?;
            }
            require_string_field(event, "error")?;
        }
        "send.daemon.failed" => {
            require_bool_field_value(event, "used", false)?;
            require_bool_field_value(event, "fallback_to_direct", false)?;
            require_error_kind_field(event)?;
            require_string_field(event, "error")?;
        }
        _ => {}
    }
    Ok(())
}

fn validate_resume_daemon_route_event_payload(event: &StoredLogEvent) -> Result<(), CamError> {
    require_canonical_agent_field(event, "agent")?;
    require_bool_field(event, "daemon_running")?;
    let daemon_bind = require_string_field(event, "daemon_bind")?;
    if !is_loopback_bind(daemon_bind) {
        return Err(CamError::InvalidState(format!(
            "event `{}` daemon_bind `{daemon_bind}` is not loopback",
            event.event_type
        )));
    }
    require_daemon_port_field(event)?;
    let attempted = require_bool_field(event, "attempted")?;
    require_bool_field(event, "used")?;
    require_bool_field(event, "fallback_to_direct")?;
    optional_usize_field(event, "status_code")?;
    optional_error_kind_field(event)?;
    optional_nonempty_trimmed_string_field(event, "error")?;

    match event.event_type.as_str() {
        "resume.daemon.used" => {
            require_bool_field_value(event, "attempted", true)?;
            require_bool_field_value(event, "used", true)?;
            require_bool_field_value(event, "fallback_to_direct", false)?;
            let status_code = require_usize_field(event, "status_code")?;
            if !(200..300).contains(&status_code) {
                return Err(CamError::InvalidState(format!(
                    "event `{}` used daemon with non-success status",
                    event.event_type
                )));
            }
            require_null_field(event, "error_kind")?;
            require_null_field(event, "error")?;
        }
        "resume.daemon.fallback" => {
            require_bool_field_value(event, "used", false)?;
            require_bool_field_value(event, "fallback_to_direct", true)?;
            if attempted {
                require_error_kind_field(event)?;
            }
            require_string_field(event, "error")?;
        }
        "resume.daemon.failed" => {
            require_bool_field_value(event, "used", false)?;
            require_bool_field_value(event, "fallback_to_direct", false)?;
            require_error_kind_field(event)?;
            require_string_field(event, "error")?;
        }
        _ => {}
    }
    Ok(())
}

fn validate_agent_create_daemon_route_event_payload(
    event: &StoredLogEvent,
) -> Result<(), CamError> {
    require_canonical_agent_field(event, "agent")?;
    require_bool_field(event, "daemon_running")?;
    let daemon_bind = require_string_field(event, "daemon_bind")?;
    if !is_loopback_bind(daemon_bind) {
        return Err(CamError::InvalidState(format!(
            "event `{}` daemon_bind `{daemon_bind}` is not loopback",
            event.event_type
        )));
    }
    require_daemon_port_field(event)?;
    let source = require_string_field(event, "source")?;
    if !matches!(source, "codex" | "agy" | "mailbox") {
        return Err(CamError::InvalidState(format!(
            "event `{}` has invalid source `{source}`",
            event.event_type
        )));
    }
    require_bool_field(event, "cwd_present")?;
    require_bool_field(event, "thread_id_present")?;
    let attempted = require_bool_field(event, "attempted")?;
    require_bool_field(event, "used")?;
    require_bool_field(event, "fallback_to_direct")?;
    optional_usize_field(event, "status_code")?;
    optional_error_kind_field(event)?;
    optional_nonempty_trimmed_string_field(event, "error")?;

    match event.event_type.as_str() {
        "agent.create.daemon.used" => {
            require_bool_field_value(event, "attempted", true)?;
            require_bool_field_value(event, "used", true)?;
            require_bool_field_value(event, "fallback_to_direct", false)?;
            let status_code = require_usize_field(event, "status_code")?;
            if !(200..300).contains(&status_code) {
                return Err(CamError::InvalidState(format!(
                    "event `{}` used daemon with non-success status",
                    event.event_type
                )));
            }
            require_null_field(event, "error_kind")?;
            require_null_field(event, "error")?;
        }
        "agent.create.daemon.fallback" => {
            require_bool_field_value(event, "used", false)?;
            require_bool_field_value(event, "fallback_to_direct", true)?;
            if attempted {
                require_error_kind_field(event)?;
            }
            require_string_field(event, "error")?;
        }
        "agent.create.daemon.failed" => {
            require_bool_field_value(event, "used", false)?;
            require_bool_field_value(event, "fallback_to_direct", false)?;
            require_error_kind_field(event)?;
            require_string_field(event, "error")?;
        }
        _ => {}
    }
    Ok(())
}

fn validate_agent_set_model_daemon_route_event_payload(
    event: &StoredLogEvent,
) -> Result<(), CamError> {
    require_canonical_agent_field(event, "agent")?;
    require_bool_field(event, "daemon_running")?;
    let daemon_bind = require_string_field(event, "daemon_bind")?;
    if !is_loopback_bind(daemon_bind) {
        return Err(CamError::InvalidState(format!(
            "event `{}` daemon_bind `{daemon_bind}` is not loopback",
            event.event_type
        )));
    }
    require_daemon_port_field(event)?;
    require_bool_field(event, "model_present")?;
    require_bool_field(event, "model_provider_present")?;
    require_bool_field(event, "effort_present")?;
    require_bool_field(event, "speed_present")?;
    require_bool_field(event, "service_tier_present")?;
    let attempted = require_bool_field(event, "attempted")?;
    require_bool_field(event, "used")?;
    require_bool_field(event, "fallback_to_direct")?;
    optional_usize_field(event, "status_code")?;
    optional_error_kind_field(event)?;
    optional_nonempty_trimmed_string_field(event, "error")?;

    match event.event_type.as_str() {
        "agent.set_model.daemon.used" => {
            require_bool_field_value(event, "attempted", true)?;
            require_bool_field_value(event, "used", true)?;
            require_bool_field_value(event, "fallback_to_direct", false)?;
            let status_code = require_usize_field(event, "status_code")?;
            if !(200..300).contains(&status_code) {
                return Err(CamError::InvalidState(format!(
                    "event `{}` used daemon with non-success status",
                    event.event_type
                )));
            }
            require_null_field(event, "error_kind")?;
            require_null_field(event, "error")?;
        }
        "agent.set_model.daemon.fallback" => {
            require_bool_field_value(event, "used", false)?;
            require_bool_field_value(event, "fallback_to_direct", true)?;
            if attempted {
                require_error_kind_field(event)?;
            }
            require_string_field(event, "error")?;
        }
        "agent.set_model.daemon.failed" => {
            require_bool_field_value(event, "used", false)?;
            require_bool_field_value(event, "fallback_to_direct", false)?;
            require_error_kind_field(event)?;
            require_string_field(event, "error")?;
        }
        _ => {}
    }
    Ok(())
}

fn validate_agent_status_daemon_route_event_payload(
    event: &StoredLogEvent,
) -> Result<(), CamError> {
    require_canonical_agent_field(event, "agent")?;
    require_bool_field(event, "daemon_running")?;
    let daemon_bind = require_string_field(event, "daemon_bind")?;
    if !is_loopback_bind(daemon_bind) {
        return Err(CamError::InvalidState(format!(
            "event `{}` daemon_bind `{daemon_bind}` is not loopback",
            event.event_type
        )));
    }
    require_daemon_port_field(event)?;
    let attempted = require_bool_field(event, "attempted")?;
    require_bool_field(event, "used")?;
    require_bool_field(event, "fallback_to_direct")?;
    optional_usize_field(event, "status_code")?;
    optional_error_kind_field(event)?;
    optional_nonempty_trimmed_string_field(event, "error")?;

    match event.event_type.as_str() {
        "agent.status.daemon.used" => {
            require_bool_field_value(event, "attempted", true)?;
            require_bool_field_value(event, "used", true)?;
            require_bool_field_value(event, "fallback_to_direct", false)?;
            let status_code = require_usize_field(event, "status_code")?;
            if !(200..300).contains(&status_code) {
                return Err(CamError::InvalidState(format!(
                    "event `{}` used daemon with non-success status",
                    event.event_type
                )));
            }
            require_null_field(event, "error_kind")?;
            require_null_field(event, "error")?;
        }
        "agent.status.daemon.fallback" => {
            require_bool_field_value(event, "used", false)?;
            require_bool_field_value(event, "fallback_to_direct", true)?;
            if attempted {
                require_error_kind_field(event)?;
            }
            require_string_field(event, "error")?;
        }
        "agent.status.daemon.failed" => {
            require_bool_field_value(event, "used", false)?;
            require_bool_field_value(event, "fallback_to_direct", false)?;
            require_error_kind_field(event)?;
            require_string_field(event, "error")?;
        }
        _ => {}
    }
    Ok(())
}

fn validate_agent_read_daemon_route_event_payload(event: &StoredLogEvent) -> Result<(), CamError> {
    require_canonical_agent_field(event, "agent")?;
    require_bool_field(event, "daemon_running")?;
    let daemon_bind = require_string_field(event, "daemon_bind")?;
    if !is_loopback_bind(daemon_bind) {
        return Err(CamError::InvalidState(format!(
            "event `{}` daemon_bind `{daemon_bind}` is not loopback",
            event.event_type
        )));
    }
    require_daemon_port_field(event)?;
    let attempted = require_bool_field(event, "attempted")?;
    require_bool_field(event, "used")?;
    require_bool_field(event, "fallback_to_direct")?;
    require_bool_field(event, "latest_only")?;
    require_bool_field(event, "include_turns")?;
    optional_usize_field(event, "turn_limit")?;
    validate_optional_wait_seconds(event, "wait_seconds")?;
    optional_usize_field(event, "status_code")?;
    optional_error_kind_field(event)?;
    optional_nonempty_trimmed_string_field(event, "error")?;

    match event.event_type.as_str() {
        "agent.read.daemon.used" => {
            require_bool_field_value(event, "attempted", true)?;
            require_bool_field_value(event, "used", true)?;
            require_bool_field_value(event, "fallback_to_direct", false)?;
            let status_code = require_usize_field(event, "status_code")?;
            if !(200..300).contains(&status_code) {
                return Err(CamError::InvalidState(format!(
                    "event `{}` used daemon with non-success status",
                    event.event_type
                )));
            }
            require_null_field(event, "error_kind")?;
            require_null_field(event, "error")?;
        }
        "agent.read.daemon.fallback" => {
            require_bool_field_value(event, "used", false)?;
            require_bool_field_value(event, "fallback_to_direct", true)?;
            if attempted {
                require_error_kind_field(event)?;
            }
            require_string_field(event, "error")?;
        }
        "agent.read.daemon.failed" => {
            require_bool_field_value(event, "used", false)?;
            require_bool_field_value(event, "fallback_to_direct", false)?;
            require_error_kind_field(event)?;
            require_string_field(event, "error")?;
        }
        _ => {}
    }
    Ok(())
}

fn validate_message_event_plan(event: &StoredLogEvent, delivery: &str) -> Result<(), CamError> {
    let planned_delivery = require_string_field(event, "planned_delivery")?;
    if !matches!(
        planned_delivery,
        "received" | "queued" | "failed" | "delivered" | "steered"
    ) {
        return Err(CamError::InvalidState(format!(
            "event `{}` has invalid planned_delivery `{planned_delivery}`",
            event.event_type
        )));
    }

    let delivery_action = require_string_field(event, "delivery_action")?;
    if !matches!(
        delivery_action,
        "steer_active_turn"
            | "wake_known_session"
            | "store_in_virtual_inbox"
            | "queue_fallback"
            | "fail_loudly"
    ) {
        return Err(CamError::InvalidState(format!(
            "event `{}` has invalid delivery_action `{delivery_action}`",
            event.event_type
        )));
    }

    let outcome_matches_plan = require_bool_field(event, "outcome_matches_plan")?;
    let expected_match = delivery == planned_delivery;
    if expected_match {
        if !outcome_matches_plan {
            return Err(CamError::InvalidState(format!(
                "event `{}` outcome_matches_plan must be true when delivery matches planned_delivery",
                event.event_type
            )));
        }
    } else {
        if outcome_matches_plan {
            return Err(CamError::InvalidState(format!(
                "event `{}` outcome_matches_plan must be false when delivery differs from planned_delivery",
                event.event_type
            )));
        }
        validate_message_event_plan_mismatch(event, delivery, planned_delivery, delivery_action)?;
    }

    if delivery == "queued"
        && matches!(delivery_action, "wake_known_session" | "steer_active_turn")
        && outcome_matches_plan
    {
        return Err(CamError::InvalidState(format!(
            "event `{}` queued direct fallback cannot claim outcome_matches_plan",
            event.event_type
        )));
    }

    Ok(())
}

fn validate_message_event_plan_mismatch(
    event: &StoredLogEvent,
    delivery: &str,
    planned_delivery: &str,
    delivery_action: &str,
) -> Result<(), CamError> {
    if delivery == "delivered"
        && planned_delivery == "steered"
        && delivery_action == "steer_active_turn"
        && is_remote_mirror_stale_steer_recovery_event(event)?
    {
        return Ok(());
    }
    if delivery == "failed" {
        return Ok(());
    }
    if delivery == "queued" {
        return Ok(());
    }
    Err(CamError::InvalidState(format!(
        "event `{}` has unclassified plan mismatch delivery `{delivery}` planned `{planned_delivery}` action `{delivery_action}`",
        event.event_type
    )))
}

fn is_remote_mirror_stale_steer_recovery_event(event: &StoredLogEvent) -> Result<bool, CamError> {
    if event
        .fields
        .get("target_kind")
        .and_then(serde_json::Value::as_str)
        != Some("remote_mirror")
    {
        return Ok(false);
    }
    let target_peer = require_string_field(event, "target_peer")?;
    let target_route = require_string_field(event, "target_route")?;
    if target_route != format!("peer:{target_peer}") {
        return Ok(false);
    }
    require_bool_field_value(event, "thread_id_present", true)?;
    require_bool_field_value(event, "turn_id_present", true)?;
    require_null_field(event, "error_kind")?;
    require_null_field(event, "queue_allowed")?;
    let reason = require_string_field(event, "reason")?.to_ascii_lowercase();
    Ok(reason.contains("no active turn") && reason.contains("waking known thread"))
}

fn validate_message_event_error_classification(
    event: &StoredLogEvent,
    delivery: &str,
    strict: bool,
) -> Result<(), CamError> {
    let delivery_action = require_string_field(event, "delivery_action")?;
    let is_direct_attempt = matches!(delivery_action, "wake_known_session" | "steer_active_turn");
    let error_kind = optional_error_kind_field(event)?;
    let queue_allowed = optional_bool_field(event, "queue_allowed")?;

    if matches!(delivery, "delivered" | "steered" | "received") {
        if error_kind.is_some() {
            return Err(CamError::InvalidState(format!(
                "event `{}` successful delivery must not include error_kind",
                event.event_type
            )));
        }
        if queue_allowed.is_some() {
            return Err(CamError::InvalidState(format!(
                "event `{}` successful delivery must not include queue_allowed",
                event.event_type
            )));
        }
        return Ok(());
    }

    if matches!(delivery, "queued" | "failed") {
        let error_kind = error_kind.ok_or_else(|| {
            CamError::InvalidState(format!(
                "event `{}` message fallback/failure is missing error_kind",
                event.event_type
            ))
        })?;
        let queue_allowed = queue_allowed.ok_or_else(|| {
            CamError::InvalidState(format!(
                "event `{}` message fallback/failure is missing queue_allowed",
                event.event_type
            ))
        })?;
        let expected_queue_allowed =
            error_kind_allows_queue_fallback(error_kind) && !strict && delivery == "queued";
        if queue_allowed != expected_queue_allowed {
            return Err(CamError::InvalidState(format!(
                "event `{}` queue_allowed must be `{expected_queue_allowed}` for error_kind `{error_kind}`",
                event.event_type
            )));
        }
        if is_direct_attempt && delivery == "queued" && queue_allowed && strict {
            return Err(CamError::InvalidState(format!(
                "event `{}` strict direct delivery cannot allow queue fallback",
                event.event_type
            )));
        }
    }

    Ok(())
}

fn validate_direct_message_event_target_proof(
    event: &StoredLogEvent,
    target: &Agent,
) -> Result<(), CamError> {
    let target_kind = require_string_field(event, "target_kind")?;
    let expected_kind = format_kind(&target.kind);
    if target_kind != expected_kind {
        return Err(CamError::InvalidState(format!(
            "event `{}` target_kind `{target_kind}` does not match registered target kind `{expected_kind}`",
            event.event_type
        )));
    }

    let target_route = require_string_field(event, "target_route")?;
    let expected_route = format_route(&target.route);
    if target_route != expected_route {
        return Err(CamError::InvalidState(format!(
            "event `{}` target_route `{target_route}` does not match registered target route `{expected_route}`",
            event.event_type
        )));
    }

    match &target.route {
        Route::Local => {
            if event
                .fields
                .get("target_peer")
                .and_then(serde_json::Value::as_str)
                .is_some()
            {
                return Err(CamError::InvalidState(format!(
                    "event `{}` local direct delivery must not include target_peer proof",
                    event.event_type
                )));
            }
        }
        Route::Peer { peer_name } => {
            let target_peer = require_string_field(event, "target_peer")?;
            if target_peer != peer_name {
                return Err(CamError::InvalidState(format!(
                    "event `{}` target_peer `{target_peer}` does not match registered peer `{peer_name}`",
                    event.event_type
                )));
            }
        }
    }

    Ok(())
}

fn validate_delivery_proof_token(
    label: &str,
    event: &StoredLogEvent,
    field: &str,
) -> Result<(), CamError> {
    let value = require_string_field(event, field)?;
    if value.chars().any(char::is_whitespace) {
        return Err(CamError::InvalidState(format!(
            "{label} `{}` field `{field}` cannot contain whitespace",
            event.event_type
        )));
    }
    Ok(())
}

fn validate_optional_message_event_proof_flag(
    event: &StoredLogEvent,
    field: &str,
    present_flag: bool,
) -> Result<(), CamError> {
    let Some(value) = event.fields.get(field) else {
        if present_flag {
            return Err(CamError::InvalidState(format!(
                "event `{}` field `{field}_present` is true but `{field}` is missing",
                event.event_type
            )));
        }
        return Ok(());
    };

    if value.is_null() {
        if present_flag {
            return Err(CamError::InvalidState(format!(
                "event `{}` field `{field}_present` is true but `{field}` is null",
                event.event_type
            )));
        }
        return Ok(());
    }

    let Some(text) = value.as_str() else {
        return Err(CamError::InvalidState(format!(
            "event `{}` field `{field}` must be a string or null",
            event.event_type
        )));
    };

    if text.trim().is_empty() || text.trim() != text {
        return Err(CamError::InvalidState(format!(
            "event `{}` has non-canonical string field `{field}`",
            event.event_type
        )));
    }

    if !present_flag {
        return Err(CamError::InvalidState(format!(
            "event `{}` field `{field}_present` is false but `{field}` is present",
            event.event_type
        )));
    }

    Ok(())
}

fn validate_remote_mirror_direct_event_payload(
    event: &StoredLogEvent,
    target_agent: &str,
) -> Result<(), CamError> {
    let Some(target_kind) = event
        .fields
        .get("target_kind")
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(());
    };
    if target_kind != "remote_mirror" {
        return Ok(());
    }

    let target_peer = require_string_field(event, "target_peer")?;
    if !is_canonical_name(target_peer) {
        return Err(CamError::InvalidState(format!(
            "event `{}` remote mirror peer proof is not canonical",
            event.event_type
        )));
    }

    let target_route = require_string_field(event, "target_route")?;
    let expected_route = format!("peer:{target_peer}");
    if target_route != expected_route {
        return Err(CamError::InvalidState(format!(
            "event `{}` remote mirror route proof does not match peer proof",
            event.event_type
        )));
    }

    let expected_prefix = format!("{target_peer}::");
    if !target_agent.starts_with(&expected_prefix) {
        return Err(CamError::InvalidState(format!(
            "event `{}` remote mirror peer proof does not match target agent namespace",
            event.event_type
        )));
    }

    Ok(())
}

fn is_loud_failure_event(event_type: &str) -> bool {
    event_type.ends_with(".failed")
        || event_type.ends_with(".blocked")
        || event_type.ends_with(".rejected")
}

fn has_error_like_payload(fields: &serde_json::Value) -> bool {
    has_nonempty_string_field(fields, "error")
        || has_nonempty_string_field(fields, "reason")
        || fields.get("ok").and_then(serde_json::Value::as_bool) == Some(false)
}

fn has_nonempty_string_field(fields: &serde_json::Value, field: &str) -> bool {
    fields
        .get(field)
        .and_then(serde_json::Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
}

fn require_string_field<'a>(event: &'a StoredLogEvent, field: &str) -> Result<&'a str, CamError> {
    let value = event
        .fields
        .get(field)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            CamError::InvalidState(format!(
                "event `{}` is missing string field `{field}`",
                event.event_type
            ))
        })?;
    if value.trim().is_empty() || value.trim() != value {
        return Err(CamError::InvalidState(format!(
            "event `{}` has non-canonical string field `{field}`",
            event.event_type
        )));
    }
    Ok(value)
}

fn require_nullable_string_field(event: &StoredLogEvent, field: &str) -> Result<(), CamError> {
    let Some(value) = event.fields.get(field) else {
        return Err(CamError::InvalidState(format!(
            "event `{}` is missing nullable string field `{field}`",
            event.event_type
        )));
    };
    if value.is_null() {
        return Ok(());
    }
    let Some(value) = value.as_str() else {
        return Err(CamError::InvalidState(format!(
            "event `{}` field `{field}` must be string or null",
            event.event_type
        )));
    };
    if value.trim().is_empty() || value.trim() != value {
        return Err(CamError::InvalidState(format!(
            "event `{}` field `{field}` must be nonempty and trimmed when present",
            event.event_type
        )));
    }
    Ok(())
}

fn optional_nonempty_trimmed_string_field(
    event: &StoredLogEvent,
    field: &str,
) -> Result<(), CamError> {
    let Some(value) = event.fields.get(field) else {
        return Ok(());
    };
    if value.is_null() {
        return Ok(());
    }
    let Some(value) = value.as_str() else {
        return Err(CamError::InvalidState(format!(
            "event `{}` field `{field}` must be string or null when present",
            event.event_type
        )));
    };
    if value.trim().is_empty() || value.trim() != value {
        return Err(CamError::InvalidState(format!(
            "event `{}` field `{field}` must be nonempty and trimmed when present",
            event.event_type
        )));
    }
    Ok(())
}

fn require_canonical_agent_field(event: &StoredLogEvent, field: &str) -> Result<(), CamError> {
    let agent = require_string_field(event, field)?;
    if !is_canonical_name(agent) {
        return Err(CamError::InvalidState(format!(
            "event `{}` field `{field}` must be a canonical agent name",
            event.event_type
        )));
    }
    Ok(())
}

fn validate_agent_kind_label(event: &StoredLogEvent, value: &str) -> Result<(), CamError> {
    if matches!(
        value,
        "codex" | "virtual_inbox" | "agy_session" | "remote_mirror"
    ) {
        return Ok(());
    }
    Err(CamError::InvalidState(format!(
        "event `{}` has unknown agent kind `{value}`",
        event.event_type
    )))
}

fn validate_agent_status_label(event: &StoredLogEvent, value: &str) -> Result<(), CamError> {
    if matches!(value, "idle" | "active" | "error" | "unknown") {
        return Ok(());
    }
    Err(CamError::InvalidState(format!(
        "event `{}` has unknown agent status `{value}`",
        event.event_type
    )))
}

fn validate_agent_route_label(event: &StoredLogEvent, value: &str) -> Result<(), CamError> {
    if value == "local" {
        return Ok(());
    }
    let Some(peer) = value.strip_prefix("peer:") else {
        return Err(CamError::InvalidState(format!(
            "event `{}` has unknown agent route `{value}`",
            event.event_type
        )));
    };
    if !is_canonical_name(peer) {
        return Err(CamError::InvalidState(format!(
            "event `{}` has non-canonical peer route `{value}`",
            event.event_type
        )));
    }
    Ok(())
}

fn validate_optional_effort_field(event: &StoredLogEvent, field: &str) -> Result<(), CamError> {
    let Some(value) = event.fields.get(field) else {
        return Ok(());
    };
    if value.is_null() {
        return Ok(());
    }
    let Some(value) = value.as_str() else {
        return Err(CamError::InvalidState(format!(
            "event `{}` field `{field}` must be string or null when present",
            event.event_type
        )));
    };
    if matches!(value, "minimal" | "low" | "medium" | "high" | "xhigh") {
        return Ok(());
    }
    Err(CamError::InvalidState(format!(
        "event `{}` has unknown effort `{value}`",
        event.event_type
    )))
}

fn require_null_field(event: &StoredLogEvent, field: &str) -> Result<(), CamError> {
    if !event
        .fields
        .get(field)
        .is_some_and(serde_json::Value::is_null)
    {
        return Err(CamError::InvalidState(format!(
            "event `{}` field `{field}` must be null",
            event.event_type
        )));
    }
    Ok(())
}

fn require_string_field_value(
    event: &StoredLogEvent,
    field: &str,
    expected: &str,
) -> Result<(), CamError> {
    let actual = require_string_field(event, field)?;
    if actual != expected {
        return Err(CamError::InvalidState(format!(
            "event `{}` field `{field}` must be `{expected}`",
            event.event_type
        )));
    }
    Ok(())
}

fn require_error_kind_field(event: &StoredLogEvent) -> Result<(), CamError> {
    let error_kind = require_string_field(event, "error_kind")?;
    validate_error_kind_value(event, error_kind)
}

fn optional_error_kind_field(event: &StoredLogEvent) -> Result<Option<&str>, CamError> {
    let Some(value) = event.fields.get("error_kind") else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let error_kind = value.as_str().ok_or_else(|| {
        CamError::InvalidState(format!(
            "event `{}` has non-string error_kind field",
            event.event_type
        ))
    })?;
    validate_error_kind_value(event, error_kind)?;
    Ok(Some(error_kind))
}

fn validate_error_kind_value(event: &StoredLogEvent, error_kind: &str) -> Result<(), CamError> {
    services::validate_error_kind_token(error_kind).map_err(|message| {
        CamError::InvalidState(format!("event `{}` {message}", event.event_type))
    })
}

fn optional_bool_field(event: &StoredLogEvent, field: &str) -> Result<Option<bool>, CamError> {
    let Some(value) = event.fields.get(field) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    if let Some(value) = value.as_bool() {
        return Ok(Some(value));
    }
    Err(CamError::InvalidState(format!(
        "event `{}` field `{field}` must be bool or null when present",
        event.event_type
    )))
}

fn require_nullable_bool_field(event: &StoredLogEvent, field: &str) -> Result<(), CamError> {
    if !event
        .fields
        .as_object()
        .is_some_and(|fields| fields.contains_key(field))
    {
        return Err(CamError::InvalidState(format!(
            "event `{}` is missing nullable bool field `{field}`",
            event.event_type
        )));
    }
    optional_bool_field(event, field)?;
    Ok(())
}

fn error_kind_allows_queue_fallback(error_kind: &str) -> bool {
    matches!(
        error_kind,
        "delivery_failed" | "provider_unavailable" | "peer_transport_failed"
    )
}

fn require_bool_field(event: &StoredLogEvent, field: &str) -> Result<bool, CamError> {
    event
        .fields
        .get(field)
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| {
            CamError::InvalidState(format!(
                "event `{}` is missing bool field `{field}`",
                event.event_type
            ))
        })
}

fn require_bool_field_value(
    event: &StoredLogEvent,
    field: &str,
    expected: bool,
) -> Result<(), CamError> {
    let actual = require_bool_field(event, field)?;
    if actual != expected {
        return Err(CamError::InvalidState(format!(
            "event `{}` field `{field}` must be `{expected}`",
            event.event_type
        )));
    }
    Ok(())
}

fn require_number_field(event: &StoredLogEvent, field: &str) -> Result<(), CamError> {
    if !event
        .fields
        .get(field)
        .is_some_and(serde_json::Value::is_number)
    {
        return Err(CamError::InvalidState(format!(
            "event `{}` is missing number field `{field}`",
            event.event_type
        )));
    }
    Ok(())
}

fn require_usize_field(event: &StoredLogEvent, field: &str) -> Result<usize, CamError> {
    let value = event
        .fields
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            CamError::InvalidState(format!(
                "event `{}` is missing unsigned integer field `{field}`",
                event.event_type
            ))
        })?;
    usize::try_from(value).map_err(|_| {
        CamError::InvalidState(format!(
            "event `{}` field `{field}` is too large",
            event.event_type
        ))
    })
}

fn optional_usize_field(event: &StoredLogEvent, field: &str) -> Result<Option<usize>, CamError> {
    let Some(value) = event.fields.get(field) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let Some(value) = value.as_u64() else {
        return Err(CamError::InvalidState(format!(
            "event `{}` field `{field}` must be an unsigned integer or null when present",
            event.event_type
        )));
    };
    usize::try_from(value).map(Some).map_err(|_| {
        CamError::InvalidState(format!(
            "event `{}` field `{field}` is too large",
            event.event_type
        ))
    })
}

fn validate_optional_wait_seconds(event: &StoredLogEvent, field: &str) -> Result<(), CamError> {
    let Some(seconds) = optional_usize_field(event, field)? else {
        return Ok(());
    };
    if seconds > 300 {
        return Err(CamError::InvalidState(format!(
            "event `{}` field `{field}` must be 300 seconds or less",
            event.event_type
        )));
    }
    Ok(())
}

fn require_usize_field_value(
    event: &StoredLogEvent,
    field: &str,
    expected: usize,
) -> Result<(), CamError> {
    let actual = require_usize_field(event, field)?;
    if actual != expected {
        return Err(CamError::InvalidState(format!(
            "event `{}` field `{field}` must be `{expected}`",
            event.event_type
        )));
    }
    Ok(())
}

fn require_error_kind_array(event: &StoredLogEvent, field: &str) -> Result<(), CamError> {
    let values = event
        .fields
        .get(field)
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            CamError::InvalidState(format!(
                "event `{}` is missing array field `{field}`",
                event.event_type
            ))
        })?;
    if values.is_empty() {
        return Err(CamError::InvalidState(format!(
            "event `{}` array field `{field}` cannot be empty",
            event.event_type
        )));
    }
    let mut previous = None;
    for value in values {
        let error_kind = value.as_str().ok_or_else(|| {
            CamError::InvalidState(format!(
                "event `{}` array field `{field}` must contain only strings",
                event.event_type
            ))
        })?;
        validate_error_kind_value(event, error_kind)?;
        if let Some(previous) = previous {
            if error_kind <= previous {
                return Err(CamError::InvalidState(format!(
                    "event `{}` array field `{field}` must be sorted and unique",
                    event.event_type
                )));
            }
        }
        previous = Some(error_kind);
    }
    Ok(())
}

fn require_string_array_field_value(
    event: &StoredLogEvent,
    field: &str,
    expected: &[String],
) -> Result<(), CamError> {
    let values = event
        .fields
        .get(field)
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            CamError::InvalidState(format!(
                "event `{}` is missing array field `{field}`",
                event.event_type
            ))
        })?;
    let actual = values
        .iter()
        .map(|value| {
            value.as_str().map(str::to_string).ok_or_else(|| {
                CamError::InvalidState(format!(
                    "event `{}` array field `{field}` must contain only strings",
                    event.event_type
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if actual != expected {
        return Err(CamError::InvalidState(format!(
            "event `{}` array field `{field}` does not match peer sync results",
            event.event_type
        )));
    }
    Ok(())
}

fn require_string_array_field(
    event: &StoredLogEvent,
    field: &str,
) -> Result<Vec<String>, CamError> {
    let values = event
        .fields
        .get(field)
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            CamError::InvalidState(format!(
                "event `{}` is missing array field `{field}`",
                event.event_type
            ))
        })?;
    values
        .iter()
        .map(|value| {
            let value = value.as_str().ok_or_else(|| {
                CamError::InvalidState(format!(
                    "event `{}` array field `{field}` must contain only strings",
                    event.event_type
                ))
            })?;
            if value.trim().is_empty() || value.trim() != value || value.contains('=') {
                return Err(CamError::InvalidState(format!(
                    "event `{}` array field `{field}` contains a non-canonical key",
                    event.event_type
                )));
            }
            Ok(value.to_string())
        })
        .collect()
}

fn require_empty_array_field(event: &StoredLogEvent, field: &str) -> Result<(), CamError> {
    let values = event
        .fields
        .get(field)
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            CamError::InvalidState(format!(
                "event `{}` is missing array field `{field}`",
                event.event_type
            ))
        })?;
    if !values.is_empty() {
        return Err(CamError::InvalidState(format!(
            "event `{}` array field `{field}` must be empty",
            event.event_type
        )));
    }
    Ok(())
}

fn is_canonical_event_type(event_type: &str) -> bool {
    !event_type.is_empty()
        && event_type.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
        })
}

fn has_known_event_prefix(event_type: &str) -> bool {
    if matches!(
        event_type,
        "command.started" | "command.finished" | "command.failed"
    ) {
        return true;
    }
    let Some(prefix) = event_type.split('.').next() else {
        return false;
    };
    matches!(
        prefix,
        "state"
            | "api"
            | "doctor"
            | "daemon"
            | "agent"
            | "message"
            | "inbox"
            | "discovery"
            | "peer"
            | "provider"
            | "inventory"
            | "send"
            | "resume"
    )
}

fn is_known_command(command: &str) -> bool {
    matches!(
        command,
        "init"
            | "doctor"
            | "daemon status"
            | "daemon health"
            | "daemon status-ui"
            | "daemon configure"
            | "daemon start"
            | "daemon run"
            | "daemon stop"
            | "agent list"
            | "agent status"
            | "agent resume"
            | "agent read"
            | "agent create"
            | "agent set-model"
            | "send"
            | "inbox"
            | "discover local"
            | "peer add"
            | "peer list"
            | "peer sync"
            | "provider codex probe"
            | "inventory export"
    )
}

fn parse_event_timestamp(value: &str, event_type: &str) -> Result<OffsetDateTime, CamError> {
    OffsetDateTime::parse(value, &Rfc3339).map_err(|error| {
        CamError::InvalidState(format!(
            "event `{event_type}` has invalid timestamp: {error}"
        ))
    })
}
