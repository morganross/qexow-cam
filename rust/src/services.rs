use crate::core::{
    Agent, AgentKind, AgentStatus, ChatStatus, ChatStatusSource, DaemonDesiredState,
    DaemonObservedState, DaemonState, DeliveryState, DiscoveryDisposition, DiscoveryRow,
    DiscoverySource, Effort, Message, Peer, PeerState, PeerTransport, Route, ThreadSource, now_utc,
};
use crate::delivery::{DeliveryAction, DeliveryDecision, decide_delivery};
use crate::discovery::{
    DesktopThreadArchiveEvidence, DiscoverySummary, PromotionDecision, PromotionDecisionKind,
    classify_row, default_codex_home, discover_local_codex, scan_desktop_thread_archive_evidence,
};
use crate::errors::CamError;
use crate::local_status::{LocalStatusResponse, LocalStatusSurface};
use crate::logging::{StoredLogEvent, read_events};
use crate::peers::{
    InventoryExport, PeerAgentReadFetcher, PeerAgentResumeFetcher, PeerInventoryFetcher,
    RemoteMirrorApplyResult, apply_trusted_peer_inventory, build_inventory, parse_inventory_export,
};
use crate::providers::{
    ProviderLifecycleRouter, ProviderSendOutcome, ProviderSendRequest, ProviderTranscriptOutcome,
    ProviderTranscriptReader, ProviderTranscriptRequest,
    codex::{CodexOwnerRegistry, CodexThreadStartOutcome},
    validate_provider_transcript_outcome,
};
use crate::resume::{
    AgentResumeResult, apply_provider_resume_outcome, result_from_lifecycle_error,
    resume_with_readiness_check, should_attempt_provider_resume,
};
use crate::state::{AppState, StateStore, validate_daemon_identity_nonce_ref};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

pub const DAEMON_STATE_SOURCE: &str = "persisted_daemon_json";
pub const DAEMON_RUN_IDENTITY_SURFACE: &str = "daemon_run_identity";
pub const DAEMON_IDENTITY_PROOF_ALGORITHM: &str = "hmac-sha256:qcam-daemon-identity-v1";
pub const MAX_LOOPBACK_WORKERS: usize = 32;
pub const MAX_AGENT_READ_TURNS: usize = 200;
const DAEMON_IDENTITY_PROOF_CONTEXT: &str = "qcam-daemon-identity-v1";

pub fn new_daemon_instance_id() -> String {
    Uuid::new_v4().to_string()
}

