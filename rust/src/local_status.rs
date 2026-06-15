use crate::services::DaemonStatusOutput;
use crate::state::AppState;
use serde::Serialize;
use serde_json::json;
use std::net::IpAddr;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LocalStatusSurface {
    pub bind: String,
    pub port: u16,
    pub health_path: &'static str,
    pub status_ui_path: &'static str,
    pub loopback_only: bool,
    pub public_network_enabled: bool,
    pub mutation_routes_enabled: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LocalStatusResponse {
    pub status_code: u16,
    pub content_type: &'static str,
    pub body: String,
}

impl LocalStatusResponse {
    pub fn ok_json(body: String) -> Self {
        Self {
            status_code: 200,
            content_type: "application/json",
            body,
        }
    }

    pub fn ok_html(body: String) -> Self {
        Self {
            status_code: 200,
            content_type: "text/html; charset=utf-8",
            body,
        }
    }

    pub fn forbidden_text(body: impl Into<String>) -> Self {
        Self {
            status_code: 403,
            content_type: "text/plain; charset=utf-8",
            body: body.into(),
        }
    }

    pub fn unauthorized_text(body: impl Into<String>) -> Self {
        Self {
            status_code: 401,
            content_type: "text/plain; charset=utf-8",
            body: body.into(),
        }
    }

    pub fn not_found_text(body: impl Into<String>) -> Self {
        Self {
            status_code: 404,
            content_type: "text/plain; charset=utf-8",
            body: body.into(),
        }
    }

    pub fn method_not_allowed_text(body: impl Into<String>) -> Self {
        Self {
            status_code: 405,
            content_type: "text/plain; charset=utf-8",
            body: body.into(),
        }
    }

    pub fn internal_server_error_text(body: impl Into<String>) -> Self {
        Self {
            status_code: 500,
            content_type: "text/plain; charset=utf-8",
            body: body.into(),
        }
    }

    pub fn service_unavailable_text(body: impl Into<String>) -> Self {
        Self {
            status_code: 503,
            content_type: "text/plain; charset=utf-8",
            body: body.into(),
        }
    }

    pub fn bad_request_text(body: impl Into<String>) -> Self {
        Self {
            status_code: 400,
            content_type: "text/plain; charset=utf-8",
            body: body.into(),
        }
    }

    pub fn to_http_response(&self) -> String {
        format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{}",
            self.status_code,
            reason_phrase(self.status_code),
            self.content_type,
            self.body.len(),
            self.body
        )
    }
}

pub fn surface_for_state(state: &AppState) -> LocalStatusSurface {
    LocalStatusSurface {
        bind: state.daemon.bind.clone(),
        port: state.daemon.port,
        health_path: "/health",
        status_ui_path: "/status-ui",
        loopback_only: is_loopback_bind(&state.daemon.bind),
        public_network_enabled: false,
        mutation_routes_enabled: true,
    }
}

pub fn render_health_response(state: &AppState) -> LocalStatusResponse {
    let status = crate::services::daemon_status(state);
    render_health_response_with_status(state, &status)
}

pub fn render_health_response_with_status(
    state: &AppState,
    status: &DaemonStatusOutput,
) -> LocalStatusResponse {
    let body = json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "node_name": state.config.node_name,
        "daemon": {
            "message": status.message,
            "state": status.state,
            "desired_state": status.state.desired_state,
            "observed_state": status.state.observed_state,
            "startup_phase": status.state.startup_phase,
            "started_at": status.state.started_at,
            "last_heartbeat_at": status.state.last_heartbeat_at,
            "running": status.running,
            "implemented": status.implemented,
            "state_source": status.state_source,
            "live_probe_attempted": status.live_probe_attempted,
            "process_supervisor_wired": status.process_supervisor_wired,
            "process_exists": status.process_exists,
            "instance_id_present": status.instance_id_present,
            "identity_nonce_ref_present": status.identity_nonce_ref_present,
            "instance_id_matched": status.instance_id_matched,
            "node_name_matched": status.node_name_matched,
            "version_matched": status.version_matched,
            "process_identity_verified": status.process_identity_verified,
            "identity_mismatch": status.identity_mismatch,
            "live_probe_error": status.live_probe_error,
        },
        "app_server_initialized": false,
        "surface": surface_for_state(state),
    })
    .to_string();

    LocalStatusResponse::ok_json(body)
}

pub fn render_status_ui_response(state: &AppState) -> LocalStatusResponse {
    if state.daemon.headless {
        return LocalStatusResponse::forbidden_text(
            "status UI is disabled in headless mode; use daemon health or daemon status",
        );
    }

    let html = format!(
        concat!(
            "<!doctype html><html><head><meta charset=\"utf-8\">",
            "<title>Qexow CAM Status</title></head><body>",
            "<h1>Qexow CAM Status</h1>",
            "<dl>",
            "<dt>Node</dt><dd>{}</dd>",
            "<dt>Version</dt><dd>{}</dd>",
            "<dt>Daemon desired</dt><dd>{:?}</dd>",
            "<dt>Daemon observed</dt><dd>{:?}</dd>",
            "<dt>Startup phase</dt><dd>{}</dd>",
            "<dt>Agents</dt><dd>{}</dd>",
            "<dt>Peers</dt><dd>{}</dd>",
            "<dt>Mailbox messages</dt><dd>{}</dd>",
            "</dl>",
            "<p>This local status page is read-only and loopback-only.</p>",
            "</body></html>"
        ),
        escape_html(&state.config.node_name),
        env!("CARGO_PKG_VERSION"),
        state.daemon.desired_state,
        state.daemon.observed_state,
        escape_html(&state.daemon.startup_phase),
        state.agents.len(),
        state.peers.len(),
        state.mailbox.len()
    );

    LocalStatusResponse::ok_html(html)
}

pub fn handle_loopback_http_request(
    state: &AppState,
    peer_addr: &str,
    raw_request: &str,
) -> LocalStatusResponse {
    if !is_loopback_bind(peer_addr) {
        return LocalStatusResponse::forbidden_text(
            "local status endpoints are loopback-only; remote clients are forbidden",
        );
    }

    let Some(request_line) = raw_request.lines().next().map(str::trim) else {
        return LocalStatusResponse::bad_request_text("missing HTTP request line");
    };
    let mut parts = request_line.split_whitespace();
    let Some(method) = parts.next() else {
        return LocalStatusResponse::bad_request_text("missing HTTP method");
    };
    let Some(path) = parts.next() else {
        return LocalStatusResponse::bad_request_text("missing HTTP path");
    };
    let Some(version) = parts.next() else {
        return LocalStatusResponse::bad_request_text("missing HTTP version");
    };
    if parts.next().is_some() || !version.starts_with("HTTP/") {
        return LocalStatusResponse::bad_request_text("malformed HTTP request line");
    }
    if method != "GET" {
        return LocalStatusResponse::method_not_allowed_text(
            "local status endpoints only allow GET",
        );
    }

    match path.split_once('?').map(|(path, _)| path).unwrap_or(path) {
        "/health" => render_health_response(state),
        "/status-ui" => render_status_ui_response(state),
        _ => LocalStatusResponse::not_found_text("unknown local status path"),
    }
}

pub fn is_loopback_bind(bind: &str) -> bool {
    if let Ok(address) = bind.parse::<IpAddr>() {
        return address.is_loopback();
    }

    let host = bind
        .strip_prefix('[')
        .and_then(|value| value.split(']').next())
        .or_else(|| bind.rsplit_once(':').map(|(host, _)| host))
        .unwrap_or(bind);

    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }

    host.parse::<IpAddr>()
        .map(|address| address.is_loopback())
        .unwrap_or(false)
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn reason_phrase(status_code: u16) -> &'static str {
    match status_code {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    }
}