pub fn daemon_identity_nonce_ref(instance_id: &str) -> String {
    format!("secrets/daemon-identity/{instance_id}.nonce")
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DaemonStatusOutput {
    pub ok: bool,
    pub implemented: bool,
    pub running: bool,
    pub state_source: &'static str,
    pub live_probe_attempted: bool,
    pub process_supervisor_wired: bool,
    pub process_exists: Option<bool>,
    pub instance_id_present: bool,
    pub identity_nonce_ref_present: bool,
    pub instance_id_matched: bool,
    pub node_name_matched: bool,
    pub version_matched: bool,
    pub process_identity_verified: bool,
    pub identity_mismatch: Option<String>,
    pub live_probe_error: Option<String>,
    pub message: String,
    pub state: DaemonState,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DaemonRunIdentityOutput {
    pub ok: bool,
    pub surface: &'static str,
    pub would_run: bool,
    pub process_supervisor_wired: bool,
    pub state_source: &'static str,
    pub pid: u32,
    pub instance_id: Option<String>,
    pub identity_nonce_ref: Option<String>,
    pub identity_challenge: String,
    pub nonce_proof_algorithm: &'static str,
    pub nonce_proof_present: bool,
    pub nonce_proof: Option<String>,
    pub node_name: String,
    pub version: String,
    pub instance_id_present: bool,
    pub identity_nonce_ref_present: bool,
    pub identity_nonce_available: bool,
    pub nonce_value_exposed: bool,
    pub identity_ready: bool,
    pub identity_error: Option<String>,
}

pub fn daemon_status(state: &AppState) -> DaemonStatusOutput {
    daemon_status_with_probe(&state.daemon, &FilesystemDaemonLiveProbe)
}

pub trait DaemonLiveProbe {
    fn probe(&self, state: &DaemonState, pid: u32) -> DaemonLiveProbeEvidence;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonLiveProbeEvidence {
    pub process_exists: bool,
    pub observed_instance_id: Option<String>,
    pub observed_node_name: Option<String>,
    pub observed_version: Option<String>,
    pub error: Option<String>,
}

pub struct FilesystemDaemonLiveProbe;

impl DaemonLiveProbe for FilesystemDaemonLiveProbe {
    fn probe(&self, _state: &DaemonState, pid: u32) -> DaemonLiveProbeEvidence {
        let process_path = format!("/proc/{pid}");
        let process_exists = Path::new(&process_path).exists();
        DaemonLiveProbeEvidence {
            process_exists,
            observed_instance_id: None,
            observed_node_name: None,
            observed_version: None,
            error: if process_exists {
                Some(
                    "process exists, but CAM daemon identity proof is unavailable until the supervisor is wired"
                        .to_string(),
                )
            } else {
                None
            },
        }
    }
}

pub fn daemon_status_with_probe(
    state: &DaemonState,
    probe: &impl DaemonLiveProbe,
) -> DaemonStatusOutput {
    daemon_status_from_state(state, probe)
}

pub fn daemon_run_identity_check(
    state: &DaemonState,
    instance_id: &str,
    identity_nonce_ref: &str,
    identity_challenge: &str,
    nonce: Option<&str>,
    identity_nonce_available: bool,
    identity_error: Option<String>,
) -> DaemonRunIdentityOutput {
    let instance_id_present =
        state.instance_id.as_deref() == Some(instance_id) && !instance_id.trim().is_empty();
    let identity_nonce_ref_present = state.identity_nonce_ref.as_deref()
        == Some(identity_nonce_ref)
        && !identity_nonce_ref.trim().is_empty();
    let nonce_proof = nonce.filter(|_| identity_nonce_available).map(|nonce| {
        daemon_identity_nonce_proof(
            nonce,
            identity_challenge,
            instance_id,
            &state.node_name,
            &state.version,
        )
    });
    let identity_ready = instance_id_present && identity_nonce_ref_present && nonce_proof.is_some();
    DaemonRunIdentityOutput {
        ok: identity_ready,
        surface: DAEMON_RUN_IDENTITY_SURFACE,
        would_run: false,
        process_supervisor_wired: false,
        state_source: DAEMON_STATE_SOURCE,
        pid: std::process::id(),
        instance_id: if instance_id_present {
            Some(instance_id.to_string())
        } else {
            None
        },
        identity_nonce_ref: if identity_nonce_ref_present {
            Some(identity_nonce_ref.to_string())
        } else {
            None
        },
        identity_challenge: identity_challenge.to_string(),
        nonce_proof_algorithm: DAEMON_IDENTITY_PROOF_ALGORITHM,
        nonce_proof_present: nonce_proof.is_some(),
        nonce_proof,
        node_name: state.node_name.clone(),
        version: state.version.clone(),
        instance_id_present,
        identity_nonce_ref_present,
        identity_nonce_available,
        nonce_value_exposed: false,
        identity_ready,
        identity_error,
    }
}

pub fn daemon_identity_nonce_proof(
    nonce: &str,
    challenge: &str,
    instance_id: &str,
    node_name: &str,
    version: &str,
) -> String {
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(nonce.as_bytes())
        .expect("HMAC-SHA256 accepts keys of any length");
    mac.update(DAEMON_IDENTITY_PROOF_CONTEXT.as_bytes());
    mac.update(b"\n");
    mac.update(challenge.as_bytes());
    mac.update(b"\n");
    mac.update(instance_id.as_bytes());
    mac.update(b"\n");
    mac.update(node_name.as_bytes());
    mac.update(b"\n");
    mac.update(version.as_bytes());
    let bytes = mac.finalize().into_bytes();
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn daemon_desired_state_label(state: &DaemonDesiredState) -> &'static str {
    match state {
        DaemonDesiredState::Stopped => "stopped",
        DaemonDesiredState::StartRequested => "start_requested",
    }
}

pub fn daemon_observed_state_label(state: &DaemonObservedState) -> &'static str {
    match state {
        DaemonObservedState::NotRunning => "not_running",
        DaemonObservedState::NotImplemented => "not_implemented",
        DaemonObservedState::Stale => "stale",
        DaemonObservedState::Running => "running",
        DaemonObservedState::Error => "error",
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StateInitializedEventFields {
    pub home: String,
    pub agents: usize,
    pub daemon_observed_state: crate::core::DaemonObservedState,
}

pub fn state_initialized_event_fields(
    home: impl Into<String>,
    state: &AppState,
) -> StateInitializedEventFields {
    StateInitializedEventFields {
        home: home.into(),
        agents: state.agents.len(),
        daemon_observed_state: state.daemon.observed_state.clone(),
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DoctorFailedEventFields {
    pub error_kind: &'static str,
    pub error: String,
}

pub fn doctor_failed_event_fields(error: &CamError) -> DoctorFailedEventFields {
    DoctorFailedEventFields {
        error_kind: error.kind(),
        error: error.to_string(),
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StateInitializedSummary {
    pub home: String,
    pub agents: usize,
}

pub fn state_initialized_summary(
    home: impl Into<String>,
    state: &AppState,
) -> StateInitializedSummary {
    StateInitializedSummary {
        home: home.into(),
        agents: state.agents.len(),
    }
}

pub fn request_daemon_start(
    state: &mut AppState,
    headless: bool,
    instance_id: impl Into<String>,
    identity_nonce_ref: impl Into<String>,
) -> DaemonStatusOutput {
    state
        .daemon
        .start_requested(headless, instance_id, identity_nonce_ref);
    daemon_status_with_probe(&state.daemon, &FilesystemDaemonLiveProbe)
}

pub fn request_daemon_stop(state: &mut AppState) -> DaemonStatusOutput {
    state.daemon.stop_requested();
    daemon_status_with_probe(&state.daemon, &FilesystemDaemonLiveProbe)
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DaemonRunIdentityCheckedEventFields {
    pub ok: bool,
    pub surface: &'static str,
    pub would_run: bool,
    pub process_supervisor_wired: bool,
    pub state_source: &'static str,
    pub pid: u32,
    pub identity_challenge_present: bool,
    pub nonce_proof_algorithm: &'static str,
    pub nonce_proof_present: bool,
    pub node_name: String,
    pub version: String,
    pub instance_id_present: bool,
    pub identity_nonce_ref_present: bool,
    pub identity_nonce_available: bool,
    pub nonce_value_exposed: bool,
    pub identity_ready: bool,
    pub identity_error: Option<String>,
}

pub fn daemon_run_identity_checked_event_fields(
    output: &DaemonRunIdentityOutput,
) -> DaemonRunIdentityCheckedEventFields {
    DaemonRunIdentityCheckedEventFields {
        ok: output.ok,
        surface: output.surface,
        would_run: output.would_run,
        process_supervisor_wired: output.process_supervisor_wired,
        state_source: output.state_source,
        pid: output.pid,
        identity_challenge_present: !output.identity_challenge.trim().is_empty(),
        nonce_proof_algorithm: output.nonce_proof_algorithm,
        nonce_proof_present: output.nonce_proof_present,
        node_name: output.node_name.clone(),
        version: output.version.clone(),
        instance_id_present: output.instance_id_present,
        identity_nonce_ref_present: output.identity_nonce_ref_present,
        identity_nonce_available: output.identity_nonce_available,
        nonce_value_exposed: output.nonce_value_exposed,
        identity_ready: output.identity_ready,
        identity_error: output.identity_error.clone(),
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DaemonStatusCheckedEventFields {
    pub desired_state: crate::core::DaemonDesiredState,
    pub observed_state: crate::core::DaemonObservedState,
    pub state_source: &'static str,
    pub live_probe_attempted: bool,
    pub process_supervisor_wired: bool,
    pub process_exists: Option<bool>,
    pub instance_id_present: bool,
    pub identity_nonce_ref_present: bool,
    pub instance_id_matched: bool,
    pub node_name_matched: bool,
    pub version_matched: bool,
    pub process_identity_verified: bool,
    pub identity_mismatch: Option<String>,
    pub live_probe_error: Option<String>,
    pub running: bool,
    pub implemented: bool,
    pub pid: Option<u32>,
}

pub fn daemon_status_checked_event_fields(
    status: &DaemonStatusOutput,
) -> DaemonStatusCheckedEventFields {
    DaemonStatusCheckedEventFields {
        desired_state: status.state.desired_state.clone(),
        observed_state: status.state.observed_state.clone(),
        state_source: status.state_source,
        live_probe_attempted: status.live_probe_attempted,
        process_supervisor_wired: status.process_supervisor_wired,
        process_exists: status.process_exists,
        instance_id_present: status.instance_id_present,
        identity_nonce_ref_present: status.identity_nonce_ref_present,
        instance_id_matched: status.instance_id_matched,
        node_name_matched: status.node_name_matched,
        version_matched: status.version_matched,
        process_identity_verified: status.process_identity_verified,
        identity_mismatch: status.identity_mismatch.clone(),
        live_probe_error: status.live_probe_error.clone(),
        running: status.running,
        implemented: status.implemented,
        pid: status.state.pid,
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DaemonStartBlockedEventFields {
    pub desired_state: crate::core::DaemonDesiredState,
    pub observed_state: crate::core::DaemonObservedState,
    pub state_source: &'static str,
    pub live_probe_attempted: bool,
    pub process_supervisor_wired: bool,
    pub process_exists: Option<bool>,
    pub instance_id_present: bool,
    pub identity_nonce_ref_present: bool,
    pub instance_id_matched: bool,
    pub node_name_matched: bool,
    pub version_matched: bool,
    pub process_identity_verified: bool,
    pub identity_mismatch: Option<String>,
    pub live_probe_error: Option<String>,
    pub headless: bool,
    pub running: bool,
    pub implemented: bool,
    pub pid: Option<u32>,
    pub error: Option<String>,
}

pub fn daemon_start_blocked_event_fields(
    status: &DaemonStatusOutput,
) -> DaemonStartBlockedEventFields {
    DaemonStartBlockedEventFields {
        desired_state: status.state.desired_state.clone(),
        observed_state: status.state.observed_state.clone(),
        state_source: status.state_source,
        live_probe_attempted: status.live_probe_attempted,
        process_supervisor_wired: status.process_supervisor_wired,
        process_exists: status.process_exists,
        instance_id_present: status.instance_id_present,
        identity_nonce_ref_present: status.identity_nonce_ref_present,
        instance_id_matched: status.instance_id_matched,
        node_name_matched: status.node_name_matched,
        version_matched: status.version_matched,
        process_identity_verified: status.process_identity_verified,
        identity_mismatch: status.identity_mismatch.clone(),
        live_probe_error: status.live_probe_error.clone(),
        headless: status.state.headless,
        running: status.running,
        implemented: status.implemented,
        pid: status.state.pid,
        error: status.state.last_error.clone(),
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DaemonStopRequestedEventFields {
    pub desired_state: crate::core::DaemonDesiredState,
    pub observed_state: crate::core::DaemonObservedState,
    pub state_source: &'static str,
    pub live_probe_attempted: bool,
    pub process_supervisor_wired: bool,
    pub process_exists: Option<bool>,
    pub instance_id_present: bool,
    pub identity_nonce_ref_present: bool,
    pub instance_id_matched: bool,
    pub node_name_matched: bool,
    pub version_matched: bool,
    pub process_identity_verified: bool,
    pub identity_mismatch: Option<String>,
    pub live_probe_error: Option<String>,
    pub running: bool,
    pub implemented: bool,
    pub pid: Option<u32>,
}

pub fn daemon_stop_requested_event_fields(
    status: &DaemonStatusOutput,
) -> DaemonStopRequestedEventFields {
    DaemonStopRequestedEventFields {
        desired_state: status.state.desired_state.clone(),
        observed_state: status.state.observed_state.clone(),
        state_source: status.state_source,
        live_probe_attempted: status.live_probe_attempted,
        process_supervisor_wired: status.process_supervisor_wired,
        process_exists: status.process_exists,
        instance_id_present: status.instance_id_present,
        identity_nonce_ref_present: status.identity_nonce_ref_present,
        instance_id_matched: status.instance_id_matched,
        node_name_matched: status.node_name_matched,
        version_matched: status.version_matched,
        process_identity_verified: status.process_identity_verified,
        identity_mismatch: status.identity_mismatch.clone(),
        live_probe_error: status.live_probe_error.clone(),
        running: status.running,
        implemented: status.implemented,
        pid: status.state.pid,
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DaemonHealthRenderedEventFields {
    pub status_code: u16,
    pub content_type: &'static str,
    pub loopback_only: bool,
    pub public_network_enabled: bool,
    pub mutation_routes_enabled: bool,
    pub state_source: &'static str,
    pub live_probe_attempted: bool,
    pub process_supervisor_wired: bool,
    pub process_exists: Option<bool>,
    pub instance_id_present: bool,
    pub identity_nonce_ref_present: bool,
    pub instance_id_matched: bool,
    pub node_name_matched: bool,
    pub version_matched: bool,
    pub process_identity_verified: bool,
    pub identity_mismatch: Option<String>,
    pub live_probe_error: Option<String>,
    pub running: bool,
    pub implemented: bool,
    pub pid: Option<u32>,
}

pub fn daemon_health_rendered_event_fields(
    response: &LocalStatusResponse,
    surface: &LocalStatusSurface,
    status: &DaemonStatusOutput,
) -> DaemonHealthRenderedEventFields {
    DaemonHealthRenderedEventFields {
        status_code: response.status_code,
        content_type: response.content_type,
        loopback_only: surface.loopback_only,
        public_network_enabled: surface.public_network_enabled,
        mutation_routes_enabled: surface.mutation_routes_enabled,
        state_source: status.state_source,
        live_probe_attempted: status.live_probe_attempted,
        process_supervisor_wired: status.process_supervisor_wired,
        process_exists: status.process_exists,
        instance_id_present: status.instance_id_present,
        identity_nonce_ref_present: status.identity_nonce_ref_present,
        instance_id_matched: status.instance_id_matched,
        node_name_matched: status.node_name_matched,
        version_matched: status.version_matched,
        process_identity_verified: status.process_identity_verified,
        identity_mismatch: status.identity_mismatch.clone(),
        live_probe_error: status.live_probe_error.clone(),
        running: status.running,
        implemented: status.implemented,
        pid: status.state.pid,
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DaemonStatusUiRenderedEventFields {
    pub status_code: u16,
    pub content_type: &'static str,
    pub headless: bool,
    pub loopback_only: bool,
    pub public_network_enabled: bool,
    pub mutation_routes_enabled: bool,
    pub state_source: &'static str,
}

pub fn daemon_status_ui_rendered_event_fields(
    response: &LocalStatusResponse,
    surface: &LocalStatusSurface,
    state: &DaemonState,
) -> DaemonStatusUiRenderedEventFields {
    DaemonStatusUiRenderedEventFields {
        status_code: response.status_code,
        content_type: response.content_type,
        headless: state.headless,
        loopback_only: surface.loopback_only,
        public_network_enabled: surface.public_network_enabled,
        mutation_routes_enabled: surface.mutation_routes_enabled,
        state_source: DAEMON_STATE_SOURCE,
    }
}

fn daemon_status_from_state(
    state: &DaemonState,
    probe: &impl DaemonLiveProbe,
) -> DaemonStatusOutput {
    let mut status_state = state.clone();
    let mut live_probe_attempted = false;
    let mut process_exists = None;
    let instance_id_present = state.instance_id.is_some();
    let identity_nonce_ref_present = state.identity_nonce_ref.is_some();
    let mut instance_id_matched = false;
    let mut node_name_matched = false;
    let mut version_matched = false;
    let mut process_identity_verified = false;
    let mut identity_mismatch = None;
    let mut live_probe_error = None;
    let mut message =
        "daemon process supervisor is unavailable in this build; persisted daemon.json is state, not liveness proof"
            .to_string();

    if state.pid == Some(std::process::id())
        && state.observed_state == DaemonObservedState::Running
        && state.instance_id.is_some()
        && state.identity_nonce_ref.is_some()
    {
        process_exists = Some(true);
        instance_id_matched = true;
        node_name_matched = true;
        version_matched = state.version == env!("CARGO_PKG_VERSION");
        process_identity_verified = version_matched;
        status_state.identity_verified_at = Some(now_utc());
        message = "daemon foreground process identity matches the current process".to_string();
    } else if let Some(pid) = state.pid {
        live_probe_attempted = true;
        let evidence = probe.probe(state, pid);
        process_exists = Some(evidence.process_exists);
        instance_id_matched = state
            .instance_id
            .as_ref()
            .zip(evidence.observed_instance_id.as_ref())
            .is_some_and(|(expected, actual)| expected == actual);
        node_name_matched = evidence
            .observed_node_name
            .as_ref()
            .is_some_and(|observed| observed == &state.node_name);
        version_matched = evidence
            .observed_version
            .as_ref()
            .is_some_and(|observed| observed == &state.version);
        process_identity_verified = evidence.process_exists
            && identity_nonce_ref_present
            && instance_id_matched
            && node_name_matched
            && version_matched;
        identity_mismatch = daemon_identity_mismatch(
            state,
            &evidence,
            instance_id_matched,
            node_name_matched,
            version_matched,
        );
        live_probe_error = evidence.error;
        if process_identity_verified {
            status_state.observed_state = DaemonObservedState::Running;
            status_state.identity_verified_at = Some(now_utc());
            message =
                "daemon process identity was verified by live probe; running status is live evidence"
                    .to_string();
        } else {
            status_state.observed_state = DaemonObservedState::Stale;
            status_state.last_error = Some(
                live_probe_error
                    .clone()
                    .unwrap_or_else(|| "persisted daemon pid is not verified as CAM".to_string()),
            );
            message =
                "persisted daemon pid was probed but not verified as CAM; treating daemon state as stale"
                    .to_string();
        }
    }

    let running = process_identity_verified;
    DaemonStatusOutput {
        ok: true,
        implemented: running,
        running,
        state_source: DAEMON_STATE_SOURCE,
        live_probe_attempted,
        process_supervisor_wired: running,
        process_exists,
        instance_id_present,
        identity_nonce_ref_present,
        instance_id_matched,
        node_name_matched,
        version_matched,
        process_identity_verified,
        identity_mismatch,
        live_probe_error,
        message,
        state: status_state,
    }
}

fn daemon_identity_mismatch(
    state: &DaemonState,
    evidence: &DaemonLiveProbeEvidence,
    instance_id_matched: bool,
    node_name_matched: bool,
    version_matched: bool,
) -> Option<String> {
    if !evidence.process_exists {
        return Some("process does not exist".to_string());
    }
    if state.instance_id.is_none() {
        return Some("daemon state has no instance_id".to_string());
    }
    if state.identity_nonce_ref.is_none() {
        return Some("daemon state has no identity_nonce_ref".to_string());
    }
    if evidence.observed_instance_id.is_none() {
        return Some("probe did not report instance_id".to_string());
    }
    if !instance_id_matched {
        return Some("probe instance_id did not match daemon state".to_string());
    }
    if evidence.observed_node_name.is_none() {
        return Some("probe did not report node_name".to_string());
    }
    if !node_name_matched {
        return Some("probe node_name did not match daemon state".to_string());
    }
    if evidence.observed_version.is_none() {
        return Some("probe did not report version".to_string());
    }
    if !version_matched {
        return Some("probe version did not match daemon state".to_string());
    }
    None
}

pub fn list_agents(state: &AppState) -> Vec<Agent> {
    state.agents.clone()
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AgentListRow {
    pub name: String,
    pub kind: &'static str,
    pub status: String,
    pub chat_status: String,
    pub chat_status_source: String,
    pub route: String,
    pub thread_id: Option<String>,
}

pub fn list_agent_rows(state: &AppState) -> Vec<AgentListRow> {
    state
        .agents
        .iter()
        .map(|agent| AgentListRow {
            name: agent.name.clone(),
            kind: format_agent_kind(&agent.kind),
            status: format!("{:?}", agent.status).to_lowercase(),
            chat_status: format_chat_status(&agent.chat_status).to_string(),
            chat_status_source: format_chat_status_source(&agent.chat_status_source),
            route: format_route(&agent.route),
            thread_id: agent.thread_id.clone(),
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AgentCreatedSummary {
    pub name: String,
    pub kind: &'static str,
    pub thread_id: Option<String>,
}

pub fn agent_created_summary(agent: &Agent) -> AgentCreatedSummary {
    AgentCreatedSummary {
        name: agent.name.clone(),
        kind: format_agent_kind(&agent.kind),
        thread_id: agent.thread_id.clone(),
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AgentModelUpdatedSummary {
    pub name: String,
    pub thread_id: Option<String>,
}

pub fn agent_model_updated_summary(agent: &Agent) -> AgentModelUpdatedSummary {
    AgentModelUpdatedSummary {
        name: agent.name.clone(),
        thread_id: agent.thread_id.clone(),
    }
}

pub fn get_agent(state: &AppState, name: &str) -> Result<Agent, CamError> {
    validate_canonical_name("agent name", name)?;
    state
        .find_agent(name)
        .cloned()
        .ok_or_else(|| CamError::NotFound(format!("agent `{name}` does not exist")))
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AgentInspectedEventFields {
    pub agent: String,
    pub kind: &'static str,
    pub status: String,
    pub route: String,
    pub thread_id_present: bool,
    pub active_turn_id_present: bool,
    pub last_turn_id_present: bool,
    pub model_present: bool,
    pub model_provider_present: bool,
}

pub fn agent_inspected_event_fields(agent: &Agent) -> AgentInspectedEventFields {
    AgentInspectedEventFields {
        agent: agent.name.clone(),
        kind: format_agent_kind(&agent.kind),
        status: format!("{:?}", agent.status).to_lowercase(),
        route: format_route(&agent.route),
        thread_id_present: agent.thread_id.is_some(),
        active_turn_id_present: agent.active_turn_id.is_some(),
        last_turn_id_present: agent.last_turn_id.is_some(),
        model_present: agent.model.is_some(),
        model_provider_present: agent.model_provider.is_some(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTranscriptEvidence {
    pub checked: bool,
    pub available: bool,
    pub status: Option<String>,
    pub error_kind: Option<String>,
    pub transcript_source: Option<String>,
    pub latest_turn_id: Option<String>,
    pub summary: Option<String>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentReadOptions {
    pub latest_only: bool,
    pub include_turns: bool,
    pub turn_limit: Option<usize>,
    pub wait_seconds: Option<u64>,
}

impl AgentReadOptions {
    pub fn summary() -> Self {
        Self {
            latest_only: false,
            include_turns: false,
            turn_limit: None,
            wait_seconds: None,
        }
    }

    pub fn latest() -> Self {
        Self {
            latest_only: true,
            include_turns: false,
            turn_limit: None,
            wait_seconds: None,
        }
    }
}

impl AgentTranscriptEvidence {
    pub fn not_applicable() -> Self {
        Self {
            checked: false,
            available: false,
            status: None,
            error_kind: None,
            transcript_source: None,
            latest_turn_id: None,
            summary: None,
            warning: None,
        }
    }

    pub fn provider_transcript_unavailable(error: CamError) -> Self {
        Self {
            checked: false,
            available: false,
            status: None,
            error_kind: Some(error.kind().to_string()),
            transcript_source: None,
            latest_turn_id: None,
            summary: None,
            warning: None,
        }
    }

    pub fn unavailable_error(error: CamError) -> Self {
        Self {
            checked: true,
            available: false,
            status: Some("error".to_string()),
            error_kind: Some(error.kind().to_string()),
            transcript_source: Some("local_mailbox_only".to_string()),
            latest_turn_id: None,
            summary: None,
            warning: Some(format!("provider transcript read failed: {error}")),
        }
    }
}

pub fn read_agent_transcript_evidence(
    agent: &Agent,
    latest_only: bool,
    provider_transcript_reader: &dyn ProviderTranscriptReader,
) -> AgentTranscriptEvidence {
    if !matches!(agent.kind, AgentKind::Codex | AgentKind::AgySession) {
        return AgentTranscriptEvidence::not_applicable();
    }

    let request = match ProviderTranscriptRequest::from_agent(agent, latest_only) {
        Ok(request) => request,
        Err(error) => return AgentTranscriptEvidence::unavailable_error(error),
    };

    match provider_transcript_reader.read_transcript(agent, request.clone()) {
        Ok(outcome) => match validate_provider_transcript_outcome(&request, &outcome) {
            Ok(()) => provider_transcript_evidence_from_outcome(outcome),
            Err(error) => AgentTranscriptEvidence::unavailable_error(error),
        },
        Err(error) if is_transcript_unavailable_error(&error) => {
            AgentTranscriptEvidence::provider_transcript_unavailable(error)
        }
        Err(error) => AgentTranscriptEvidence::unavailable_error(error),
    }
}

fn read_agent_transcript_evidence_with_wait(
    agent: &Agent,
    options: AgentReadOptions,
    provider_transcript_reader: &dyn ProviderTranscriptReader,
) -> AgentTranscriptEvidence {
    let wait_seconds = options.wait_seconds.unwrap_or(0);
    let deadline = Instant::now() + Duration::from_secs(wait_seconds);
    let mut evidence =
        read_agent_transcript_evidence(agent, options.latest_only, provider_transcript_reader);

    while transcript_wait_should_continue(agent, &evidence, wait_seconds, deadline) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        std::thread::sleep(remaining.min(Duration::from_millis(500)));
        evidence =
            read_agent_transcript_evidence(agent, options.latest_only, provider_transcript_reader);
    }

    evidence
}

fn transcript_wait_should_continue(
    agent: &Agent,
    evidence: &AgentTranscriptEvidence,
    wait_seconds: u64,
    deadline: Instant,
) -> bool {
    wait_seconds > 0
        && matches!(agent.kind, AgentKind::Codex | AgentKind::AgySession)
        && Instant::now() < deadline
        && (!evidence.available
            || evidence
                .summary
                .as_deref()
                .map(str::trim)
                .unwrap_or_default()
                .is_empty())
}

fn provider_transcript_evidence_from_outcome(
    outcome: ProviderTranscriptOutcome,
) -> AgentTranscriptEvidence {
    AgentTranscriptEvidence {
        checked: outcome.provider_checked,
        available: outcome.transcript_available,
        status: Some(if outcome.transcript_available {
            "available".to_string()
        } else {
            "unavailable".to_string()
        }),
        error_kind: None,
        transcript_source: Some(if outcome.transcript_available {
            outcome.transcript_source
        } else {
            "local_mailbox_only".to_string()
        }),
        latest_turn_id: outcome.latest_turn_id,
        summary: outcome.summary,
        warning: outcome.warning,
    }
}

fn is_transcript_unavailable_error(error: &CamError) -> bool {
    matches!(error, CamError::ProviderUnavailable(message) if message.contains("transcript") && message.contains("not available"))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentReadSnapshot {
    pub agent: Agent,
    pub mailbox_messages: Vec<Message>,
    pub mailbox_message_count: usize,
    pub mailbox_messages_total: usize,
    pub mailbox_evidence_available: bool,
    pub latest_only: bool,
    pub include_turns: bool,
    pub requested_turn_limit: Option<usize>,
    pub requested_wait_seconds: Option<u64>,
    pub evidence_scope: String,
    pub snapshot_source: String,
    pub transcript_source: String,
    pub provider_transcript_checked: bool,
    pub provider_transcript_available: bool,
    pub provider_transcript_status: String,
    pub provider_transcript_error_kind: Option<String>,
    pub provider_transcript_latest_turn_id: Option<String>,
    pub provider_transcript_summary: Option<String>,
    pub provider_transcript_warning: Option<String>,
}

pub fn build_agent_read_snapshot(
    agent: Agent,
    mailbox_messages: Vec<Message>,
    mailbox_messages_total: usize,
    options: AgentReadOptions,
    transcript_evidence: AgentTranscriptEvidence,
) -> AgentReadSnapshot {
    let mailbox_message_count = mailbox_messages.len();
    let mailbox_evidence_available = mailbox_message_count > 0;
    let is_virtual_inbox = agent.kind == AgentKind::VirtualInbox;
    let default_transcript_source = if is_virtual_inbox {
        "virtual_inbox_mailbox"
    } else {
        "local_mailbox_only"
    };
    let default_provider_transcript_status = if is_virtual_inbox {
        "not_applicable"
    } else {
        "provider_transcript_unavailable"
    };
    let default_provider_transcript_warning = if transcript_evidence.available {
        None
    } else if is_virtual_inbox {
        Some(
            "virtual inbox transcript is local mailbox evidence; provider transcript reading is not applicable"
                .to_string(),
        )
    } else {
        Some(format!(
            "{} transcript reading is not available from a real provider primitive yet; this snapshot contains local CAM state and mailbox evidence only",
            format_agent_kind(&agent.kind)
        ))
    };
    let evidence_scope = if transcript_evidence.available {
        "local_cam_state_mailbox_and_provider_transcript"
    } else {
        "local_cam_state_and_mailbox"
    };
    let snapshot_source = if transcript_evidence.available {
        "local_state_mailbox_and_provider_transcript"
    } else {
        "local_state_and_mailbox"
    };

    AgentReadSnapshot {
        agent,
        mailbox_messages,
        mailbox_message_count,
        mailbox_messages_total,
        mailbox_evidence_available,
        latest_only: options.latest_only,
        include_turns: options.include_turns,
        requested_turn_limit: options.turn_limit,
        requested_wait_seconds: options.wait_seconds,
        evidence_scope: evidence_scope.to_string(),
        snapshot_source: snapshot_source.to_string(),
        transcript_source: transcript_evidence
            .transcript_source
            .unwrap_or_else(|| default_transcript_source.to_string()),
        provider_transcript_checked: transcript_evidence.checked,
        provider_transcript_available: transcript_evidence.available,
        provider_transcript_status: transcript_evidence
            .status
            .unwrap_or_else(|| default_provider_transcript_status.to_string()),
        provider_transcript_error_kind: transcript_evidence.error_kind,
        provider_transcript_latest_turn_id: transcript_evidence.latest_turn_id,
        provider_transcript_summary: transcript_evidence.summary,
        provider_transcript_warning: transcript_evidence
            .warning
            .or(default_provider_transcript_warning),
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AgentReadEventFields {
    pub agent: String,
    pub evidence_scope: String,
    pub snapshot_source: String,
    pub transcript_source: String,
    pub mailbox_messages: usize,
    pub mailbox_evidence_available: bool,
    pub mailbox_message_count: usize,
    pub mailbox_messages_total: usize,
    pub provider_transcript_checked: bool,
    pub latest_only: bool,
    pub include_turns: bool,
    pub requested_turn_limit: Option<usize>,
    pub requested_wait_seconds: Option<u64>,
    pub provider_transcript_available: bool,
    pub provider_transcript_status: String,
    pub provider_transcript_error_kind: Option<String>,
    pub provider_transcript_latest_turn_id: Option<String>,
}

pub fn agent_read_event_fields(snapshot: &AgentReadSnapshot) -> AgentReadEventFields {
    AgentReadEventFields {
        agent: snapshot.agent.name.clone(),
        evidence_scope: snapshot.evidence_scope.clone(),
        snapshot_source: snapshot.snapshot_source.clone(),
        transcript_source: snapshot.transcript_source.clone(),
        mailbox_messages: snapshot.mailbox_messages.len(),
        mailbox_evidence_available: snapshot.mailbox_evidence_available,
        mailbox_message_count: snapshot.mailbox_message_count,
        mailbox_messages_total: snapshot.mailbox_messages_total,
        provider_transcript_checked: snapshot.provider_transcript_checked,
        latest_only: snapshot.latest_only,
        include_turns: snapshot.include_turns,
        requested_turn_limit: snapshot.requested_turn_limit,
        requested_wait_seconds: snapshot.requested_wait_seconds,
        provider_transcript_available: snapshot.provider_transcript_available,
        provider_transcript_status: snapshot.provider_transcript_status.clone(),
        provider_transcript_error_kind: snapshot.provider_transcript_error_kind.clone(),
        provider_transcript_latest_turn_id: snapshot.provider_transcript_latest_turn_id.clone(),
    }
}

pub fn read_agent_snapshot(
    state: &AppState,
    name: &str,
    options: AgentReadOptions,
    provider_transcript_reader: &dyn ProviderTranscriptReader,
) -> Result<AgentReadSnapshot, CamError> {
    validate_canonical_name("agent name", name)?;
    let agent = state
        .find_agent(name)
        .ok_or_else(|| CamError::NotFound(format!("agent `{name}` does not exist")))?
        .clone();
    let mailbox = select_agent_mailbox(state, &agent.name, options);
    let provider_transcript_result =
        read_agent_transcript_evidence_with_wait(&agent, options, provider_transcript_reader);

    Ok(build_agent_read_snapshot(
        agent,
        mailbox.messages,
        mailbox.total,
        mailbox.options,
        provider_transcript_result,
    ))
}

pub fn read_agent_snapshot_with_peer_fetcher(
    state: &AppState,
    name: &str,
    options: AgentReadOptions,
    provider_transcript_reader: &dyn ProviderTranscriptReader,
    peer_agent_read_fetcher: &dyn PeerAgentReadFetcher,
) -> Result<AgentReadSnapshot, CamError> {
    validate_canonical_name("agent name", name)?;
    let agent = state
        .find_agent(name)
        .ok_or_else(|| CamError::NotFound(format!("agent `{name}` does not exist")))?
        .clone();
    if agent.kind != AgentKind::RemoteMirror {
        return read_agent_snapshot(state, name, options, provider_transcript_reader);
    }

    read_remote_mirror_agent_snapshot(state, agent, options, peer_agent_read_fetcher)
}

fn read_remote_mirror_agent_snapshot(
    state: &AppState,
    agent: Agent,
    options: AgentReadOptions,
    peer_agent_read_fetcher: &dyn PeerAgentReadFetcher,
) -> Result<AgentReadSnapshot, CamError> {
    let Route::Peer { peer_name } = &agent.route else {
        return Err(CamError::InvalidState(format!(
            "remote mirror agent `{}` is missing peer route",
            agent.name
        )));
    };
    let peer = state
        .find_peer(peer_name)
        .ok_or_else(|| CamError::NotFound(format!("peer `{peer_name}` does not exist")))?;
    let remote_agent_name = agent
        .name
        .strip_prefix(&format!("{peer_name}::"))
        .ok_or_else(|| {
            CamError::InvalidState(format!(
                "remote mirror agent `{}` is not namespaced for peer `{peer_name}`",
                agent.name
            ))
        })?;

    let remote_snapshot = peer_agent_read_fetcher
        .read_agent(peer, remote_agent_name, options)
        .map_err(|error| {
            if error.remote_command_attempted {
                CamError::PeerTransportFailed(format!(
                    "peer `{peer_name}` remote mirror read failed after remote command: {}",
                    error.error
                ))
            } else {
                CamError::PeerTransportFailed(format!(
                    "peer `{peer_name}` remote mirror read failed before remote command: {}",
                    error.error
                ))
            }
        })?;
    validate_remote_mirror_read_snapshot(&agent, remote_agent_name, &remote_snapshot)?;

    let mailbox = select_agent_mailbox(state, &agent.name, options);
    let evidence = AgentTranscriptEvidence {
        checked: true,
        available: true,
        status: Some("available".to_string()),
        error_kind: None,
        transcript_source: Some(format!("owning_peer_cam_over_ssh:{peer_name}")),
        latest_turn_id: remote_snapshot
            .provider_transcript_latest_turn_id
            .clone()
            .or_else(|| remote_snapshot.agent.last_turn_id.clone()),
        summary: remote_snapshot
            .provider_transcript_summary
            .clone()
            .or_else(|| Some(remote_mirror_summary(&remote_snapshot))),
        warning: remote_snapshot
            .provider_transcript_warning
            .clone()
            .map(|warning| format!("remote peer warning: {warning}")),
    };

    Ok(build_agent_read_snapshot(
        agent,
        mailbox.messages,
        mailbox.total,
        mailbox.options,
        evidence,
    ))
}

fn validate_remote_mirror_read_snapshot(
    local_agent: &Agent,
    remote_agent_name: &str,
    remote_snapshot: &AgentReadSnapshot,
) -> Result<(), CamError> {
    if remote_snapshot.agent.name != remote_agent_name {
        return Err(CamError::PeerProtocolViolation(format!(
            "remote agent read returned `{}` instead of `{remote_agent_name}`",
            remote_snapshot.agent.name
        )));
    }
    if remote_snapshot.agent.kind == AgentKind::RemoteMirror {
        return Err(CamError::PeerProtocolViolation(
            "remote agent read returned another remote mirror instead of an owned remote agent"
                .to_string(),
        ));
    }
    if !matches!(remote_snapshot.agent.route, Route::Local) {
        return Err(CamError::PeerProtocolViolation(format!(
            "remote agent read returned non-local route `{}` for `{remote_agent_name}`",
            format_route(&remote_snapshot.agent.route)
        )));
    }
    if remote_snapshot.agent.thread_id != local_agent.thread_id {
        return Err(CamError::PeerProtocolViolation(format!(
            "remote agent read attempted to remap thread identity for `{}`",
            local_agent.name
        )));
    }
    Ok(())
}

fn remote_mirror_summary(remote_snapshot: &AgentReadSnapshot) -> String {
    format!(
        "remote peer snapshot read for `{}`; remote_snapshot_source={}; remote_transcript_source={}; provider_status={}; mailbox_messages={}",
        remote_snapshot.agent.name,
        remote_snapshot.snapshot_source,
        remote_snapshot.transcript_source,
        remote_snapshot.provider_transcript_status,
        remote_snapshot.mailbox_message_count
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMailboxSelection {
    pub messages: Vec<Message>,
    pub total: usize,
    pub options: AgentReadOptions,
}

pub fn select_agent_mailbox(
    state: &AppState,
    agent_name: &str,
    options: AgentReadOptions,
) -> AgentMailboxSelection {
    let mut messages = state
        .mailbox
        .iter()
        .filter(|message| message.target_agent == agent_name)
        .cloned()
        .collect::<Vec<_>>();
    let total = messages.len();

    if options.latest_only && messages.len() > 1 {
        if let Some(latest) = messages.pop() {
            messages = vec![latest];
        }
    } else if let Some(limit) = options.turn_limit
        && messages.len() > limit
    {
        messages = messages.split_off(messages.len() - limit);
    }

    AgentMailboxSelection {
        messages,
        total,
        options,
    }
}

fn format_agent_kind(kind: &AgentKind) -> &'static str {
    match kind {
        AgentKind::Codex => "codex",
        AgentKind::VirtualInbox => "virtual_inbox",
        AgentKind::AgySession => "agy_session",
        AgentKind::RemoteMirror => "remote_mirror",
    }
}

pub fn read_mailbox(state: &AppState, agent_filter: Option<&str>) -> Vec<Message> {
    state
        .mailbox
        .iter()
        .filter(|message| {
            agent_filter
                .map(|agent| message.target_agent == agent)
                .unwrap_or(true)
        })
        .cloned()
        .collect()
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MailboxWaitResult {
    pub messages: Vec<Message>,
    pub wait_seconds: u64,
    pub timed_out: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct InboxReadEventFields {
    pub filter: Option<String>,
    pub message_count: usize,
    pub wait_seconds: u64,
    pub timed_out: bool,
}

pub fn inbox_read_event_fields(
    result: &MailboxWaitResult,
    filter: Option<&str>,
) -> InboxReadEventFields {
    InboxReadEventFields {
        filter: filter.map(str::to_string),
        message_count: result.messages.len(),
        wait_seconds: result.wait_seconds,
        timed_out: result.timed_out,
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct InboxListRow {
    pub message_id: String,
    pub target: String,
    pub delivery: &'static str,
    pub source: String,
    pub message_type: String,
    pub body: String,
}

pub fn inbox_list_rows(messages: &[Message]) -> Vec<InboxListRow> {
    messages
        .iter()
        .map(|message| InboxListRow {
            message_id: message.message_id.clone(),
            target: message.target_agent.clone(),
            delivery: delivery_state_label(&message.delivery),
            source: message
                .source_agent
                .clone()
                .unwrap_or_else(|| "-".to_string()),
            message_type: message
                .message_type
                .clone()
                .unwrap_or_else(|| "-".to_string()),
            body: message.body.clone(),
        })
        .collect()
}

pub fn read_mailbox_with_wait(
    store: &StateStore,
    agent_filter: Option<&str>,
    wait_seconds: u64,
    poll_interval: Duration,
) -> Result<MailboxWaitResult, CamError> {
    let deadline = Instant::now() + Duration::from_secs(wait_seconds);
    let mut state = store.load_existing()?;
    let mut messages = read_mailbox(&state, agent_filter);

    while messages.is_empty() && wait_seconds > 0 && Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        std::thread::sleep(remaining.min(poll_interval));
        state = store.load_existing()?;
        messages = read_mailbox(&state, agent_filter);
    }

    Ok(MailboxWaitResult {
        timed_out: messages.is_empty() && wait_seconds > 0,
        messages,
        wait_seconds,
    })
}

pub fn read_logs(
    store: &StateStore,
    limit: Option<usize>,
) -> Result<Vec<StoredLogEvent>, CamError> {
    read_events(store, limit)
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LogListRow {
    pub at: String,
    pub event_type: String,
    pub message: String,
}

pub fn log_list_rows(events: &[StoredLogEvent]) -> Vec<LogListRow> {
    events
        .iter()
        .map(|event| LogListRow {
            at: event.at.clone(),
            event_type: event.event_type.clone(),
            message: event.message.clone(),
        })
        .collect()
}

pub fn parse_log_limit(value: &str) -> Result<usize, CamError> {
    let limit = value.parse::<usize>().map_err(|_| {
        CamError::InvalidCommand(format!("--limit `{value}` must be a positive integer"))
    })?;
    if limit == 0 {
        return Err(CamError::InvalidCommand(
            "--limit must be greater than zero".to_string(),
        ));
    }
    Ok(limit)
}

pub fn parse_agent_read_turn_limit(value: &str) -> Result<usize, CamError> {
    let limit = value.parse::<usize>().map_err(|_| {
        CamError::InvalidCommand(format!("--turns `{value}` must be a positive integer"))
    })?;
    if limit == 0 {
        return Err(CamError::InvalidCommand(
            "--turns must be greater than zero".to_string(),
        ));
    }
    if limit > MAX_AGENT_READ_TURNS {
        return Err(CamError::InvalidCommand(format!(
            "--turns must be {MAX_AGENT_READ_TURNS} or less"
        )));
    }
    Ok(limit)
}

pub fn parse_agent_read_wait_seconds(value: &str) -> Result<u64, CamError> {
    let seconds = value.parse::<u64>().map_err(|_| {
        CamError::InvalidCommand(format!(
            "--wait-seconds `{value}` must be a non-negative integer"
        ))
    })?;
    if seconds > 300 {
        return Err(CamError::InvalidCommand(
            "--wait-seconds must be 300 seconds or less".to_string(),
        ));
    }
    Ok(seconds)
}

pub fn parse_inbox_wait_seconds(value: &str) -> Result<u64, CamError> {
    let seconds = value.parse::<u64>().map_err(|_| {
        CamError::InvalidCommand(format!("--wait `{value}` must be a non-negative integer"))
    })?;
    if seconds > 300 {
        return Err(CamError::InvalidCommand(
            "--wait must be 300 seconds or less".to_string(),
        ));
    }
    Ok(seconds)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDiscoveryApplication {
    pub summary: DiscoverySummary,
    pub source_path: Option<PathBuf>,
    pub persist_state: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DesktopArchiveMergeSummary {
    pub attempted: bool,
    pub source_path: Option<String>,
    pub source_paths: Vec<String>,
    pub evidence_rows: usize,
    pub remote_mirrors_seen: usize,
    pub matched: usize,
    pub applied: usize,
    pub conflicts: usize,
    pub unmatched: usize,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DiscoveryLocalScannedEventFields {
    pub scanner: String,
    pub source_path: Option<String>,
    pub rows_discovered: usize,
    pub approved: usize,
    pub candidate: usize,
    pub quarantined: usize,
    pub rejected: usize,
    pub promoted: usize,
    pub skipped_existing_thread: usize,
    pub skipped_name_collision: usize,
    pub skipped_not_approved: usize,
    pub skipped_promotion_not_requested: usize,
    pub skipped_invalid_after_reclassify: usize,
    pub promotion_decisions: Vec<PromotionDecision>,
    pub warning: Option<String>,
}

pub fn discovery_local_scanned_event_fields(
    application: &LocalDiscoveryApplication,
) -> DiscoveryLocalScannedEventFields {
    let summary = &application.summary;
    DiscoveryLocalScannedEventFields {
        scanner: summary.scanner.clone(),
        source_path: application
            .source_path
            .as_ref()
            .map(|path| path.display().to_string()),
        rows_discovered: summary.rows_discovered,
        approved: summary.approved,
        candidate: summary.candidate,
        quarantined: summary.quarantined,
        rejected: summary.rejected,
        promoted: summary.promoted,
        skipped_existing_thread: summary.skipped_existing_thread,
        skipped_name_collision: summary.skipped_name_collision,
        skipped_not_approved: summary.skipped_not_approved,
        skipped_promotion_not_requested: summary.skipped_promotion_not_requested,
        skipped_invalid_after_reclassify: summary.skipped_invalid_after_reclassify,
        promotion_decisions: summary.promotion_decisions.clone(),
        warning: summary.warning.clone(),
    }
}

pub fn run_local_discovery(
    state: &mut AppState,
    requested_codex_home: Option<PathBuf>,
    promote_approved: bool,
) -> Result<LocalDiscoveryApplication, CamError> {
    let Some(codex_home) = requested_codex_home.or_else(default_codex_home) else {
        return Ok(LocalDiscoveryApplication {
            summary: DiscoverySummary::from_rows(
                &[],
                Vec::new(),
                "codex_local_sources",
                Some(
                    "no Codex home could be resolved from --codex-home, CODEX_HOME, USERPROFILE, or HOME"
                        .to_string(),
                ),
            ),
            source_path: None,
            persist_state: false,
        });
    };

    let (rows, warning) = discover_local_codex(&codex_home)?;
    state.discovery_rows = rows;
    refresh_local_agent_chat_status_from_discovery(state);
    let desktop_archive_merge =
        merge_desktop_archive_evidence_into_remote_mirrors(state, Some(&codex_home));
    let promotion_decisions = if promote_approved {
        promote_approved_discovery_rows(state)
    } else {
        discovery_promotion_not_requested_decisions(&state.discovery_rows)
    };
    let mut summary = DiscoverySummary::from_rows(
        &state.discovery_rows,
        promotion_decisions,
        "codex_local_sources",
        warning,
    );
    summary.desktop_archive_merge =
        Some(serde_json::to_value(&desktop_archive_merge).unwrap_or_else(
            |_| serde_json::json!({"warning":"failed to serialize Desktop archive merge summary"}),
        ));

    Ok(LocalDiscoveryApplication {
        summary,
        source_path: Some(codex_home),
        persist_state: true,
    })
}

pub fn discovery_promotion_not_requested_decisions(
    rows: &[DiscoveryRow],
) -> Vec<PromotionDecision> {
    rows.iter()
        .enumerate()
        .map(|(row_index, row)| {
            let row = classify_row(row.clone());
            let generated_name = if row.disposition == DiscoveryDisposition::Approved {
                Some(discovery_agent_name(&row.title))
            } else {
                None
            };
            let (decision, reason) = if row.disposition == DiscoveryDisposition::Approved {
                (
                    PromotionDecisionKind::SkippedPromotionNotRequested,
                    "approved discovery row was not promoted because promotion was not requested"
                        .to_string(),
                )
            } else {
                (
                    PromotionDecisionKind::SkippedNotApproved,
                    format!(
                        "discovery row disposition `{}` is not promotable",
                        format_discovery_disposition(&row.disposition)
                    ),
                )
            };
            promotion_decision(row_index, &row, generated_name, None, decision, reason)
        })
        .collect()
}

pub fn apply_agent_resume_success(
    state: &mut AppState,
    name: &str,
    result: &AgentResumeResult,
) -> Result<Agent, CamError> {
    let agent = state
        .find_agent_mut(name)
        .ok_or_else(|| CamError::NotFound(format!("agent `{name}` does not exist")))?;
    agent.thread_id = result.thread_id.clone();
    agent.status = if result.active_turn_id.is_some() || result.last_turn_id.is_some() {
        AgentStatus::Active
    } else {
        AgentStatus::Idle
    };
    agent.active_turn_id = result
        .active_turn_id
        .clone()
        .or_else(|| result.last_turn_id.clone());
    agent.last_turn_id = result
        .last_turn_id
        .clone()
        .or_else(|| result.active_turn_id.clone());
    agent.last_error = None;
    agent.updated_at = now_utc();
    Ok(agent.clone())
}

pub fn apply_codex_thread_start_success(
    state: &mut AppState,
    name: &str,
    outcome: CodexThreadStartOutcome,
) -> Result<AgentResumeApplication, CamError> {
    let agent = state
        .find_agent_mut(name)
        .ok_or_else(|| CamError::NotFound(format!("agent `{name}` does not exist")))?;
    agent.thread_id = Some(outcome.thread_id.clone());
    agent.status = match outcome.status.as_str() {
        "active" => AgentStatus::Active,
        _ => AgentStatus::Idle,
    };
    agent.active_turn_id = None;
    agent.last_error = None;
    agent.updated_at = now_utc();

    let result = AgentResumeResult {
        ok: true,
        implemented: true,
        agent: agent.name.clone(),
        kind: "codex".to_string(),
        route: "local".to_string(),
        thread_id: Some(outcome.thread_id),
        active_turn_id: None,
        last_turn_id: None,
        ready: true,
        provider_checked: false,
        provider_resumed: true,
        readiness_ready: None,
        readiness_error: None,
        resume_attempted: true,
        message: "Codex provider thread/start created a local thread binding".to_string(),
        warning: None,
        error: None,
    };
    Ok(AgentResumeApplication {
        result,
        persist_state: true,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentResumeApplication {
    pub result: AgentResumeResult,
    pub persist_state: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AgentResumeEventFields {
    pub agent: String,
    pub kind: String,
    pub route: String,
    pub thread_id_present: bool,
    pub ok: bool,
    pub implemented: bool,
    pub ready: bool,
    pub provider_checked: bool,
    pub provider_resumed: bool,
    pub readiness_ready: Option<bool>,
    pub readiness_error: Option<String>,
    pub resume_attempted: bool,
    pub active_turn_id: Option<String>,
    pub last_turn_id: Option<String>,
    pub transcript_mutated: bool,
    pub reason: String,
    pub error: Option<String>,
    pub warning: Option<String>,
}

pub fn agent_resume_event_fields(result: &AgentResumeResult) -> AgentResumeEventFields {
    AgentResumeEventFields {
        agent: result.agent.clone(),
        kind: result.kind.clone(),
        route: result.route.clone(),
        thread_id_present: result.thread_id.is_some(),
        ok: result.ok,
        implemented: result.implemented,
        ready: result.ready,
        provider_checked: result.provider_checked,
        provider_resumed: result.provider_resumed,
        readiness_ready: result.readiness_ready,
        readiness_error: result.readiness_error.clone(),
        resume_attempted: result.resume_attempted,
        active_turn_id: result.active_turn_id.clone(),
        last_turn_id: result.last_turn_id.clone(),
        transcript_mutated: false,
        reason: result.message.clone(),
        error: result.error.clone(),
        warning: result.warning.clone(),
    }
}

pub fn resume_agent(
    state: &mut AppState,
    name: &str,
    provider_lifecycle_router: &dyn ProviderLifecycleRouter,
) -> Result<AgentResumeApplication, CamError> {
    validate_canonical_name("agent name", name)?;
    let agent = state
        .find_agent(name)
        .ok_or_else(|| CamError::NotFound(format!("agent `{name}` does not exist")))?
        .clone();

    if !should_attempt_provider_resume(&agent) {
        return Ok(AgentResumeApplication {
            result: AgentResumeResult::for_agent(&agent),
            persist_state: false,
        });
    }

    let result = match resume_with_readiness_check(&agent, provider_lifecycle_router) {
        Ok(result) => result,
        Err(error) => result_from_lifecycle_error(&agent, error),
    };
    let persist_state = result.ready;
    if persist_state {
        apply_agent_resume_success(state, name, &result)?;
    }

    Ok(AgentResumeApplication {
        result,
        persist_state,
    })
}

pub fn resume_agent_with_peer_fetcher(
    state: &mut AppState,
    name: &str,
    provider_lifecycle_router: &dyn ProviderLifecycleRouter,
    peer_agent_resume_fetcher: &dyn PeerAgentResumeFetcher,
) -> Result<AgentResumeApplication, CamError> {
    validate_canonical_name("agent name", name)?;
    let agent = state
        .find_agent(name)
        .ok_or_else(|| CamError::NotFound(format!("agent `{name}` does not exist")))?
        .clone();
    if agent.kind != AgentKind::RemoteMirror {
        return resume_agent(state, name, provider_lifecycle_router);
    }

    resume_remote_mirror_agent(state, agent, peer_agent_resume_fetcher)
}

fn daemon_codex_owner_ready_resume_result(agent: &Agent) -> AgentResumeResult {
    AgentResumeResult {
        ok: true,
        implemented: true,
        agent: agent.name.clone(),
        kind: format_agent_kind(&agent.kind).to_string(),
        route: format_route(&agent.route),
        thread_id: agent.thread_id.clone(),
        active_turn_id: agent.active_turn_id.clone(),
        last_turn_id: agent.last_turn_id.clone(),
        ready: true,
        provider_checked: true,
        provider_resumed: false,
        readiness_ready: Some(true),
        readiness_error: None,
        resume_attempted: false,
        message:
            "daemon owns a live Codex app-server for this thread; no provider resume was needed"
                .to_string(),
        warning: None,
        error: None,
    }
}

fn daemon_codex_owner_missing_turn_proof_resume_result(agent: &Agent) -> AgentResumeResult {
    let mut result = AgentResumeResult::for_agent(agent);
    result.provider_checked = true;
    result.readiness_ready = Some(false);
    result.readiness_error =
        Some("daemon owns the Codex app-server but local state has no turn proof".to_string());
    result.resume_attempted = false;
    result.warning = Some(
        "daemon owner presence alone is not enough; readiness requires active_turn_id or last_turn_id proof"
            .to_string(),
    );
    result.error = Some("missing Codex turn proof for daemon-owned thread".to_string());
    result
}

fn resume_remote_mirror_agent(
    state: &mut AppState,
    agent: Agent,
    peer_agent_resume_fetcher: &dyn PeerAgentResumeFetcher,
) -> Result<AgentResumeApplication, CamError> {
    let Route::Peer { peer_name } = &agent.route else {
        return Err(CamError::InvalidState(format!(
            "remote mirror agent `{}` is missing peer route",
            agent.name
        )));
    };
    let peer = state
        .find_peer(peer_name)
        .ok_or_else(|| CamError::NotFound(format!("peer `{peer_name}` does not exist")))?;
    let remote_agent_name = agent
        .name
        .strip_prefix(&format!("{peer_name}::"))
        .ok_or_else(|| {
            CamError::InvalidState(format!(
                "remote mirror agent `{}` is not namespaced for peer `{peer_name}`",
                agent.name
            ))
        })?;

    let remote_result = peer_agent_resume_fetcher
        .resume_agent(peer, remote_agent_name)
        .map_err(|error| {
            if error.remote_command_attempted {
                CamError::PeerTransportFailed(format!(
                    "peer `{peer_name}` remote mirror resume failed after remote command: {}",
                    error.error
                ))
            } else {
                CamError::PeerTransportFailed(format!(
                    "peer `{peer_name}` remote mirror resume failed before remote command: {}",
                    error.error
                ))
            }
        })?;
    validate_remote_mirror_resume_result(&agent, remote_agent_name, &remote_result)?;
    let result = localize_remote_mirror_resume_result(&agent, &remote_result);
    let persist_state = result.ready;
    if persist_state {
        apply_agent_resume_success(state, &agent.name, &result)?;
    }

    Ok(AgentResumeApplication {
        result,
        persist_state,
    })
}

fn validate_remote_mirror_resume_result(
    local_agent: &Agent,
    remote_agent_name: &str,
    remote_result: &AgentResumeResult,
) -> Result<(), CamError> {
    if remote_result.agent != remote_agent_name {
        return Err(CamError::PeerProtocolViolation(format!(
            "remote agent resume returned `{}` instead of `{remote_agent_name}`",
            remote_result.agent
        )));
    }
    if remote_result.kind == "remote_mirror" {
        return Err(CamError::PeerProtocolViolation(
            "remote agent resume returned another remote mirror instead of an owned remote agent"
                .to_string(),
        ));
    }
    if remote_result.route != "local" {
        return Err(CamError::PeerProtocolViolation(format!(
            "remote agent resume returned non-local route `{}` for `{remote_agent_name}`",
            remote_result.route
        )));
    }
    if remote_result.thread_id != local_agent.thread_id {
        return Err(CamError::PeerProtocolViolation(format!(
            "remote agent resume attempted to remap thread identity for `{}`",
            local_agent.name
        )));
    }
    if remote_result.ok && !remote_result.ready {
        return Err(CamError::PeerProtocolViolation(format!(
            "remote agent resume returned ok=true without readiness for `{remote_agent_name}`"
        )));
    }
    if remote_result.ok && remote_result.error.is_some() {
        return Err(CamError::PeerProtocolViolation(format!(
            "remote agent resume returned ok=true with error text for `{remote_agent_name}`"
        )));
    }
    if remote_result.ready
        && remote_result.active_turn_id.is_none()
        && remote_result.last_turn_id.is_none()
    {
        return Err(CamError::PeerProtocolViolation(format!(
            "remote agent resume claimed readiness for `{remote_agent_name}` without turn proof"
        )));
    }
    if remote_result.provider_resumed
        && (!remote_result.resume_attempted || !remote_result.provider_checked)
    {
        return Err(CamError::PeerProtocolViolation(format!(
            "remote agent resume reported provider_resumed without provider check and resume attempt for `{remote_agent_name}`"
        )));
    }
    Ok(())
}

fn localize_remote_mirror_resume_result(
    local_agent: &Agent,
    remote_result: &AgentResumeResult,
) -> AgentResumeResult {
    AgentResumeResult {
        ok: remote_result.ok,
        implemented: true,
        agent: local_agent.name.clone(),
        kind: "remote_mirror".to_string(),
        route: format_route(&local_agent.route),
        thread_id: local_agent.thread_id.clone(),
        active_turn_id: remote_result.active_turn_id.clone(),
        last_turn_id: remote_result.last_turn_id.clone(),
        ready: remote_result.ready,
        provider_checked: true,
        provider_resumed: remote_result.provider_resumed || remote_result.resume_attempted,
        readiness_ready: remote_result.readiness_ready,
        readiness_error: remote_result.readiness_error.clone(),
        resume_attempted: true,
        message: format!(
            "owning peer CAM resumed remote agent `{}`: {}",
            remote_result.agent, remote_result.message
        ),
        warning: remote_result
            .warning
            .clone()
            .map(|warning| format!("remote peer warning: {warning}")),
        error: remote_result.error.clone(),
    }
}

pub fn validate_state_identity_invariants(state: &AppState) -> Result<(), CamError> {
    if state.find_agent("operator").is_none() {
        return Err(CamError::InvalidState(
            "built-in `operator` agent is missing".to_string(),
        ));
    }
    validate_daemon_identity_state(&state.daemon)?;

    validate_name_hygiene(state)?;
    ensure_unique_names(
        state.agents.iter().map(|agent| agent.name.as_str()),
        "agent names",
    )?;
    ensure_unique_names(
        state.peers.iter().map(|peer| peer.name.as_str()),
        "peer names",
    )?;
    Ok(())
}

fn validate_daemon_identity_state(daemon: &DaemonState) -> Result<(), CamError> {
    match (&daemon.instance_id, &daemon.identity_nonce_ref) {
        (Some(instance_id), Some(identity_nonce_ref)) => {
            if Uuid::parse_str(instance_id).is_err() {
                return Err(CamError::InvalidState(
                    "daemon instance_id must be a UUID".to_string(),
                ));
            }
            validate_daemon_identity_nonce_ref(identity_nonce_ref)?;
            let expected = daemon_identity_nonce_ref(instance_id);
            if identity_nonce_ref != &expected {
                return Err(CamError::InvalidState(
                    "daemon identity_nonce_ref must match instance_id".to_string(),
                ));
            }
        }
        (None, None) => {}
        (Some(_), None) => {
            return Err(CamError::InvalidState(
                "daemon instance_id requires identity_nonce_ref".to_string(),
            ));
        }
        (None, Some(_)) => {
            return Err(CamError::InvalidState(
                "daemon identity_nonce_ref requires instance_id".to_string(),
            ));
        }
    }
    Ok(())
}

pub fn validate_state_invariants(state: &AppState) -> Result<(), CamError> {
    validate_state_identity_invariants(state)?;
    validate_state_peer_invariants(state)?;
    validate_state_agent_shape_invariants(state)?;
    validate_state_thread_mapping_invariants(state)?;
    validate_state_mailbox_invariants(state)?;
    validate_state_discovery_invariants(state)?;
    Ok(())
}

pub fn validate_state_peer_invariants(state: &AppState) -> Result<(), CamError> {
    for peer in &state.peers {
        if peer.transport == PeerTransport::Ssh {
            let target = peer.ssh_target.as_deref().unwrap_or_default();
            if target.trim().is_empty()
                || target.contains(char::is_whitespace)
                || !target.contains('@')
            {
                return Err(CamError::InvalidState(format!(
                    "ssh peer `{}` has invalid ssh target",
                    peer.name
                )));
            }
        }
        if matches!(
            peer.state,
            PeerState::Mirrored | PeerState::MirroredDegraded
        ) {
            if peer
                .remote_node_name
                .as_deref()
                .unwrap_or_default()
                .is_empty()
                || peer.last_sync_at.as_deref().unwrap_or_default().is_empty()
                || peer.inventory_source.as_deref() != Some("inventory_export")
            {
                return Err(CamError::InvalidState(format!(
                    "mirrored peer `{}` is missing inventory sync metadata",
                    peer.name
                )));
            }
        }
        if peer.state == PeerState::SyncFailed {
            if peer
                .last_sync_error
                .as_deref()
                .unwrap_or_default()
                .is_empty()
            {
                return Err(CamError::InvalidState(format!(
                    "sync_failed peer `{}` is missing last_sync_error",
                    peer.name
                )));
            }
            let error_kind = peer.last_sync_error_kind.as_deref().unwrap_or_default();
            if error_kind.is_empty() {
                return Err(CamError::InvalidState(format!(
                    "sync_failed peer `{}` is missing last_sync_error_kind",
                    peer.name
                )));
            }
            validate_error_kind_token(error_kind).map_err(|message| {
                CamError::InvalidState(format!(
                    "sync_failed peer `{}` has invalid last_sync_error_kind: {message}",
                    peer.name
                ))
            })?;
        }
    }
    Ok(())
}

pub fn validate_agent_turn_tokens(agent: &Agent) -> Result<(), CamError> {
    if has_noncanonical_id(agent.active_turn_id.as_deref()) {
        return Err(CamError::InvalidState(format!(
            "agent `{}` has non-canonical active turn id",
            agent.name
        )));
    }
    if has_noncanonical_id(agent.last_turn_id.as_deref()) {
        return Err(CamError::InvalidState(format!(
            "agent `{}` has non-canonical last turn id",
            agent.name
        )));
    }
    if agent.status == AgentStatus::Active && agent.active_turn_id.is_none() {
        return Err(CamError::InvalidState(format!(
            "agent `{}` is active without active turn proof",
            agent.name
        )));
    }
    if (agent.active_turn_id.is_some() || agent.last_turn_id.is_some()) && agent.thread_id.is_none()
    {
        return Err(CamError::InvalidState(format!(
            "agent `{}` has turn proof without thread/session identity",
            agent.name
        )));
    }
    Ok(())
}

pub fn validate_state_thread_mapping_invariants(state: &AppState) -> Result<(), CamError> {
    let mut seen = BTreeSet::new();
    for agent in &state.agents {
        let Some(thread_id) = agent.thread_id.as_deref() else {
            continue;
        };
        let thread_id = thread_id.trim();
        if thread_id.is_empty() {
            continue;
        }
        let key = (
            thread_mapping_route_key(&agent.route),
            thread_id.to_string(),
        );
        if !seen.insert(key) {
            return Err(CamError::InvalidState(format!(
                "duplicate thread mapping for `{thread_id}`"
            )));
        }
    }
    Ok(())
}

pub fn validate_state_mailbox_invariants(state: &AppState) -> Result<(), CamError> {
    let mut seen_message_ids = BTreeSet::new();
    for message in &state.mailbox {
        if !has_canonical_id(Some(&message.message_id)) {
            return Err(CamError::InvalidState(
                "mailbox message id must be nonempty and trimmed".to_string(),
            ));
        }
        let parsed_message_id = Uuid::parse_str(&message.message_id).map_err(|_| {
            CamError::InvalidState(format!(
                "mailbox message id `{}` is not a valid UUID",
                message.message_id
            ))
        })?;
        if parsed_message_id.to_string() != message.message_id {
            return Err(CamError::InvalidState(format!(
                "mailbox message id `{}` is not canonical",
                message.message_id
            )));
        }
        if !seen_message_ids.insert(message.message_id.as_str()) {
            return Err(CamError::InvalidState(format!(
                "duplicate mailbox message id `{}`",
                message.message_id
            )));
        }
        if !is_canonical_state_name(&message.target_agent) {
            return Err(CamError::InvalidState(format!(
                "mailbox message `{}` has invalid target agent `{}`",
                message.message_id, message.target_agent
            )));
        }
        if let Some(source_agent) = &message.source_agent {
            if !is_canonical_state_name(source_agent) {
                return Err(CamError::InvalidState(format!(
                    "mailbox message `{}` has invalid source agent `{}`",
                    message.message_id, source_agent
                )));
            }
        }
        validate_optional_mailbox_trimmed(
            message.correlation_id.as_deref(),
            "correlation_id",
            &message.message_id,
        )?;
        validate_optional_mailbox_trimmed(
            message.message_type.as_deref(),
            "message_type",
            &message.message_id,
        )?;
        validate_optional_mailbox_trimmed(
            message.source_node.as_deref(),
            "source_node",
            &message.message_id,
        )?;
        validate_optional_mailbox_trimmed(
            message.thread_id.as_deref(),
            "thread_id",
            &message.message_id,
        )?;
        validate_optional_mailbox_trimmed(
            message.turn_id.as_deref(),
            "turn_id",
            &message.message_id,
        )?;

        if message
            .source_node
            .as_deref()
            .unwrap_or_default()
            .is_empty()
        {
            return Err(CamError::InvalidState(format!(
                "mailbox message `{}` is missing source_node",
                message.message_id
            )));
        }
        if message.body.trim().is_empty() {
            return Err(CamError::InvalidState(format!(
                "mailbox message `{}` has empty body",
                message.message_id
            )));
        }
        if message.created_at.trim().is_empty() || message.updated_at.trim().is_empty() {
            return Err(CamError::InvalidState(format!(
                "mailbox message `{}` is missing timestamps",
                message.message_id
            )));
        }
        let created_at =
            parse_mailbox_timestamp(&message.created_at, "created_at", &message.message_id)?;
        let updated_at =
            parse_mailbox_timestamp(&message.updated_at, "updated_at", &message.message_id)?;
        if updated_at < created_at {
            return Err(CamError::InvalidState(format!(
                "mailbox message `{}` has updated_at before created_at",
                message.message_id
            )));
        }
        if let Some(target) = state.find_agent(&message.target_agent) {
            if let Some(target_thread_id) = target.thread_id.as_deref() {
                if message.thread_id.as_deref() != Some(target_thread_id) {
                    return Err(CamError::InvalidState(format!(
                        "mailbox message `{}` thread id does not match target agent `{}`",
                        message.message_id, message.target_agent
                    )));
                }
            }
        }

        match message.delivery {
            DeliveryState::Received => {
                let Some(target) = state.find_agent(&message.target_agent) else {
                    return Err(CamError::InvalidState(format!(
                        "received mailbox message `{}` targets unknown agent `{}`",
                        message.message_id, message.target_agent
                    )));
                };
                if target.kind != AgentKind::VirtualInbox {
                    return Err(CamError::InvalidState(format!(
                        "received mailbox message `{}` targets non-virtual inbox `{}`",
                        message.message_id, message.target_agent
                    )));
                }
                if message.error.is_some() {
                    return Err(CamError::InvalidState(format!(
                        "received mailbox message `{}` must not carry an error",
                        message.message_id
                    )));
                }
            }
            DeliveryState::Queued => {
                if message.strict {
                    return Err(CamError::InvalidState(format!(
                        "strict mailbox message `{}` cannot be queued",
                        message.message_id
                    )));
                }
                if message
                    .error
                    .as_deref()
                    .unwrap_or_default()
                    .trim()
                    .is_empty()
                {
                    return Err(CamError::InvalidState(format!(
                        "queued mailbox message `{}` is missing queue reason",
                        message.message_id
                    )));
                }
                if message
                    .error
                    .as_deref()
                    .is_some_and(|error| error.trim() != error)
                {
                    return Err(CamError::InvalidState(format!(
                        "queued mailbox message `{}` has non-canonical queue reason",
                        message.message_id
                    )));
                }
            }
            DeliveryState::Started
            | DeliveryState::Steered
            | DeliveryState::Delivered
            | DeliveryState::Failed => {
                return Err(CamError::InvalidState(format!(
                    "mailbox message `{}` has non-durable mailbox delivery state `{}`",
                    message.message_id,
                    delivery_state_label(&message.delivery)
                )));
            }
        }
    }
    Ok(())
}

pub fn validate_state_discovery_invariants(state: &AppState) -> Result<(), CamError> {
    for row in &state.discovery_rows {
        if row.title.trim().is_empty() {
            return Err(CamError::InvalidState(
                "discovery row title cannot be empty".to_string(),
            ));
        }
        if row.reason.trim().is_empty() {
            return Err(CamError::InvalidState(format!(
                "discovery row `{}` is missing a classification reason",
                row.title
            )));
        }
        validate_discovery_route_coherence(row, state)?;
        validate_discovery_classifier_determinism(row)?;
        validate_discovery_promotion_boundary(row, state)?;

        if row.disposition == DiscoveryDisposition::Approved {
            if row
                .thread_id
                .as_deref()
                .unwrap_or_default()
                .trim()
                .is_empty()
            {
                return Err(CamError::InvalidState(format!(
                    "approved discovery row `{}` is missing thread/session id",
                    row.title
                )));
            }
            if row.cwd.as_deref().unwrap_or_default().trim().is_empty() {
                return Err(CamError::InvalidState(format!(
                    "approved discovery row `{}` is missing local workspace path",
                    row.title
                )));
            }
            if !row.workspace_in_project {
                return Err(CamError::InvalidState(format!(
                    "approved discovery row `{}` is not proven in-project",
                    row.title
                )));
            }
            if !matches!(row.route, Route::Local) {
                return Err(CamError::InvalidState(format!(
                    "approved discovery row `{}` must use local route",
                    row.title
                )));
            }
            if row.thread_source != ThreadSource::Codex {
                return Err(CamError::InvalidState(format!(
                    "approved discovery row `{}` has non-trusted discovery thread source",
                    row.title
                )));
            }
            if row.source == DiscoverySource::RemoteInventory {
                return Err(CamError::InvalidState(format!(
                    "remote inventory discovery row `{}` cannot be approved locally",
                    row.title
                )));
            }
            if !matches!(
                row.source,
                DiscoverySource::Rollout | DiscoverySource::ThreadDatabase
            ) {
                return Err(CamError::InvalidState(format!(
                    "approved discovery row `{}` must be backed by Codex thread metadata",
                    row.title
                )));
            }
        }

        if row.source == DiscoverySource::RemoteInventory {
            let Route::Peer { peer_name } = &row.route else {
                return Err(CamError::InvalidState(format!(
                    "remote inventory discovery row `{}` must use peer route",
                    row.title
                )));
            };
            if row.peer_name.as_deref() != Some(peer_name.as_str()) {
                return Err(CamError::InvalidState(format!(
                    "remote inventory discovery row `{}` has mismatched peer metadata",
                    row.title
                )));
            }
            if state.find_peer(peer_name).is_none() {
                return Err(CamError::InvalidState(format!(
                    "remote inventory discovery row `{}` references unknown peer `{}`",
                    row.title, peer_name
                )));
            }
            if row.thread_source != ThreadSource::RemoteMirror {
                return Err(CamError::InvalidState(format!(
                    "remote inventory discovery row `{}` must use remote mirror thread source",
                    row.title
                )));
            }
            if row.disposition == DiscoveryDisposition::Approved {
                return Err(CamError::InvalidState(format!(
                    "remote inventory discovery row `{}` cannot be approved locally",
                    row.title
                )));
            }
        }
    }
    Ok(())
}

pub fn validate_state_agent_shape_invariants(state: &AppState) -> Result<(), CamError> {
    let known_peer_names = state
        .peers
        .iter()
        .map(|peer| peer.name.clone())
        .collect::<BTreeSet<_>>();
    for agent in &state.agents {
        validate_agent_shape_invariants(agent, &known_peer_names)?;
        validate_agent_turn_tokens(agent)?;
    }
    Ok(())
}

pub fn promote_approved_discovery_rows(state: &mut AppState) -> Vec<PromotionDecision> {
    let rows = state.discovery_rows.clone();
    let mut decisions = Vec::new();

    for (row_index, row) in rows.into_iter().enumerate() {
        let row = classify_row(row);
        let generated_name = if row.disposition == DiscoveryDisposition::Approved {
            Some(discovery_agent_name(&row.title))
        } else {
            None
        };
        if row.disposition != DiscoveryDisposition::Approved {
            decisions.push(promotion_decision(
                row_index,
                &row,
                generated_name,
                None,
                PromotionDecisionKind::SkippedNotApproved,
                format!(
                    "discovery row disposition `{}` is not promotable",
                    format_discovery_disposition(&row.disposition)
                ),
            ));
            continue;
        }
        let Some(thread_id) = row.thread_id.clone() else {
            decisions.push(promotion_decision(
                row_index,
                &row,
                generated_name,
                None,
                PromotionDecisionKind::SkippedInvalidAfterReclassify,
                "approved discovery row is missing thread/session id after reclassification"
                    .to_string(),
            ));
            continue;
        };
        let Some(cwd) = row.cwd.clone() else {
            decisions.push(promotion_decision(
                row_index,
                &row,
                generated_name,
                None,
                PromotionDecisionKind::SkippedInvalidAfterReclassify,
                "approved discovery row is missing workspace path after reclassification"
                    .to_string(),
            ));
            continue;
        };
        if let Some(existing_agent) = state
            .agents
            .iter()
            .find(|agent| agent.thread_id.as_deref() == Some(thread_id.as_str()))
            .map(|agent| agent.name.clone())
        {
            decisions.push(promotion_decision(
                row_index,
                &row,
                generated_name,
                Some(existing_agent),
                PromotionDecisionKind::SkippedExistingThread,
                "thread/session id is already mapped to an existing agent".to_string(),
            ));
            continue;
        }
        let Some(name) = generated_name.clone() else {
            decisions.push(promotion_decision(
                row_index,
                &row,
                None,
                None,
                PromotionDecisionKind::SkippedInvalidAfterReclassify,
                "approved discovery row could not produce a canonical agent name".to_string(),
            ));
            continue;
        };
        if state.find_agent(&name).is_some() {
            decisions.push(promotion_decision(
                row_index,
                &row,
                Some(name.clone()),
                Some(name),
                PromotionDecisionKind::SkippedNameCollision,
                "generated agent name already exists".to_string(),
            ));
            continue;
        }

        let now = now_utc();
        state.agents.push(Agent {
            name: name.clone(),
            kind: AgentKind::Codex,
            thread_id: Some(thread_id),
            thread_source: ThreadSource::Codex,
            cwd: Some(cwd),
            route: Route::Local,
            status: AgentStatus::Unknown,
            chat_status: row.chat_status.clone(),
            chat_status_source: row.chat_status_source.clone(),
            active_turn_id: None,
            last_turn_id: None,
            model: None,
            model_provider: None,
            effort: None,
            service_tier: None,
            created_at: now.clone(),
            updated_at: now,
            last_error: None,
        });
        decisions.push(promotion_decision(
            row_index,
            &row,
            Some(name),
            None,
            PromotionDecisionKind::Promoted,
            "approved discovery row promoted to local agent".to_string(),
        ));
    }

    decisions
}

fn promotion_decision(
    row_index: usize,
    row: &DiscoveryRow,
    generated_name: Option<String>,
    existing_agent: Option<String>,
    decision: PromotionDecisionKind,
    reason: String,
) -> PromotionDecision {
    PromotionDecision {
        row_index,
        title: row.title.clone(),
        thread_id: row.thread_id.clone(),
        cwd_present: row.cwd.is_some(),
        source: format_discovery_source(&row.source),
        route: format_route(&row.route),
        thread_source: format_thread_source(&row.thread_source),
        classified_disposition: format_discovery_disposition(&row.disposition).to_string(),
        generated_name,
        existing_agent,
        decision,
        reason,
    }
}

fn discovery_agent_name(title: &str) -> String {
    let mut name = String::new();
    let mut previous_dash = false;
    for character in title.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            name.push(character);
            previous_dash = false;
        } else if !previous_dash && !name.is_empty() {
            name.push('-');
            previous_dash = true;
        }
    }
    while name.ends_with('-') {
        name.pop();
    }
    if name.is_empty() {
        "codex-session".to_string()
    } else {
        name
    }
}

fn validate_agent_shape_invariants(
    agent: &Agent,
    known_peer_names: &BTreeSet<String>,
) -> Result<(), CamError> {
    match agent.kind {
        AgentKind::VirtualInbox => {
            if !matches!(agent.route, Route::Local) {
                return Err(CamError::InvalidState(format!(
                    "virtual inbox agent `{}` must use local route",
                    agent.name
                )));
            }
            if !matches!(
                agent.thread_source,
                ThreadSource::Mailbox | ThreadSource::GuiOnly
            ) {
                return Err(CamError::InvalidState(format!(
                    "virtual inbox agent `{}` has invalid thread source",
                    agent.name
                )));
            }
        }
        AgentKind::Codex => {
            if !matches!(agent.route, Route::Local) {
                return Err(CamError::InvalidState(format!(
                    "local codex agent `{}` must use local route",
                    agent.name
                )));
            }
            if agent.thread_source != ThreadSource::Codex {
                return Err(CamError::InvalidState(format!(
                    "codex agent `{}` has invalid thread source",
                    agent.name
                )));
            }
            if agent.cwd.as_deref().unwrap_or_default().trim().is_empty() {
                return Err(CamError::InvalidState(format!(
                    "codex agent `{}` is missing local workspace path",
                    agent.name
                )));
            }
            if has_noncanonical_id(agent.thread_id.as_deref()) {
                return Err(CamError::InvalidState(format!(
                    "codex agent `{}` has non-canonical thread id",
                    agent.name
                )));
            }
        }
        AgentKind::AgySession => {
            if !matches!(agent.route, Route::Local) {
                return Err(CamError::InvalidState(format!(
                    "local agy agent `{}` must use local route",
                    agent.name
                )));
            }
            if agent.thread_source != ThreadSource::AgySession {
                return Err(CamError::InvalidState(format!(
                    "agy agent `{}` has invalid thread source",
                    agent.name
                )));
            }
            if !has_canonical_id(agent.thread_id.as_deref()) {
                return Err(CamError::InvalidState(format!(
                    "agy agent `{}` is missing session id",
                    agent.name
                )));
            }
        }
        AgentKind::RemoteMirror => {
            let Route::Peer { peer_name } = &agent.route else {
                return Err(CamError::InvalidState(format!(
                    "remote mirror agent `{}` must use peer route",
                    agent.name
                )));
            };
            let expected_prefix = format!("{peer_name}::");
            if !agent.name.starts_with(&expected_prefix) {
                return Err(CamError::InvalidState(format!(
                    "remote mirror agent `{}` must be namespaced as `{expected_prefix}...`",
                    agent.name
                )));
            }
            if peer_name.trim().is_empty() || !known_peer_names.contains(peer_name) {
                return Err(CamError::InvalidState(format!(
                    "remote mirror agent `{}` references unknown peer `{}`",
                    agent.name, peer_name
                )));
            }
            if agent.thread_source != ThreadSource::RemoteMirror {
                return Err(CamError::InvalidState(format!(
                    "remote mirror agent `{}` has invalid thread source",
                    agent.name
                )));
            }
            if let Some(thread_id) = agent.thread_id.as_deref()
                && !has_canonical_id(Some(thread_id))
            {
                return Err(CamError::InvalidState(format!(
                    "remote mirror agent `{}` has invalid thread id",
                    agent.name
                )));
            }
        }
    }
    Ok(())
}

fn validate_discovery_route_coherence(
    row: &DiscoveryRow,
    state: &AppState,
) -> Result<(), CamError> {
    match row.source {
        DiscoverySource::RemoteInventory => {
            let Route::Peer { peer_name } = &row.route else {
                return Err(CamError::InvalidState(format!(
                    "remote inventory discovery row `{}` must use peer route",
                    row.title
                )));
            };
            if row.peer_name.as_deref() != Some(peer_name.as_str()) {
                return Err(CamError::InvalidState(format!(
                    "remote inventory discovery row `{}` has mismatched peer metadata",
                    row.title
                )));
            }
            if state.find_peer(peer_name).is_none() {
                return Err(CamError::InvalidState(format!(
                    "remote inventory discovery row `{}` references unknown peer `{}`",
                    row.title, peer_name
                )));
            }
        }
        DiscoverySource::ThreadDatabase
        | DiscoverySource::CodexState
        | DiscoverySource::SessionIndex
        | DiscoverySource::Rollout => {
            if !matches!(row.route, Route::Local) {
                return Err(CamError::InvalidState(format!(
                    "local discovery row `{}` must use local route",
                    row.title
                )));
            }
            if row.peer_name.is_some() {
                return Err(CamError::InvalidState(format!(
                    "local discovery row `{}` must not carry peer metadata",
                    row.title
                )));
            }
        }
    }
    Ok(())
}

fn validate_discovery_classifier_determinism(row: &DiscoveryRow) -> Result<(), CamError> {
    let classified = classify_row(row.clone());
    if classified.disposition != row.disposition || classified.reason != row.reason {
        return Err(CamError::InvalidState(format!(
            "discovery row `{}` does not match trust classifier: expected {:?} because `{}`",
            row.title, classified.disposition, classified.reason
        )));
    }
    Ok(())
}

fn validate_discovery_promotion_boundary(
    row: &DiscoveryRow,
    state: &AppState,
) -> Result<(), CamError> {
    if row.disposition == DiscoveryDisposition::Approved || !matches!(row.route, Route::Local) {
        return Ok(());
    }
    let Some(thread_id) = row.thread_id.as_deref() else {
        return Ok(());
    };
    if thread_id.trim().is_empty() {
        return Ok(());
    }
    if state.agents.iter().any(|agent| {
        matches!(agent.kind, AgentKind::Codex | AgentKind::AgySession)
            && matches!(agent.route, Route::Local)
            && agent.thread_id.as_deref() == Some(thread_id)
    }) {
        return Err(CamError::InvalidState(format!(
            "non-approved discovery row `{}` is promoted as a local agent",
            row.title
        )));
    }
    Ok(())
}

pub fn validate_error_kind_token(error_kind: &str) -> Result<(), String> {
    if error_kind.trim().is_empty() || error_kind.trim() != error_kind {
        return Err("has non-canonical error_kind".to_string());
    }
    if !matches!(
        error_kind,
        "io" | "json"
            | "invalid_command"
            | "invalid_state"
            | "not_found"
            | "delivery_failed"
            | "provider_unavailable"
            | "peer_transport_failed"
            | "provider_contract_violation"
            | "peer_protocol_violation"
    ) {
        return Err(format!("has unknown error_kind `{error_kind}`"));
    }
    Ok(())
}

fn has_noncanonical_id(value: Option<&str>) -> bool {
    value.is_some_and(|value| !is_canonical_token(value))
}

fn has_canonical_id(value: Option<&str>) -> bool {
    value.is_some_and(is_canonical_token)
}

fn is_canonical_token(value: &str) -> bool {
    !value.is_empty() && value.trim() == value && !value.chars().any(char::is_whitespace)
}

fn thread_mapping_route_key(route: &Route) -> String {
    match route {
        Route::Local => "local".to_string(),
        Route::Peer { peer_name } => format!("peer:{peer_name}"),
    }
}

fn format_discovery_disposition(disposition: &DiscoveryDisposition) -> &'static str {
    match disposition {
        DiscoveryDisposition::Approved => "approved",
        DiscoveryDisposition::Candidate => "candidate",
        DiscoveryDisposition::Quarantined => "quarantined",
        DiscoveryDisposition::Rejected => "rejected",
    }
}

fn format_discovery_source(source: &DiscoverySource) -> String {
    match source {
        DiscoverySource::ThreadDatabase => "thread_database".to_string(),
        DiscoverySource::CodexState => "codex_state".to_string(),
        DiscoverySource::SessionIndex => "session_index".to_string(),
        DiscoverySource::Rollout => "rollout".to_string(),
        DiscoverySource::RemoteInventory => "remote_inventory".to_string(),
    }
}

fn format_chat_status(status: &ChatStatus) -> &'static str {
    match status {
        ChatStatus::Active => "active",
        ChatStatus::Archived => "archived",
        ChatStatus::Unknown => "unknown",
    }
}

fn format_chat_status_source(source: &ChatStatusSource) -> String {
    match source {
        ChatStatusSource::ThreadDatabase => "thread_database".to_string(),
        ChatStatusSource::DesktopThreadDatabase => "desktop_thread_database".to_string(),
        ChatStatusSource::RemoteInventory => "remote_inventory".to_string(),
        ChatStatusSource::Unknown => "unknown".to_string(),
    }
}

fn refresh_local_agent_chat_status_from_discovery(state: &mut AppState) {
    for agent in &mut state.agents {
        if !matches!(agent.route, Route::Local) {
            continue;
        }
        let Some(thread_id) = agent.thread_id.as_deref() else {
            continue;
        };
        let Some(row) = state
            .discovery_rows
            .iter()
            .find(|row| row.thread_id.as_deref() == Some(thread_id))
        else {
            continue;
        };
        if row.chat_status != ChatStatus::Unknown {
            agent.chat_status = row.chat_status.clone();
            agent.chat_status_source = row.chat_status_source.clone();
        }
        if agent.cwd.is_none() && row.cwd.is_some() {
            agent.cwd = row.cwd.clone();
        }
        agent.updated_at = now_utc();
    }
}

fn merge_desktop_archive_evidence_into_remote_mirrors(
    state: &mut AppState,
    codex_home: Option<&Path>,
) -> DesktopArchiveMergeSummary {
    let Some(codex_home) = codex_home else {
        return DesktopArchiveMergeSummary {
            warning: Some("no Codex home available; local Desktop remote-chat archive merge was not attempted".to_string()),
            ..DesktopArchiveMergeSummary::default()
        };
    };
    let candidate_paths = desktop_archive_evidence_paths(codex_home);
    let mut summary = DesktopArchiveMergeSummary {
        attempted: true,
        source_path: candidate_paths
            .first()
            .map(|path| path.display().to_string()),
        source_paths: candidate_paths
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
        ..DesktopArchiveMergeSummary::default()
    };
    let mut evidence = BTreeMap::<String, DesktopThreadArchiveEvidence>::new();
    let mut source_warnings = Vec::new();
    let mut existing_source_count = 0usize;
    for db_path in candidate_paths {
        if !db_path.is_file() {
            source_warnings.push(format!(
                "local Desktop thread database {} does not exist",
                db_path.display()
            ));
            continue;
        }
        existing_source_count += 1;
        match scan_desktop_thread_archive_evidence(&db_path) {
            Ok(rows) => {
                summary.evidence_rows += rows.len();
                for (thread_id, row) in rows {
                    evidence.insert(thread_id, row);
                }
            }
            Err(error) => {
                source_warnings.push(format!(
                    "local Desktop archive evidence source {} failed loudly: {error}",
                    db_path.display()
                ));
            }
        }
    }
    if existing_source_count == 0 {
        summary.warning = Some(format!(
            "no local Desktop thread databases exist at [{}]; remote mirror Desktop archive merge could not run",
            summary.source_paths.join(", ")
        ));
        return summary;
    }
    if !source_warnings.is_empty() {
        summary.warning = Some(source_warnings.join("; "));
    }

    for agent in &mut state.agents {
        if !matches!(agent.route, Route::Peer { .. }) {
            continue;
        }
        summary.remote_mirrors_seen += 1;
        let Some(thread_id) = agent.thread_id.as_deref() else {
            summary.unmatched += 1;
            continue;
        };
        let Some(row) = evidence.get(thread_id) else {
            summary.unmatched += 1;
            continue;
        };
        summary.matched += 1;
        if agent.chat_status != ChatStatus::Unknown && agent.chat_status != row.chat_status {
            summary.conflicts += 1;
            agent.last_error = Some(format!(
                "chat status conflict: remote inventory says {:?}, local Desktop thread database says {:?} for thread `{}`",
                agent.chat_status, row.chat_status, thread_id
            ));
        }
        agent.chat_status = row.chat_status.clone();
        agent.chat_status_source = row.chat_status_source.clone();
        agent.updated_at = now_utc();
        summary.applied += 1;
    }

    summary
}

fn desktop_archive_evidence_paths(codex_home: &Path) -> Vec<PathBuf> {
    vec![
        codex_home.join("state_5.sqlite"),
        codex_home.join("sqlite").join("state_5.sqlite"),
    ]
}

fn format_thread_source(source: &ThreadSource) -> String {
    match source {
        ThreadSource::Codex => "codex".to_string(),
        ThreadSource::AgySession => "agy_session".to_string(),
        ThreadSource::Mailbox => "mailbox".to_string(),
        ThreadSource::GuiOnly => "gui_only".to_string(),
        ThreadSource::RemoteMirror => "remote_mirror".to_string(),
    }
}

fn format_route(route: &Route) -> String {
    match route {
        Route::Local => "local".to_string(),
        Route::Peer { peer_name } => format!("peer:{peer_name}"),
    }
}

pub fn delivery_state_label(delivery: &DeliveryState) -> &'static str {
    match delivery {
        DeliveryState::Started => "started",
        DeliveryState::Steered => "steered",
        DeliveryState::Delivered => "delivered",
        DeliveryState::Received => "received",
        DeliveryState::Queued => "queued",
        DeliveryState::Failed => "failed",
    }
}

pub fn delivery_action_label(action: &DeliveryAction) -> &'static str {
    match action {
        DeliveryAction::SteerActiveTurn => "steer_active_turn",
        DeliveryAction::WakeKnownSession => "wake_known_session",
        DeliveryAction::StoreInVirtualInbox => "store_in_virtual_inbox",
        DeliveryAction::QueueFallback => "queue_fallback",
        DeliveryAction::FailLoudly => "fail_loudly",
    }
}

pub fn parse_delivery_state_label(value: &str) -> Result<DeliveryState, CamError> {
    match value {
        "started" => Ok(DeliveryState::Started),
        "steered" => Ok(DeliveryState::Steered),
        "delivered" => Ok(DeliveryState::Delivered),
        "received" => Ok(DeliveryState::Received),
        "queued" => Ok(DeliveryState::Queued),
        "failed" => Ok(DeliveryState::Failed),
        _ => Err(CamError::InvalidCommand(format!(
            "unknown delivery state `{value}`"
        ))),
    }
}

pub fn parse_delivery_action_label(value: &str) -> Result<DeliveryAction, CamError> {
    match value {
        "steer_active_turn" => Ok(DeliveryAction::SteerActiveTurn),
        "wake_known_session" => Ok(DeliveryAction::WakeKnownSession),
        "store_in_virtual_inbox" => Ok(DeliveryAction::StoreInVirtualInbox),
        "queue_fallback" => Ok(DeliveryAction::QueueFallback),
        "fail_loudly" => Ok(DeliveryAction::FailLoudly),
        _ => Err(CamError::InvalidCommand(format!(
            "unknown delivery action `{value}`"
        ))),
    }
}

fn validate_optional_mailbox_trimmed(
    value: Option<&str>,
    field: &str,
    message_id: &str,
) -> Result<(), CamError> {
    if value.is_some_and(|value| value.is_empty() || value.trim() != value) {
        return Err(CamError::InvalidState(format!(
            "mailbox message `{message_id}` has non-canonical {field}"
        )));
    }
    Ok(())
}

fn parse_mailbox_timestamp(
    value: &str,
    field: &str,
    message_id: &str,
) -> Result<OffsetDateTime, CamError> {
    OffsetDateTime::parse(value, &Rfc3339).map_err(|error| {
        CamError::InvalidState(format!(
            "mailbox message `{message_id}` has invalid {field}: {error}"
        ))
    })
}

fn validate_name_hygiene(state: &AppState) -> Result<(), CamError> {
    for agent in &state.agents {
        if !is_canonical_state_name(&agent.name) {
            return Err(CamError::InvalidState(format!(
                "agent name `{}` must be nonempty and contain no whitespace",
                agent.name
            )));
        }
    }
    for peer in &state.peers {
        if !is_canonical_state_name(&peer.name) {
            return Err(CamError::InvalidState(format!(
                "peer name `{}` must be nonempty and contain no whitespace",
                peer.name
            )));
        }
    }
    Ok(())
}

fn is_canonical_state_name(name: &str) -> bool {
    !name.is_empty() && name.trim() == name && !name.contains(char::is_whitespace)
}

fn ensure_unique_names<'a>(
    names: impl IntoIterator<Item = &'a str>,
    label: &str,
) -> Result<(), CamError> {
    let mut seen = BTreeSet::new();
    for name in names {
        if !seen.insert(name) {
            return Err(CamError::InvalidState(format!(
                "duplicate {label}: `{name}`"
            )));
        }
    }
    Ok(())
}

pub fn list_peers(state: &AppState) -> Vec<Peer> {
    state.peers.clone()
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PeerListRow {
    pub name: String,
    pub transport: &'static str,
    pub state: &'static str,
    pub ssh_target: Option<String>,
    pub remote_root: Option<String>,
}

pub fn list_peer_rows(state: &AppState) -> Vec<PeerListRow> {
    state
        .peers
        .iter()
        .map(|peer| PeerListRow {
            name: peer.name.clone(),
            transport: format_peer_transport(&peer.transport),
            state: format_peer_state(&peer.state),
            ssh_target: peer.ssh_target.clone(),
            remote_root: peer.remote_root.clone(),
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerSyncResult {
    pub ok: bool,
    pub implemented: bool,
    pub peer: String,
    pub transport: String,
    pub ssh_target: Option<String>,
    pub state: String,
    pub remote_command_attempted: bool,
    pub inventory_source: Option<String>,
    pub agents_imported: usize,
    pub remote_agents_seen: usize,
    pub mirrored_agents_added: usize,
    pub mirrored_agents_updated: usize,
    pub mirrored_agents_skipped: usize,
    pub mirrored_agents_stale: usize,
    pub collision_count: usize,
    pub degraded: bool,
    pub remote_node_name: Option<String>,
    pub last_sync_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desktop_archive_merge: Option<DesktopArchiveMergeSummary>,
    pub error_kind: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerSyncAllResult {
    pub ok: bool,
    pub implemented: bool,
    pub mode: String,
    pub peers_requested: usize,
    pub peers_synced: usize,
    pub peers_failed: usize,
    pub peers_degraded: usize,
    pub remote_command_attempted: bool,
    pub agents_imported: usize,
    pub remote_agents_seen: usize,
    pub mirrored_agents_added: usize,
    pub mirrored_agents_updated: usize,
    pub mirrored_agents_skipped: usize,
    pub mirrored_agents_stale: usize,
    pub collision_count: usize,
    pub failed_error_kinds: Vec<String>,
    pub warning: Option<String>,
    pub results: Vec<PeerSyncResult>,
}

impl PeerSyncAllResult {
    pub fn from_results(peers: Vec<PeerSyncResult>) -> Self {
        let failed = peers.iter().filter(|peer| !peer.ok).count();
        let degraded = peers.iter().filter(|peer| peer.ok && peer.degraded).count();
        let warning = if peers.is_empty() {
            Some("no peers are enrolled; nothing was synced".to_string())
        } else if failed > 0 {
            Some("one or more peer inventory sync attempts failed loudly".to_string())
        } else if degraded > 0 {
            Some(
                "one or more peer inventory sync attempts completed with degraded mirror metadata"
                    .to_string(),
            )
        } else {
            None
        };
        let remote_command_attempted = peers.iter().any(|peer| peer.remote_command_attempted);
        let failed_error_kinds = collect_failed_peer_error_kinds(&peers);
        Self {
            ok: failed == 0,
            implemented: !peers.is_empty(),
            mode: "all".to_string(),
            peers_requested: peers.len(),
            peers_synced: peers.iter().filter(|peer| peer.ok).count(),
            peers_failed: failed,
            peers_degraded: degraded,
            remote_command_attempted,
            agents_imported: peers.iter().map(|peer| peer.agents_imported).sum(),
            remote_agents_seen: peers.iter().map(|peer| peer.remote_agents_seen).sum(),
            mirrored_agents_added: peers.iter().map(|peer| peer.mirrored_agents_added).sum(),
            mirrored_agents_updated: peers.iter().map(|peer| peer.mirrored_agents_updated).sum(),
            mirrored_agents_skipped: peers.iter().map(|peer| peer.mirrored_agents_skipped).sum(),
            mirrored_agents_stale: peers.iter().map(|peer| peer.mirrored_agents_stale).sum(),
            collision_count: peers.iter().map(|peer| peer.collision_count).sum(),
            failed_error_kinds,
            warning,
            results: peers,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PeerSyncEventFields {
    pub peer: String,
    pub transport: String,
    pub ssh_target: Option<String>,
    pub ok: bool,
    pub implemented: bool,
    pub remote_command_attempted: bool,
    pub agents_imported: usize,
    pub remote_agents_seen: usize,
    pub mirrored_agents_added: usize,
    pub mirrored_agents_updated: usize,
    pub mirrored_agents_skipped: usize,
    pub mirrored_agents_stale: usize,
    pub collision_count: usize,
    pub degraded: bool,
    pub error_kind: Option<String>,
    pub error: Option<String>,
}

pub fn peer_sync_event_fields(result: &PeerSyncResult) -> PeerSyncEventFields {
    PeerSyncEventFields {
        peer: result.peer.clone(),
        transport: result.transport.clone(),
        ssh_target: result.ssh_target.clone(),
        ok: result.ok,
        implemented: result.implemented,
        remote_command_attempted: result.remote_command_attempted,
        agents_imported: result.agents_imported,
        remote_agents_seen: result.remote_agents_seen,
        mirrored_agents_added: result.mirrored_agents_added,
        mirrored_agents_updated: result.mirrored_agents_updated,
        mirrored_agents_skipped: result.mirrored_agents_skipped,
        mirrored_agents_stale: result.mirrored_agents_stale,
        collision_count: result.collision_count,
        degraded: result.degraded,
        error_kind: result.error_kind.clone(),
        error: result.error.clone(),
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PeerSyncAllEventFields {
    pub mode: String,
    pub ok: bool,
    pub implemented: bool,
    pub peers_requested: usize,
    pub peers_synced: usize,
    pub peers_failed: usize,
    pub peers_degraded: usize,
    pub agents_imported: usize,
    pub remote_agents_seen: usize,
    pub mirrored_agents_added: usize,
    pub mirrored_agents_updated: usize,
    pub mirrored_agents_skipped: usize,
    pub mirrored_agents_stale: usize,
    pub collision_count: usize,
    pub remote_command_attempted: bool,
    pub failed_error_kinds: Vec<String>,
    pub warning: Option<String>,
}

pub fn peer_sync_all_event_fields(result: &PeerSyncAllResult) -> PeerSyncAllEventFields {
    PeerSyncAllEventFields {
        mode: result.mode.clone(),
        ok: result.ok,
        implemented: result.implemented,
        peers_requested: result.peers_requested,
        peers_synced: result.peers_synced,
        peers_failed: result.peers_failed,
        peers_degraded: result.peers_degraded,
        agents_imported: result.agents_imported,
        remote_agents_seen: result.remote_agents_seen,
        mirrored_agents_added: result.mirrored_agents_added,
        mirrored_agents_updated: result.mirrored_agents_updated,
        mirrored_agents_skipped: result.mirrored_agents_skipped,
        mirrored_agents_stale: result.mirrored_agents_stale,
        collision_count: result.collision_count,
        remote_command_attempted: result.remote_command_attempted,
        failed_error_kinds: result.failed_error_kinds.clone(),
        warning: result.warning.clone(),
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct InventoryExportedEventFields {
    pub inventory_version: u32,
    pub node_name: String,
    pub agents: usize,
    pub peers: usize,
    pub discovery_rows: usize,
    pub mailbox_exported: bool,
}

pub fn inventory_exported_event_fields(
    inventory: &InventoryExport,
) -> InventoryExportedEventFields {
    InventoryExportedEventFields {
        inventory_version: inventory.inventory_version,
        node_name: inventory.node_name.clone(),
        agents: inventory.agents.len(),
        peers: inventory.peers.len(),
        discovery_rows: inventory.discovery.rows,
        mailbox_exported: false,
    }
}

impl PeerSyncResult {
    pub fn synced(
        peer: &Peer,
        remote_command_attempted: bool,
        apply: &RemoteMirrorApplyResult,
    ) -> Self {
        Self {
            ok: true,
            implemented: true,
            peer: peer.name.clone(),
            transport: format_peer_transport(&peer.transport).to_string(),
            ssh_target: peer.ssh_target.clone(),
            state: format_peer_state(&peer.state).to_string(),
            remote_command_attempted,
            inventory_source: peer.inventory_source.clone(),
            agents_imported: apply.mirrored_agents_added,
            remote_agents_seen: apply.remote_agents_seen,
            mirrored_agents_added: apply.mirrored_agents_added,
            mirrored_agents_updated: apply.mirrored_agents_updated,
            mirrored_agents_skipped: apply.mirrored_agents_skipped,
            mirrored_agents_stale: apply.mirrored_agents_stale,
            collision_count: apply.collision_count,
            degraded: peer.state == PeerState::MirroredDegraded,
            remote_node_name: peer.remote_node_name.clone(),
            last_sync_at: peer.last_sync_at.clone(),
            desktop_archive_merge: None,
            error_kind: None,
            error: None,
        }
    }

    pub fn failed(peer: &Peer, remote_command_attempted: bool, agents_imported: usize) -> Self {
        Self {
            ok: false,
            implemented: true,
            peer: peer.name.clone(),
            transport: format_peer_transport(&peer.transport).to_string(),
            ssh_target: peer.ssh_target.clone(),
            state: format_peer_state(&peer.state).to_string(),
            remote_command_attempted,
            inventory_source: peer.inventory_source.clone(),
            agents_imported,
            remote_agents_seen: 0,
            mirrored_agents_added: 0,
            mirrored_agents_updated: 0,
            mirrored_agents_skipped: 0,
            mirrored_agents_stale: 0,
            collision_count: 0,
            degraded: false,
            remote_node_name: peer.remote_node_name.clone(),
            last_sync_at: peer.last_sync_at.clone(),
            desktop_archive_merge: None,
            error_kind: peer.last_sync_error_kind.clone(),
            error: peer.last_sync_error.clone(),
        }
    }
}

pub fn mark_peer_sync_failed(peer: &mut Peer, error: &CamError) {
    let now = now_utc();
    peer.state = PeerState::SyncFailed;
    peer.last_sync_at = Some(now.clone());
    peer.last_sync_error = Some(error.to_string());
    peer.last_sync_error_kind = Some(error.kind().to_string());
    peer.inventory_source = Some("inventory_export".to_string());
    peer.updated_at = now;
}

pub fn apply_peer_sync_failure(
    peer: &mut Peer,
    error: &CamError,
    remote_command_attempted: bool,
    agents_imported: usize,
) -> PeerSyncResult {
    mark_peer_sync_failed(peer, error);
    PeerSyncResult::failed(peer, remote_command_attempted, agents_imported)
}

pub fn sync_peer_inventory_from_fetcher(
    state: &mut AppState,
    peer_name: &str,
    peer_inventory_fetcher: &dyn PeerInventoryFetcher,
) -> Result<PeerSyncResult, CamError> {
    let peer_snapshot = state
        .find_peer(peer_name)
        .cloned()
        .ok_or_else(|| CamError::NotFound(format!("peer `{peer_name}` does not exist")))?;

    let fetch = match peer_inventory_fetcher.fetch_inventory(&peer_snapshot) {
        Ok(fetch) => fetch,
        Err(fetch_error) => {
            let peer = state
                .peers
                .iter_mut()
                .find(|peer| peer.name == peer_name)
                .ok_or_else(|| CamError::NotFound(format!("peer `{peer_name}` does not exist")))?;
            return Ok(apply_peer_sync_failure(
                peer,
                &fetch_error.error,
                fetch_error.remote_command_attempted,
                0,
            ));
        }
    };

    let inventory = match parse_inventory_export(&fetch.stdout) {
        Ok(inventory) => inventory,
        Err(error) => {
            let peer = state
                .peers
                .iter_mut()
                .find(|peer| peer.name == peer_name)
                .ok_or_else(|| CamError::NotFound(format!("peer `{peer_name}` does not exist")))?;
            return Ok(apply_peer_sync_failure(peer, &error, true, 0));
        }
    };

    match apply_peer_inventory_sync(state, peer_name, &inventory, true) {
        Ok(result) => Ok(result),
        Err(error) => {
            let mut peer = peer_snapshot;
            Ok(apply_peer_sync_failure(&mut peer, &error, true, 0))
        }
    }
}

pub fn sync_all_peer_inventories_from_fetcher(
    state: &mut AppState,
    peer_inventory_fetcher: &dyn PeerInventoryFetcher,
) -> PeerSyncAllResult {
    let peer_names = state
        .peers
        .iter()
        .map(|peer| peer.name.clone())
        .collect::<Vec<_>>();
    let results = peer_names
        .into_iter()
        .map(|peer_name| {
            sync_peer_inventory_from_fetcher(state, &peer_name, peer_inventory_fetcher)
                .unwrap_or_else(|error| {
                    let peer = Peer {
                        name: peer_name,
                        transport: PeerTransport::Ssh,
                        ssh_target: None,
                        key_path: None,
                        remote_root: None,
                        state: PeerState::SyncFailed,
                        remote_node_name: None,
                        last_sync_at: None,
                        last_sync_error: Some(error.to_string()),
                        last_sync_error_kind: Some(error.kind().to_string()),
                        inventory_source: Some("inventory_export".to_string()),
                        created_at: now_utc(),
                        updated_at: now_utc(),
                    };
                    PeerSyncResult::failed(&peer, false, 0)
                })
        })
        .collect::<Vec<_>>();
    PeerSyncAllResult::from_results(results)
}

pub fn apply_peer_inventory_sync(
    state: &mut AppState,
    peer_name: &str,
    inventory: &InventoryExport,
    remote_command_attempted: bool,
) -> Result<PeerSyncResult, CamError> {
    let peer_index = state
        .peers
        .iter()
        .position(|peer| peer.name == peer_name)
        .ok_or_else(|| CamError::NotFound(format!("peer `{peer_name}` is not enrolled")))?;

    let mut peer = state.peers.remove(peer_index);
    let result = match apply_trusted_peer_inventory(&mut peer, inventory, &mut state.agents) {
        Ok(apply) => {
            let codex_home = default_codex_home();
            let desktop_archive_merge =
                merge_desktop_archive_evidence_into_remote_mirrors(state, codex_home.as_deref());
            let mut result = PeerSyncResult::synced(&peer, remote_command_attempted, &apply);
            result.desktop_archive_merge = Some(desktop_archive_merge);
            result
        }
        Err(_) => PeerSyncResult::failed(&peer, remote_command_attempted, 0),
    };
    state.peers.insert(peer_index, peer);
    Ok(result)
}

fn collect_failed_peer_error_kinds(peers: &[PeerSyncResult]) -> Vec<String> {
    let mut kinds = BTreeSet::new();
    for peer in peers {
        if !peer.ok {
            if let Some(error_kind) = peer.error_kind.as_deref() {
                kinds.insert(error_kind.to_string());
            }
        }
    }
    kinds.into_iter().collect()
}

fn format_peer_transport(transport: &PeerTransport) -> &'static str {
    match transport {
        PeerTransport::Ssh => "ssh",
        PeerTransport::CodexManaged => "codex_managed",
    }
}

fn format_peer_state(state: &PeerState) -> &'static str {
    match state {
        PeerState::Unknown => "unknown",
        PeerState::Verified => "verified",
        PeerState::Mirrored => "mirrored",
        PeerState::MirroredDegraded => "mirrored_degraded",
        PeerState::SyncFailed => "sync_failed",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrollPeerRequest {
    pub name: String,
    pub ssh_target: String,
    pub key_path: Option<String>,
    pub remote_root: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildEnrollPeerRequest {
    pub peer_names: Vec<String>,
    pub ssh_target: Option<String>,
    pub key_path: Option<String>,
    pub remote_root: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrollPeerOutcome {
    pub peer: Peer,
    pub updated_existing: bool,
    pub network_probe_attempted: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PeerEnrollmentEventFields {
    pub peer: String,
    pub transport: &'static str,
    pub ssh_target: Option<String>,
    pub key_path_present: bool,
    pub remote_root: Option<String>,
    pub network_probe_attempted: bool,
}

pub fn peer_enrollment_event_fields(outcome: &EnrollPeerOutcome) -> PeerEnrollmentEventFields {
    PeerEnrollmentEventFields {
        peer: outcome.peer.name.clone(),
        transport: format_peer_transport(&outcome.peer.transport),
        ssh_target: outcome.peer.ssh_target.clone(),
        key_path_present: outcome.peer.key_path.is_some(),
        remote_root: outcome.peer.remote_root.clone(),
        network_probe_attempted: outcome.network_probe_attempted,
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PeerEnrollmentSummary {
    pub name: String,
    pub transport: &'static str,
    pub ssh_target: Option<String>,
    pub state: &'static str,
    pub network_probe_attempted: bool,
}

pub fn peer_enrollment_summary(outcome: &EnrollPeerOutcome) -> PeerEnrollmentSummary {
    PeerEnrollmentSummary {
        name: outcome.peer.name.clone(),
        transport: format_peer_transport(&outcome.peer.transport),
        ssh_target: outcome.peer.ssh_target.clone(),
        state: format_peer_state(&outcome.peer.state),
        network_probe_attempted: outcome.network_probe_attempted,
    }
}

pub fn build_enroll_peer_request(
    request: BuildEnrollPeerRequest,
) -> Result<EnrollPeerRequest, CamError> {
    if request.peer_names.len() != 1 {
        return Err(CamError::InvalidCommand(
            "peer add requires exactly one peer name".to_string(),
        ));
    }
    let name = request.peer_names[0].clone();
    validate_canonical_name("peer name", &name)?;
    let ssh_target = request.ssh_target.ok_or_else(|| {
        CamError::InvalidCommand("peer add requires --ssh <user@host>".to_string())
    })?;
    validate_ssh_target(&ssh_target)?;
    validate_optional_nonempty_trimmed("--key", request.key_path.as_deref())?;
    validate_optional_nonempty_trimmed("--remote-root", request.remote_root.as_deref())?;

    Ok(EnrollPeerRequest {
        name,
        ssh_target,
        key_path: request.key_path,
        remote_root: request.remote_root,
    })
}

pub fn enroll_peer(
    state: &mut AppState,
    request: EnrollPeerRequest,
) -> Result<EnrollPeerOutcome, CamError> {
    validate_canonical_name("peer name", &request.name)?;
    validate_ssh_target(&request.ssh_target)?;
    validate_optional_nonempty_trimmed("key_path", request.key_path.as_deref())?;
    validate_optional_nonempty_trimmed("remote_root", request.remote_root.as_deref())?;

    let now = now_utc();
    let peer = Peer {
        name: request.name,
        transport: PeerTransport::Ssh,
        ssh_target: Some(request.ssh_target),
        key_path: request.key_path,
        remote_root: request.remote_root,
        state: PeerState::Unknown,
        remote_node_name: None,
        last_sync_at: None,
        last_sync_error: None,
        last_sync_error_kind: None,
        inventory_source: None,
        created_at: now.clone(),
        updated_at: now,
    };
    let updated_existing = state.upsert_peer(peer.clone());
    let peer = state.find_peer(&peer.name).cloned().unwrap_or(peer);

    Ok(EnrollPeerOutcome {
        peer,
        updated_existing,
        network_probe_attempted: false,
    })
}

pub fn export_inventory(state: &AppState) -> InventoryExport {
    build_inventory(
        &state.config,
        &state.agents,
        &state.peers,
        &state.discovery_rows,
    )
}

fn validate_canonical_name(label: &str, value: &str) -> Result<(), CamError> {
    if value.trim().is_empty() {
        return Err(CamError::InvalidCommand(format!("{label} cannot be empty")));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(CamError::InvalidCommand(format!(
            "{label} `{value}` cannot contain whitespace"
        )));
    }
    Ok(())
}

pub fn validate_command_agent_name(value: &str) -> Result<(), CamError> {
    validate_canonical_name("agent name", value)
}

pub fn validate_command_peer_name(value: &str) -> Result<(), CamError> {
    validate_canonical_name("peer name", value)
}

pub fn validate_command_optional_nonempty_trimmed(
    label: &str,
    value: Option<&str>,
) -> Result<(), CamError> {
    validate_optional_nonempty_trimmed(&format!("--{label}"), value)
}

fn validate_ssh_target(target: &str) -> Result<(), CamError> {
    if target.trim().is_empty() || target.contains(char::is_whitespace) || !target.contains('@') {
        return Err(CamError::InvalidCommand(format!(
            "ssh target `{target}` must look like user@host"
        )));
    }
    Ok(())
}

fn validate_optional_agent_name(label: &str, value: Option<&str>) -> Result<(), CamError> {
    if let Some(value) = value {
        validate_canonical_name("agent name", value).map_err(|error| {
            CamError::InvalidCommand(format!("{label} must be a canonical agent name: {error}"))
        })?;
    }
    Ok(())
}

fn validate_nonempty_trimmed(label: &str, value: &str) -> Result<(), CamError> {
    if value.trim().is_empty() {
        return Err(CamError::InvalidCommand(format!("{label} cannot be empty")));
    }
    if value.trim() != value {
        return Err(CamError::InvalidCommand(format!(
            "{label} must not have leading or trailing whitespace"
        )));
    }
    Ok(())
}

fn validate_optional_nonempty_trimmed(label: &str, value: Option<&str>) -> Result<(), CamError> {
    if let Some(value) = value {
        validate_nonempty_trimmed(label, value)?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepareSendRequest {
    pub target_agent: String,
    pub body: String,
    pub source_agent: Option<String>,
    pub source_node: Option<String>,
    pub correlation_id: Option<String>,
    pub message_type: Option<String>,
    pub strict: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildSendRequest {
    pub positionals: Vec<String>,
    pub source_agent: Option<String>,
    pub source_node: Option<String>,
    pub correlation_id: Option<String>,
    pub message_type: Option<String>,
    pub receipt_nonce: Option<String>,
    pub expected_delivery: Option<String>,
    pub delivery_action: Option<String>,
    pub strict: bool,
    pub use_codex_stdio: bool,
    pub codex_program: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltSendRequest {
    pub prepare: PrepareSendRequest,
    pub receipt_nonce: Option<String>,
    pub expected_delivery: Option<DeliveryState>,
    pub delivery_action: Option<DeliveryAction>,
    pub use_codex_stdio: bool,
    pub codex_program: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedSend {
    pub target: Option<Agent>,
    pub decision: DeliveryDecision,
    pub message: Message,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSendApplication {
    pub target: Option<Agent>,
    pub decision: DeliveryDecision,
    pub message: Message,
    pub event_type: &'static str,
    pub ok: bool,
    pub result_error: Option<String>,
    pub persist_state: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectSendApplication {
    pub target: Agent,
    pub decision: DeliveryDecision,
    pub message: Message,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendApplication {
    Local(LocalSendApplication),
    Direct(DirectSendApplication),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectDeliveryFailureKind {
    Unqueueable,
    Recoverable,
}

pub struct CodexOwnerDeliveryRuntime {
    registry: CodexOwnerRegistry,
}

impl CodexOwnerDeliveryRuntime {
    pub fn new() -> Self {
        Self {
            registry: CodexOwnerRegistry::new(),
        }
    }

    pub fn owner_count(&self) -> usize {
        self.registry.len()
    }

    pub fn has_owner_for_thread(&self, thread_id: &str) -> bool {
        self.registry.has_owner_for_thread(thread_id)
    }

    pub fn start_threadless_and_wake_with_program_and_model(
        &mut self,
        state: &mut AppState,
        target: Agent,
        decision: DeliveryDecision,
        message: Message,
        program: Option<String>,
        model: Option<String>,
    ) -> Result<DirectSendApplication, CamError> {
        if decision.action != DeliveryAction::WakeKnownSession {
            return Err(CamError::InvalidState(format!(
                "Codex owner runtime threadless wake requires wake_known_session decision, got `{}`",
                delivery_action_label(&decision.action)
            )));
        }
        if decision.state != DeliveryState::Delivered {
            return Err(CamError::InvalidState(format!(
                "Codex owner runtime threadless wake requires delivered state, got `{}`",
                delivery_state_label(&decision.state)
            )));
        }
        if target.kind != AgentKind::Codex || target.route != Route::Local {
            return Err(CamError::InvalidState(format!(
                "Codex owner runtime threadless wake requires a local Codex agent, got kind `{}` route `{}`",
                format_agent_kind(&target.kind),
                format_route(&target.route)
            )));
        }
        if target.thread_id.is_some() {
            return Err(CamError::InvalidState(
                "Codex owner runtime threadless wake requires an agent without a thread id"
                    .to_string(),
            ));
        }
        let outcome = self
            .registry
            .start_thread_then_wake_with_program_and_model(&target, &message, program, model)?;
        apply_codex_threadless_send_success(state, target, decision, message, outcome.send)
    }

    pub fn steer_active_turn(
        &mut self,
        state: &mut AppState,
        target: Agent,
        decision: DeliveryDecision,
        message: Message,
    ) -> Result<DirectSendApplication, CamError> {
        if decision.action != DeliveryAction::SteerActiveTurn {
            return Err(CamError::InvalidState(format!(
                "Codex owner runtime steer requires steer_active_turn decision, got `{}`",
                delivery_action_label(&decision.action)
            )));
        }
        if decision.state != DeliveryState::Steered {
            return Err(CamError::InvalidState(format!(
                "Codex owner runtime steer requires steered state, got `{}`",
                delivery_state_label(&decision.state)
            )));
        }
        let request = ProviderSendRequest::from_delivery_decision(
            &target,
            &message,
            decision.action.clone(),
            decision.state.clone(),
        );
        let outcome = self.registry.steer_active_turn(request)?;
        apply_codex_owner_steer_success(state, target, decision, message, outcome)
    }

    pub fn wake_known_session_with_program_and_model(
        &mut self,
        state: &mut AppState,
        target: Agent,
        decision: DeliveryDecision,
        message: Message,
        program: Option<String>,
        model: Option<String>,
    ) -> Result<DirectSendApplication, CamError> {
        if decision.action != DeliveryAction::WakeKnownSession {
            return Err(CamError::InvalidState(format!(
                "Codex owner runtime wake requires wake_known_session decision, got `{}`",
                delivery_action_label(&decision.action)
            )));
        }
        if decision.state != DeliveryState::Delivered {
            return Err(CamError::InvalidState(format!(
                "Codex owner runtime wake requires delivered state, got `{}`",
                delivery_state_label(&decision.state)
            )));
        }
        if target.kind != AgentKind::Codex || target.route != Route::Local {
            return Err(CamError::InvalidState(format!(
                "Codex owner runtime wake requires a local Codex agent, got kind `{}` route `{}`",
                format_agent_kind(&target.kind),
                format_route(&target.route)
            )));
        }
        if target.thread_id.is_none() {
            return Err(CamError::InvalidState(
                "Codex owner runtime wake requires an agent with a thread id".to_string(),
            ));
        }
        let mut wake_target = target.clone();
        wake_target.active_turn_id = None;
        let request = ProviderSendRequest::from_delivery_decision(
            &wake_target,
            &message,
            decision.action.clone(),
            decision.state.clone(),
        );
        let outcome = self
            .registry
            .wake_known_session_with_program_and_model(request, program, model)?;
        apply_codex_owner_wake_success(state, target, decision, message, outcome)
    }

    pub fn resume_existing_thread_with_program_and_model(
        &mut self,
        state: &mut AppState,
        agent: Agent,
        program: Option<String>,
        model: Option<String>,
    ) -> Result<AgentResumeApplication, CamError> {
        if agent.kind != AgentKind::Codex || agent.route != Route::Local {
            return Err(CamError::InvalidState(format!(
                "Codex owner runtime resume attach requires a local Codex agent, got kind `{}` route `{}`",
                format_agent_kind(&agent.kind),
                format_route(&agent.route)
            )));
        }
        if agent.thread_id.is_none() {
            return Ok(AgentResumeApplication {
                result: AgentResumeResult::for_agent(&agent),
                persist_state: false,
            });
        }
        let outcome = self
            .registry
            .resume_existing_thread_with_program_and_model(&agent, program, model)?;
        let mut result = apply_provider_resume_outcome(&agent, outcome.resume)?;
        if result.ready && !outcome.owner_retained {
            return Err(CamError::ProviderContractViolation(
                "Codex owner runtime resume attach returned ready without retained owner proof"
                    .to_string(),
            ));
        }
        if result.ready && result.active_turn_id.is_none() && result.last_turn_id.is_none() {
            return Err(CamError::ProviderContractViolation(
                "Codex owner runtime resume attach returned ready without active_turn_id or last_turn_id proof"
                    .to_string(),
            ));
        }
        if result.ready {
            result.message =
                "daemon-owned Codex app-server resumed and attached live owner".to_string();
            apply_agent_resume_success(state, &agent.name, &result)?;
            return Ok(AgentResumeApplication {
                result,
                persist_state: true,
            });
        }
        result.message =
            "daemon-owned Codex app-server resume attempted but provider did not prove readiness"
                .to_string();
        Ok(AgentResumeApplication {
            result,
            persist_state: false,
        })
    }
}

impl Default for CodexOwnerDeliveryRuntime {
    fn default() -> Self {
        Self::new()
    }
}

pub struct DaemonRuntime {
    codex_owner_delivery: CodexOwnerDeliveryRuntime,
}

impl DaemonRuntime {
    pub fn new() -> Self {
        Self {
            codex_owner_delivery: CodexOwnerDeliveryRuntime::new(),
        }
    }

    pub fn codex_owner_count(&self) -> usize {
        self.codex_owner_delivery.owner_count()
    }

    pub fn resume_local_codex_with_owner_runtime(
        &mut self,
        state: &mut AppState,
        name: &str,
    ) -> Result<AgentResumeApplication, CamError> {
        validate_canonical_name("agent name", name)?;
        let agent = state
            .find_agent(name)
            .ok_or_else(|| CamError::NotFound(format!("agent `{name}` does not exist")))?
            .clone();
        if agent.kind != AgentKind::Codex || agent.route != Route::Local {
            return Err(CamError::InvalidState(format!(
                "daemon owner runtime resume requires a local Codex agent, got kind `{}` route `{}`",
                format_agent_kind(&agent.kind),
                format_route(&agent.route)
            )));
        }
        let Some(thread_id) = agent.thread_id.as_deref() else {
            return Ok(AgentResumeApplication {
                result: AgentResumeResult::for_agent(&agent),
                persist_state: false,
            });
        };
        if !self.codex_owner_delivery.has_owner_for_thread(thread_id) {
            let model = agent.model.clone();
            return self
                .codex_owner_delivery
                .resume_existing_thread_with_program_and_model(state, agent, None, model);
        }
        if agent.active_turn_id.is_none() && agent.last_turn_id.is_none() {
            return Ok(AgentResumeApplication {
                result: daemon_codex_owner_missing_turn_proof_resume_result(&agent),
                persist_state: false,
            });
        }
        let result = daemon_codex_owner_ready_resume_result(&agent);
        apply_agent_resume_success(state, name, &result)?;
        Ok(AgentResumeApplication {
            result,
            persist_state: true,
        })
    }

    pub fn send_local_codex_with_owner_runtime(
        &mut self,
        state: &mut AppState,
        request: BuiltSendRequest,
        codex_model: Option<String>,
    ) -> Result<DirectSendApplication, CamError> {
        if !request.use_codex_stdio {
            return Err(CamError::InvalidState(
                "daemon owner runtime send requires Codex stdio delivery".to_string(),
            ));
        }
        let Some(target) = state.find_agent(&request.prepare.target_agent).cloned() else {
            return Err(CamError::NotFound(format!(
                "agent `{}` does not exist",
                request.prepare.target_agent
            )));
        };
        if target.kind != AgentKind::Codex || target.route != Route::Local {
            return Err(CamError::InvalidState(format!(
                "daemon owner runtime send requires a local Codex agent, got kind `{}` route `{}`",
                format_agent_kind(&target.kind),
                format_route(&target.route)
            )));
        }

        if target.thread_id.is_none() {
            let decision = DeliveryDecision {
                state: DeliveryState::Delivered,
                action: DeliveryAction::WakeKnownSession,
                reason: "daemon owner runtime is creating a Codex thread before delivery"
                    .to_string(),
            };
            validate_send_contract_metadata(
                &decision,
                request.expected_delivery.as_ref(),
                request.delivery_action.as_ref(),
            )?;
            let message = message_from_prepare_send_request(state, request.prepare);
            return self
                .codex_owner_delivery
                .start_threadless_and_wake_with_program_and_model(
                    state,
                    target,
                    decision,
                    message,
                    request.codex_program,
                    codex_model,
                );
        }

        let prepare_for_recovery = request.prepare.clone();
        let prepared = prepare_send(state, request.prepare.clone());
        validate_send_contract_metadata(
            &prepared.decision,
            request.expected_delivery.as_ref(),
            request.delivery_action.as_ref(),
        )?;
        if prepared.decision.action == DeliveryAction::WakeKnownSession {
            let Some(target) = prepared.target else {
                return Err(CamError::InvalidState(
                    "daemon owner runtime wake delivery was selected without a target agent"
                        .to_string(),
                ));
            };
            return self
                .codex_owner_delivery
                .wake_known_session_with_program_and_model(
                    state,
                    target,
                    prepared.decision,
                    prepared.message,
                    request.codex_program,
                    codex_model,
                );
        }
        if prepared.decision.action != DeliveryAction::SteerActiveTurn {
            return Err(CamError::ProviderUnavailable(format!(
                "daemon owner runtime only handles active Codex steering or known-thread wake after a thread exists; planned action was `{}`",
                delivery_action_label(&prepared.decision.action)
            )));
        }
        let Some(target) = prepared.target else {
            return Err(CamError::InvalidState(
                "daemon owner runtime direct delivery was selected without a target agent"
                    .to_string(),
            ));
        };
        let steer_result = self.codex_owner_delivery.steer_active_turn(
            state,
            target,
            prepared.decision,
            prepared.message,
        );
        match steer_result {
            Ok(application) => Ok(application),
            Err(error) if can_recover_stale_codex_steer_with_wake(&error, &request) => {
                let wake_prepare = prepare_for_recovery;
                let Some(target) = state.find_agent(&wake_prepare.target_agent).cloned() else {
                    return Err(CamError::NotFound(format!(
                        "agent `{}` does not exist",
                        wake_prepare.target_agent
                    )));
                };
                let wake_decision = DeliveryDecision {
                    state: DeliveryState::Delivered,
                    action: DeliveryAction::WakeKnownSession,
                    reason: "Codex reported no active turn; waking known thread instead"
                        .to_string(),
                };
                let wake_message = message_from_prepare_send_request(state, wake_prepare);
                self.codex_owner_delivery
                    .wake_known_session_with_program_and_model(
                        state,
                        target,
                        wake_decision,
                        wake_message,
                        request.codex_program,
                        codex_model,
                    )
            }
            Err(error) => Err(error),
        }
    }
}

impl Default for DaemonRuntime {
    fn default() -> Self {
        Self::new()
    }
}

fn message_from_prepare_send_request(state: &AppState, request: PrepareSendRequest) -> Message {
    let mut message = Message::new(request.target_agent, request.body);
    message.source_agent = request.source_agent;
    message.source_node = Some(
        request
            .source_node
            .unwrap_or_else(|| state.config.node_name.clone()),
    );
    message.correlation_id = request.correlation_id;
    message.message_type = request.message_type;
    message.strict = request.strict;
    message
}

fn can_recover_stale_codex_steer_with_wake(error: &CamError, request: &BuiltSendRequest) -> bool {
    let error = error.to_string().to_ascii_lowercase();
    request.expected_delivery.is_none()
        && request.delivery_action.is_none()
        && (error.contains("no active turn to steer")
            || error.contains("has no live app-server owner for thread")
            || error.contains("steer envelope requires active turn id"))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SendResult {
    pub ok: bool,
    pub delivered: bool,
    pub received: bool,
    pub queued: bool,
    pub delivery: DeliveryState,
    pub message_id: String,
    pub target_agent: String,
    pub receipt_nonce: Option<String>,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub error_kind: Option<String>,
    pub queue_allowed: Option<bool>,
    pub error: Option<String>,
}

impl SendResult {
    pub fn from_message(message: &Message) -> Self {
        Self {
            ok: false,
            delivered: false,
            received: false,
            queued: false,
            delivery: message.delivery.clone(),
            message_id: message.message_id.clone(),
            target_agent: message.target_agent.clone(),
            receipt_nonce: None,
            thread_id: message.thread_id.clone(),
            turn_id: message.turn_id.clone(),
            error_kind: None,
            queue_allowed: None,
            error: message.error.clone(),
        }
    }

    pub fn apply_message(&mut self, message: &Message, ok: bool, error: Option<String>) {
        self.apply_message_with_error(message, ok, error, None);
    }

    pub fn apply_message_with_error(
        &mut self,
        message: &Message,
        ok: bool,
        error: Option<String>,
        failure: Option<&CamError>,
    ) {
        self.ok = ok;
        self.delivery = message.delivery.clone();
        self.delivered = matches!(
            message.delivery,
            DeliveryState::Delivered | DeliveryState::Steered
        );
        self.received = message.delivery == DeliveryState::Received;
        self.queued = message.delivery == DeliveryState::Queued;
        self.message_id = message.message_id.clone();
        self.target_agent = message.target_agent.clone();
        self.thread_id = message.thread_id.clone();
        self.turn_id = message.turn_id.clone();
        self.error_kind = failure.map(|error| error.kind().to_string());
        self.queue_allowed = failure.map(|error| error.queue_fallback_allowed() && !message.strict);
        self.error = error;
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MessageEventFields {
    pub message_id: String,
    pub target_agent: String,
    pub source_agent: Option<String>,
    pub correlation_id: Option<String>,
    pub message_type: Option<String>,
    pub delivery: DeliveryState,
    pub strict: bool,
    pub planned_delivery: DeliveryState,
    pub delivery_action: &'static str,
    pub outcome_matches_plan: bool,
    pub reason: String,
    pub target_kind: Option<&'static str>,
    pub target_route: Option<String>,
    pub target_peer: Option<String>,
    pub error_kind: Option<&'static str>,
    pub queue_allowed: Option<bool>,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub thread_id_present: bool,
    pub turn_id_present: bool,
}

pub fn message_event_fields(
    message: &Message,
    target: Option<&Agent>,
    decision: &DeliveryDecision,
    reason: &str,
    error: Option<&CamError>,
) -> MessageEventFields {
    let target_kind = target.map(|agent| format_agent_kind(&agent.kind));
    let target_route = target.map(|agent| format_route(&agent.route));
    let target_peer = target.and_then(|agent| match &agent.route {
        Route::Local => None,
        Route::Peer { peer_name } => Some(peer_name.clone()),
    });
    let error_kind = error.map(CamError::kind);
    let queue_allowed = error.map(|error| error.queue_fallback_allowed() && !message.strict);

    MessageEventFields {
        message_id: message.message_id.clone(),
        target_agent: message.target_agent.clone(),
        source_agent: message.source_agent.clone(),
        correlation_id: message.correlation_id.clone(),
        message_type: message.message_type.clone(),
        delivery: message.delivery.clone(),
        strict: message.strict,
        planned_delivery: decision.state.clone(),
        delivery_action: delivery_action_label(&decision.action),
        outcome_matches_plan: message.delivery == decision.state,
        reason: reason.to_string(),
        target_kind,
        target_route,
        target_peer,
        error_kind,
        queue_allowed,
        thread_id: message.thread_id.clone(),
        turn_id: message.turn_id.clone(),
        thread_id_present: message.thread_id.is_some(),
        turn_id_present: message.turn_id.is_some(),
    }
}

pub fn build_send_request(request: BuildSendRequest) -> Result<BuiltSendRequest, CamError> {
    if request.positionals.len() < 2 {
        return Err(CamError::InvalidCommand(
            "send requires <target-agent> and <message>".to_string(),
        ));
    }

    let target_agent = request.positionals[0].clone();
    let body = request.positionals[1..].join(" ");
    validate_canonical_name("agent name", &target_agent)?;
    validate_nonempty_trimmed("message body", &body)?;
    validate_optional_agent_name("--from", request.source_agent.as_deref())?;
    validate_optional_nonempty_trimmed("--source-node", request.source_node.as_deref())?;
    validate_optional_nonempty_trimmed("--correlation-id", request.correlation_id.as_deref())?;
    validate_optional_nonempty_trimmed("--message-type", request.message_type.as_deref())?;
    validate_optional_nonempty_trimmed("--receipt-nonce", request.receipt_nonce.as_deref())?;
    validate_optional_nonempty_trimmed(
        "--expected-delivery",
        request.expected_delivery.as_deref(),
    )?;
    validate_optional_nonempty_trimmed("--delivery-action", request.delivery_action.as_deref())?;
    if let Some(receipt_nonce) = request.receipt_nonce.as_deref() {
        validate_receipt_nonce(receipt_nonce)?;
    }
    let expected_delivery = request
        .expected_delivery
        .as_deref()
        .map(parse_delivery_state_label)
        .transpose()
        .map_err(|error| CamError::InvalidCommand(format!("--expected-delivery {error}")))?;
    let delivery_action = request
        .delivery_action
        .as_deref()
        .map(parse_delivery_action_label)
        .transpose()
        .map_err(|error| CamError::InvalidCommand(format!("--delivery-action {error}")))?;
    if request.codex_program.is_some() && !request.use_codex_stdio {
        return Err(CamError::InvalidCommand(
            "--codex-program requires --codex-stdio".to_string(),
        ));
    }
    validate_optional_nonempty_trimmed("--codex-program", request.codex_program.as_deref())?;

    Ok(BuiltSendRequest {
        prepare: PrepareSendRequest {
            target_agent,
            body,
            source_agent: request.source_agent,
            source_node: request.source_node,
            correlation_id: request.correlation_id,
            message_type: request.message_type,
            strict: request.strict,
        },
        receipt_nonce: request.receipt_nonce,
        expected_delivery,
        delivery_action,
        use_codex_stdio: request.use_codex_stdio,
        codex_program: request.codex_program,
    })
}

pub fn prepare_send(state: &AppState, request: PrepareSendRequest) -> PreparedSend {
    let target = state.find_agent(&request.target_agent).cloned();
    let decision = decide_delivery(target.as_ref(), request.strict);
    let mut message = Message::new(request.target_agent, request.body);
    message.source_agent = request.source_agent;
    message.source_node = Some(
        request
            .source_node
            .unwrap_or_else(|| state.config.node_name.clone()),
    );
    message.correlation_id = request.correlation_id;
    message.message_type = request.message_type;
    message.strict = request.strict;
    if let Some(agent) = &target {
        message.thread_id = agent.thread_id.clone();
    }

    PreparedSend {
        target,
        decision,
        message,
    }
}

pub fn validate_receipt_nonce(value: &str) -> Result<(), CamError> {
    let parsed = Uuid::parse_str(value)
        .map_err(|_| CamError::InvalidCommand("--receipt-nonce must be a UUID".to_string()))?;
    if parsed.to_string() != value {
        return Err(CamError::InvalidCommand(
            "--receipt-nonce must be a canonical lowercase UUID".to_string(),
        ));
    }
    Ok(())
}

pub fn apply_prepared_send_decision(
    state: &mut AppState,
    prepared: PreparedSend,
) -> Result<SendApplication, CamError> {
    let PreparedSend {
        target,
        decision,
        mut message,
    } = prepared;

    match decision.action {
        DeliveryAction::StoreInVirtualInbox => {
            message.delivery = DeliveryState::Received;
            message.updated_at = now_utc();
            state.push_mailbox_message(message.clone());
            Ok(SendApplication::Local(LocalSendApplication {
                target,
                decision,
                message,
                event_type: "message.received",
                ok: true,
                result_error: None,
                persist_state: true,
            }))
        }
        DeliveryAction::QueueFallback => {
            message.delivery = DeliveryState::Queued;
            message.error = Some(decision.reason.clone());
            message.updated_at = now_utc();
            state.push_mailbox_message(message.clone());
            let result_error = message.error.clone();
            Ok(SendApplication::Local(LocalSendApplication {
                target,
                decision,
                message,
                event_type: "message.queued",
                ok: true,
                result_error,
                persist_state: true,
            }))
        }
        DeliveryAction::FailLoudly => {
            message.delivery = DeliveryState::Failed;
            message.error = Some(decision.reason.clone());
            message.updated_at = now_utc();
            let result_error = message.error.clone();
            Ok(SendApplication::Local(LocalSendApplication {
                target,
                decision,
                message,
                event_type: "message.failed",
                ok: false,
                result_error,
                persist_state: false,
            }))
        }
        DeliveryAction::SteerActiveTurn | DeliveryAction::WakeKnownSession => {
            let Some(target) = target else {
                return Err(CamError::InvalidState(
                    "direct delivery was selected without a target agent".to_string(),
                ));
            };
            Ok(SendApplication::Direct(DirectSendApplication {
                target,
                decision,
                message,
            }))
        }
    }
}

pub fn apply_codex_threadless_send_success(
    state: &mut AppState,
    target: Agent,
    decision: DeliveryDecision,
    mut message: Message,
    outcome: ProviderSendOutcome,
) -> Result<DirectSendApplication, CamError> {
    if outcome.delivery != DeliveryState::Delivered {
        return Err(CamError::ProviderContractViolation(format!(
            "Codex thread/start then send returned `{}` instead of delivered",
            delivery_state_label(&outcome.delivery)
        )));
    }
    let thread_id = outcome.thread_id.clone().ok_or_else(|| {
        CamError::ProviderContractViolation(
            "Codex thread/start then send returned no thread id".to_string(),
        )
    })?;
    let turn_id = outcome.turn_id.clone().ok_or_else(|| {
        CamError::ProviderContractViolation(
            "Codex thread/start then send returned no turn id".to_string(),
        )
    })?;
    if outcome.target_agent.as_deref() != Some(target.name.as_str()) {
        return Err(CamError::ProviderContractViolation(format!(
            "Codex thread/start then send targeted `{}` instead of `{}`",
            outcome.target_agent.as_deref().unwrap_or("<missing>"),
            target.name
        )));
    }
    if outcome.message_id.as_deref() != Some(message.message_id.as_str()) {
        return Err(CamError::ProviderContractViolation(format!(
            "Codex thread/start then send echoed message `{}` instead of `{}`",
            outcome.message_id.as_deref().unwrap_or("<missing>"),
            message.message_id
        )));
    }

    message.delivery = DeliveryState::Delivered;
    message.thread_id = Some(thread_id.clone());
    message.turn_id = Some(turn_id.clone());
    message.updated_at = now_utc();

    let agent = state
        .find_agent_mut(&target.name)
        .ok_or_else(|| CamError::NotFound(format!("agent `{}` does not exist", target.name)))?;
    agent.thread_id = Some(thread_id);
    agent.status = AgentStatus::Idle;
    agent.active_turn_id = None;
    agent.last_turn_id = Some(turn_id);
    agent.last_error = None;
    agent.updated_at = now_utc();

    Ok(DirectSendApplication {
        target: agent.clone(),
        decision,
        message,
    })
}

pub fn apply_codex_owner_steer_success(
    state: &mut AppState,
    target: Agent,
    decision: DeliveryDecision,
    mut message: Message,
    outcome: ProviderSendOutcome,
) -> Result<DirectSendApplication, CamError> {
    if outcome.delivery != DeliveryState::Steered {
        return Err(CamError::ProviderContractViolation(format!(
            "Codex owner steer returned `{}` instead of steered",
            delivery_state_label(&outcome.delivery)
        )));
    }
    let thread_id = outcome.thread_id.clone().ok_or_else(|| {
        CamError::ProviderContractViolation("Codex owner steer returned no thread id".to_string())
    })?;
    let turn_id = outcome.turn_id.clone().ok_or_else(|| {
        CamError::ProviderContractViolation("Codex owner steer returned no turn id".to_string())
    })?;
    if outcome.target_agent.as_deref() != Some(target.name.as_str()) {
        return Err(CamError::ProviderContractViolation(format!(
            "Codex owner steer targeted `{}` instead of `{}`",
            outcome.target_agent.as_deref().unwrap_or("<missing>"),
            target.name
        )));
    }
    if outcome.message_id.as_deref() != Some(message.message_id.as_str()) {
        return Err(CamError::ProviderContractViolation(format!(
            "Codex owner steer echoed message `{}` instead of `{}`",
            outcome.message_id.as_deref().unwrap_or("<missing>"),
            message.message_id
        )));
    }
    if target.thread_id.as_deref() != Some(thread_id.as_str()) {
        return Err(CamError::ProviderContractViolation(format!(
            "Codex owner steer attempted to remap thread `{}` to `{thread_id}`",
            target.thread_id.as_deref().unwrap_or("<missing>")
        )));
    }
    if target.active_turn_id.as_deref() != Some(turn_id.as_str()) {
        return Err(CamError::ProviderContractViolation(format!(
            "Codex owner steer returned turn `{turn_id}` instead of active turn `{}`",
            target.active_turn_id.as_deref().unwrap_or("<missing>")
        )));
    }

    message.delivery = DeliveryState::Steered;
    message.thread_id = Some(thread_id);
    message.turn_id = Some(turn_id.clone());
    message.updated_at = now_utc();

    let agent = state
        .find_agent_mut(&target.name)
        .ok_or_else(|| CamError::NotFound(format!("agent `{}` does not exist", target.name)))?;
    agent.status = AgentStatus::Idle;
    agent.active_turn_id = None;
    agent.last_turn_id = Some(turn_id);
    agent.last_error = None;
    agent.updated_at = now_utc();

    Ok(DirectSendApplication {
        target: agent.clone(),
        decision,
        message,
    })
}

pub fn apply_codex_owner_wake_success(
    state: &mut AppState,
    target: Agent,
    decision: DeliveryDecision,
    mut message: Message,
    outcome: ProviderSendOutcome,
) -> Result<DirectSendApplication, CamError> {
    if outcome.delivery != DeliveryState::Delivered {
        return Err(CamError::ProviderContractViolation(format!(
            "Codex owner wake returned `{}` instead of delivered",
            delivery_state_label(&outcome.delivery)
        )));
    }
    let thread_id = outcome.thread_id.ok_or_else(|| {
        CamError::ProviderContractViolation("Codex owner wake returned no thread id".to_string())
    })?;
    let turn_id = outcome.turn_id.ok_or_else(|| {
        CamError::ProviderContractViolation("Codex owner wake returned no turn id".to_string())
    })?;
    if outcome.target_agent.as_deref() != Some(target.name.as_str()) {
        return Err(CamError::ProviderContractViolation(format!(
            "Codex owner wake targeted `{}` instead of `{}`",
            outcome
                .target_agent
                .unwrap_or_else(|| "<missing>".to_string()),
            target.name
        )));
    }
    if outcome.message_id.as_deref() != Some(message.message_id.as_str()) {
        return Err(CamError::ProviderContractViolation(format!(
            "Codex owner wake echoed message `{}` instead of `{}`",
            outcome
                .message_id
                .unwrap_or_else(|| "<missing>".to_string()),
            message.message_id
        )));
    }
    if Some(thread_id.as_str()) != target.thread_id.as_deref() {
        return Err(CamError::ProviderContractViolation(format!(
            "Codex owner wake attempted to remap thread `{}` to `{thread_id}`",
            target.thread_id.as_deref().unwrap_or("<missing>")
        )));
    }

    message.delivery = DeliveryState::Delivered;
    message.thread_id = Some(thread_id);
    message.turn_id = Some(turn_id.clone());
    message.updated_at = now_utc();

    let agent = state
        .find_agent_mut(&target.name)
        .ok_or_else(|| CamError::NotFound(format!("agent `{}` does not exist", target.name)))?;
    agent.status = AgentStatus::Idle;
    agent.active_turn_id = None;
    agent.last_turn_id = Some(turn_id);
    agent.last_error = None;
    agent.updated_at = now_utc();

    Ok(DirectSendApplication {
        target: agent.clone(),
        decision,
        message,
    })
}

pub fn local_send_decision_error(application: &LocalSendApplication) -> Option<CamError> {
    match application.decision.action {
        DeliveryAction::StoreInVirtualInbox => None,
        DeliveryAction::QueueFallback => Some(CamError::DeliveryFailed(
            application
                .result_error
                .clone()
                .unwrap_or_else(|| application.decision.reason.clone()),
        )),
        DeliveryAction::FailLoudly if application.target.is_none() => {
            Some(CamError::NotFound(application.decision.reason.clone()))
        }
        DeliveryAction::FailLoudly => {
            Some(CamError::InvalidState(application.decision.reason.clone()))
        }
        DeliveryAction::SteerActiveTurn | DeliveryAction::WakeKnownSession => None,
    }
}

pub fn validate_send_contract_metadata(
    decision: &DeliveryDecision,
    expected_delivery: Option<&DeliveryState>,
    delivery_action: Option<&DeliveryAction>,
) -> Result<(), CamError> {
    if let Some(expected_delivery) = expected_delivery {
        if expected_delivery != &decision.state {
            return Err(CamError::InvalidCommand(format!(
                "--expected-delivery `{}` does not match planned delivery `{}`",
                delivery_state_label(expected_delivery),
                delivery_state_label(&decision.state)
            )));
        }
    }
    if let Some(delivery_action) = delivery_action {
        if delivery_action != &decision.action {
            return Err(CamError::InvalidCommand(format!(
                "--delivery-action `{}` does not match planned action `{}`",
                delivery_action_label(delivery_action),
                delivery_action_label(&decision.action)
            )));
        }
    }
    Ok(())
}

pub fn apply_direct_delivery_failure(
    state: &mut AppState,
    target: Agent,
    decision: DeliveryDecision,
    mut message: Message,
    failure: &CamError,
    failure_kind: DirectDeliveryFailureKind,
) -> LocalSendApplication {
    let (delivery, event_type, ok, persist_state, reason) = match failure_kind {
        DirectDeliveryFailureKind::Unqueueable => (
            DeliveryState::Failed,
            "message.failed",
            false,
            false,
            format!(
                "{}; direct delivery proof failed and queue fallback is not allowed: {failure}",
                decision.reason
            ),
        ),
        DirectDeliveryFailureKind::Recoverable if message.strict => (
            DeliveryState::Failed,
            "message.failed",
            false,
            false,
            format!(
                "{}; provider delivery failed and strict mode forbids queue fallback: {failure}",
                decision.reason
            ),
        ),
        DirectDeliveryFailureKind::Recoverable => (
            DeliveryState::Queued,
            "message.queued",
            true,
            true,
            format!(
                "{}; queued because direct provider delivery did not complete: {failure}",
                decision.reason
            ),
        ),
    };

    message.delivery = delivery;
    message.error = Some(reason.clone());
    message.updated_at = now_utc();
    if persist_state {
        state.push_mailbox_message(message.clone());
    }

    LocalSendApplication {
        target: Some(target),
        decision,
        message,
        event_type,
        ok,
        result_error: Some(reason),
        persist_state,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateAgentRequest {
    pub name: String,
    pub kind: AgentKind,
    pub thread_id: Option<String>,
    pub thread_source: ThreadSource,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub effort: Option<Effort>,
    pub service_tier: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildCreateAgentRequest {
    pub name: String,
    pub source: Option<String>,
    pub cwd: Option<String>,
    pub thread_id: Option<String>,
    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub effort: Option<String>,
    pub speed: Option<String>,
    pub service_tier: Option<String>,
}

pub fn build_create_agent_request(
    request: BuildCreateAgentRequest,
) -> Result<CreateAgentRequest, CamError> {
    validate_create_agent_name(&request.name)?;
    validate_optional_nonempty_trimmed("--cwd", request.cwd.as_deref())?;
    validate_optional_nonempty_trimmed("--thread-id", request.thread_id.as_deref())?;
    validate_optional_nonempty_trimmed("--model", request.model.as_deref())?;
    validate_optional_nonempty_trimmed("--model-provider", request.model_provider.as_deref())?;
    validate_optional_nonempty_trimmed("--speed", request.speed.as_deref())?;
    validate_optional_nonempty_trimmed("--service-tier", request.service_tier.as_deref())?;

    let source = request.source.as_deref().unwrap_or("codex");
    let (kind, thread_source) = parse_create_agent_source(source)?;
    match kind {
        AgentKind::Codex if request.cwd.is_none() => {
            return Err(CamError::InvalidCommand(
                "codex agents require --cwd <path>".to_string(),
            ));
        }
        AgentKind::AgySession if request.thread_id.is_none() => {
            return Err(CamError::InvalidCommand(
                "agy agents require --thread-id <session-id>".to_string(),
            ));
        }
        AgentKind::RemoteMirror => {
            return Err(CamError::InvalidCommand(
                "remote mirror agents must come from peer sync, not local create".to_string(),
            ));
        }
        AgentKind::VirtualInbox | AgentKind::Codex | AgentKind::AgySession => {}
    }

    Ok(CreateAgentRequest {
        name: request.name,
        kind,
        thread_id: request.thread_id,
        thread_source,
        cwd: request.cwd,
        model: request.model,
        model_provider: request.model_provider,
        effort: request
            .effort
            .as_deref()
            .map(parse_create_agent_effort)
            .transpose()?,
        service_tier: resolve_create_agent_service_tier(
            request.speed.as_deref(),
            request.service_tier.as_deref(),
        )?,
    })
}

pub fn create_agent(state: &mut AppState, request: CreateAgentRequest) -> Result<Agent, CamError> {
    validate_create_agent_name(&request.name)?;
    if request.kind == AgentKind::RemoteMirror
        || request.thread_source == ThreadSource::RemoteMirror
    {
        return Err(CamError::InvalidState(
            "remote mirror agents must come from peer sync, not local create".to_string(),
        ));
    }
    if let Some(thread_id) = request.thread_id.as_deref() {
        if state.agents.iter().any(|existing| {
            matches!(existing.route, Route::Local)
                && existing.thread_id.as_deref() == Some(thread_id)
        }) {
            return Err(CamError::InvalidState(format!(
                "local thread/session `{thread_id}` is already mapped to an agent"
            )));
        }
    }
    let now = now_utc();
    let agent = Agent {
        name: request.name,
        kind: request.kind,
        thread_id: request.thread_id,
        thread_source: request.thread_source,
        cwd: request.cwd,
        route: Route::Local,
        status: AgentStatus::Idle,
        chat_status: ChatStatus::Unknown,
        chat_status_source: ChatStatusSource::Unknown,
        active_turn_id: None,
        last_turn_id: None,
        model: request.model,
        model_provider: request.model_provider,
        effort: request.effort,
        service_tier: request.service_tier,
        created_at: now.clone(),
        updated_at: now,
        last_error: None,
    };
    state.add_agent(agent.clone())?;
    Ok(agent)
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AgentCreatedEventFields {
    pub agent: String,
    pub kind: &'static str,
    pub thread_id_present: bool,
    pub cwd_present: bool,
}

pub fn agent_created_event_fields(agent: &Agent) -> AgentCreatedEventFields {
    AgentCreatedEventFields {
        agent: agent.name.clone(),
        kind: format_agent_kind(&agent.kind),
        thread_id_present: agent.thread_id.is_some(),
        cwd_present: agent.cwd.is_some(),
    }
}

fn validate_create_agent_name(name: &str) -> Result<(), CamError> {
    validate_canonical_name("agent name", name)
}

fn parse_create_agent_source(source: &str) -> Result<(AgentKind, ThreadSource), CamError> {
    match source {
        "codex" => Ok((AgentKind::Codex, ThreadSource::Codex)),
        "agy" | "antigravity" | "agy_session" => {
            Ok((AgentKind::AgySession, ThreadSource::AgySession))
        }
        "mailbox" | "virtual_inbox" | "virtual-inbox" => {
            Ok((AgentKind::VirtualInbox, ThreadSource::Mailbox))
        }
        other => Err(CamError::InvalidCommand(format!(
            "unknown agent source `{other}`"
        ))),
    }
}

fn parse_create_agent_effort(value: &str) -> Result<Effort, CamError> {
    match value {
        "minimal" => Ok(Effort::Minimal),
        "low" => Ok(Effort::Low),
        "medium" => Ok(Effort::Medium),
        "high" => Ok(Effort::High),
        "xhigh" => Ok(Effort::Xhigh),
        other => Err(CamError::InvalidCommand(format!(
            "unknown effort `{other}`; expected minimal, low, medium, high, or xhigh"
        ))),
    }
}

fn resolve_create_agent_service_tier(
    speed: Option<&str>,
    service_tier: Option<&str>,
) -> Result<Option<String>, CamError> {
    if speed.is_some() && service_tier.is_some() {
        return Err(CamError::InvalidCommand(
            "--speed cannot be combined with --service-tier; choose one service-tier control"
                .to_string(),
        ));
    }

    if let Some(speed) = speed {
        return match speed {
            "standard" => Ok(None),
            "fast" => Ok(Some("fast".to_string())),
            other => Err(CamError::InvalidCommand(format!(
                "unknown speed `{other}`; expected standard or fast"
            ))),
        };
    }

    Ok(service_tier.map(str::to_string))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetModelUpdate {
    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub effort: Option<Effort>,
    pub service_tier: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildSetModelUpdateRequest {
    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub effort: Option<String>,
    pub speed: Option<String>,
    pub service_tier: Option<String>,
}

pub fn build_set_model_update(
    request: BuildSetModelUpdateRequest,
) -> Result<SetModelUpdate, CamError> {
    validate_optional_nonempty_trimmed("--model", request.model.as_deref())?;
    validate_optional_nonempty_trimmed("--model-provider", request.model_provider.as_deref())?;
    validate_optional_nonempty_trimmed("--speed", request.speed.as_deref())?;
    validate_optional_nonempty_trimmed("--service-tier", request.service_tier.as_deref())?;

    if request.model.is_none()
        && request.model_provider.is_none()
        && request.effort.is_none()
        && request.speed.is_none()
        && request.service_tier.is_none()
    {
        return Err(CamError::InvalidCommand(
            "agent set-model requires at least one model option".to_string(),
        ));
    }

    Ok(SetModelUpdate {
        model: request.model,
        model_provider: request.model_provider,
        effort: request
            .effort
            .as_deref()
            .map(parse_create_agent_effort)
            .transpose()?,
        service_tier: match resolve_create_agent_service_tier(
            request.speed.as_deref(),
            request.service_tier.as_deref(),
        )? {
            Some(service_tier) => Some(Some(service_tier)),
            None if request.speed.is_some() => Some(None),
            None => None,
        },
    })
}

pub fn set_agent_model(
    state: &mut AppState,
    name: &str,
    update: SetModelUpdate,
) -> Result<Agent, CamError> {
    let agent = state
        .find_agent_mut(name)
        .ok_or_else(|| CamError::NotFound(format!("agent `{name}` does not exist")))?;
    let original = agent.clone();
    let mut updated = original.clone();

    if let Some(model) = update.model {
        updated.model = Some(model);
    }
    if let Some(model_provider) = update.model_provider {
        updated.model_provider = Some(model_provider);
    }
    if let Some(effort) = update.effort {
        updated.effort = Some(effort);
    }
    if let Some(service_tier) = update.service_tier {
        updated.service_tier = service_tier;
    }
    updated.updated_at = now_utc();

    if updated.name != original.name
        || updated.kind != original.kind
        || updated.thread_id != original.thread_id
        || updated.thread_source != original.thread_source
        || updated.cwd != original.cwd
        || updated.route != original.route
        || updated.status != original.status
        || updated.active_turn_id != original.active_turn_id
        || updated.last_turn_id != original.last_turn_id
        || updated.created_at != original.created_at
        || updated.last_error != original.last_error
    {
        return Err(CamError::InvalidState(
            "model update attempted to mutate agent identity".to_string(),
        ));
    }

    *agent = updated.clone();
    Ok(updated)
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AgentModelUpdatedEventFields {
    pub agent: String,
    pub thread_id: Option<String>,
    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub effort: Option<Effort>,
    pub service_tier: Option<String>,
}

pub fn agent_model_updated_event_fields(agent: &Agent) -> AgentModelUpdatedEventFields {
    AgentModelUpdatedEventFields {
        agent: agent.name.clone(),
        thread_id: agent.thread_id.clone(),
        model: agent.model.clone(),
        model_provider: agent.model_provider.clone(),
        effort: agent.effort.clone(),
        service_tier: agent.service_tier.clone(),
    }
}
