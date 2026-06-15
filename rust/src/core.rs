use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    pub node_name: String,
    pub headless: bool,
    pub created_at: String,
    pub updated_at: String,
}

impl Config {
    pub fn new(node_name: impl Into<String>) -> Self {
        let now = now_utc();
        Self {
            node_name: node_name.into(),
            headless: true,
            created_at: now.clone(),
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonState {
    pub desired_state: DaemonDesiredState,
    pub observed_state: DaemonObservedState,
    pub pid: Option<u32>,
    #[serde(default)]
    pub instance_id: Option<String>,
    #[serde(default)]
    pub identity_nonce_ref: Option<String>,
    #[serde(default)]
    pub identity_verified_at: Option<String>,
    pub started_at: Option<String>,
    pub stopped_at: Option<String>,
    pub last_heartbeat_at: Option<String>,
    pub bind: String,
    #[serde(default = "default_daemon_port")]
    pub port: u16,
    pub headless: bool,
    pub version: String,
    pub node_name: String,
    pub startup_phase: String,
    pub shutdown_reason: Option<String>,
    pub last_error: Option<String>,
    pub updated_at: String,
}

impl DaemonState {
    pub fn new(node_name: impl Into<String>, headless: bool) -> Self {
        Self {
            desired_state: DaemonDesiredState::Stopped,
            observed_state: DaemonObservedState::NotRunning,
            pid: None,
            instance_id: None,
            identity_nonce_ref: None,
            identity_verified_at: None,
            started_at: None,
            stopped_at: None,
            last_heartbeat_at: None,
            bind: "127.0.0.1".to_string(),
            port: default_daemon_port(),
            headless,
            version: env!("CARGO_PKG_VERSION").to_string(),
            node_name: node_name.into(),
            startup_phase: "not_started".to_string(),
            shutdown_reason: None,
            last_error: None,
            updated_at: now_utc(),
        }
    }

    pub fn start_requested(
        &mut self,
        headless: bool,
        instance_id: impl Into<String>,
        identity_nonce_ref: impl Into<String>,
    ) {
        let now = now_utc();
        self.desired_state = DaemonDesiredState::StartRequested;
        self.observed_state = DaemonObservedState::NotImplemented;
        self.pid = None;
        self.instance_id = Some(instance_id.into());
        self.identity_nonce_ref = Some(identity_nonce_ref.into());
        self.identity_verified_at = None;
        self.started_at = None;
        self.stopped_at = None;
        self.last_heartbeat_at = None;
        self.headless = headless;
        self.startup_phase = "blocked_before_process_spawn".to_string();
        self.shutdown_reason = None;
        self.last_error = Some(
            "daemon process supervisor is unavailable in this build; no process was started"
                .to_string(),
        );
        self.updated_at = now;
    }

    pub fn mark_running(
        &mut self,
        headless: bool,
        pid: u32,
        instance_id: impl Into<String>,
        identity_nonce_ref: impl Into<String>,
    ) {
        let now = now_utc();
        self.desired_state = DaemonDesiredState::StartRequested;
        self.observed_state = DaemonObservedState::Running;
        self.pid = Some(pid);
        self.instance_id = Some(instance_id.into());
        self.identity_nonce_ref = Some(identity_nonce_ref.into());
        self.identity_verified_at = Some(now.clone());
        self.started_at = Some(now.clone());
        self.stopped_at = None;
        self.last_heartbeat_at = Some(now.clone());
        self.headless = headless;
        self.startup_phase = "serving_loopback_http".to_string();
        self.shutdown_reason = None;
        self.last_error = None;
        self.updated_at = now;
    }

    pub fn heartbeat(&mut self) {
        let now = now_utc();
        self.last_heartbeat_at = Some(now.clone());
        self.updated_at = now;
    }

    pub fn stop_requested(&mut self) {
        let now = now_utc();
        self.desired_state = DaemonDesiredState::Stopped;
        self.observed_state = DaemonObservedState::NotRunning;
        self.pid = None;
        self.instance_id = None;
        self.identity_nonce_ref = None;
        self.identity_verified_at = None;
        self.started_at = None;
        self.stopped_at = Some(now.clone());
        self.last_heartbeat_at = None;
        self.startup_phase = "not_started".to_string();
        self.shutdown_reason = Some("operator_requested_stop".to_string());
        self.last_error = None;
        self.updated_at = now;
    }
}

pub fn default_daemon_port() -> u16 {
    37631
}

impl Default for DaemonState {
    fn default() -> Self {
        Self::new(default_node_name(), true)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonDesiredState {
    Stopped,
    StartRequested,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonObservedState {
    NotRunning,
    NotImplemented,
    Stale,
    Running,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Agent {
    pub name: String,
    pub kind: AgentKind,
    pub thread_id: Option<String>,
    pub thread_source: ThreadSource,
    pub cwd: Option<String>,
    pub route: Route,
    pub status: AgentStatus,
    #[serde(default)]
    pub chat_status: ChatStatus,
    #[serde(default)]
    pub chat_status_source: ChatStatusSource,
    pub active_turn_id: Option<String>,
    pub last_turn_id: Option<String>,
    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub effort: Option<Effort>,
    pub service_tier: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub last_error: Option<String>,
}

impl Agent {
    pub fn builtin_operator() -> Self {
        let now = now_utc();
        Self {
            name: "operator".to_string(),
            kind: AgentKind::VirtualInbox,
            thread_id: None,
            thread_source: ThreadSource::Mailbox,
            cwd: None,
            route: Route::Local,
            status: AgentStatus::Idle,
            chat_status: ChatStatus::Unknown,
            chat_status_source: ChatStatusSource::Unknown,
            active_turn_id: None,
            last_turn_id: None,
            model: None,
            model_provider: None,
            effort: None,
            service_tier: None,
            created_at: now.clone(),
            updated_at: now,
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Message {
    pub message_id: String,
    pub target_agent: String,
    pub source_agent: Option<String>,
    pub source_node: Option<String>,
    pub body: String,
    pub correlation_id: Option<String>,
    pub message_type: Option<String>,
    pub delivery: DeliveryState,
    pub strict: bool,
    pub error: Option<String>,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl Message {
    pub fn new(target_agent: impl Into<String>, body: impl Into<String>) -> Self {
        let now = now_utc();
        Self {
            message_id: Uuid::new_v4().to_string(),
            target_agent: target_agent.into(),
            source_agent: None,
            source_node: None,
            body: body.into(),
            correlation_id: None,
            message_type: None,
            delivery: DeliveryState::Started,
            strict: false,
            error: None,
            thread_id: None,
            turn_id: None,
            created_at: now.clone(),
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscoveryRow {
    pub thread_id: Option<String>,
    pub title: String,
    pub cwd: Option<String>,
    #[serde(default)]
    pub workspace_in_project: bool,
    pub source: DiscoverySource,
    pub route: Route,
    pub peer_name: Option<String>,
    pub thread_source: ThreadSource,
    #[serde(default)]
    pub chat_status: ChatStatus,
    #[serde(default)]
    pub chat_status_source: ChatStatusSource,
    pub updated_at: String,
    pub disposition: DiscoveryDisposition,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Peer {
    pub name: String,
    pub transport: PeerTransport,
    pub ssh_target: Option<String>,
    pub key_path: Option<String>,
    pub remote_root: Option<String>,
    pub state: PeerState,
    pub remote_node_name: Option<String>,
    pub last_sync_at: Option<String>,
    pub last_sync_error: Option<String>,
    #[serde(default)]
    pub last_sync_error_kind: Option<String>,
    pub inventory_source: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    Codex,
    VirtualInbox,
    AgySession,
    RemoteMirror,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThreadSource {
    Codex,
    AgySession,
    Mailbox,
    GuiOnly,
    RemoteMirror,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    Local,
    Peer { peer_name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Idle,
    Active,
    Error,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChatStatus {
    Active,
    Archived,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChatStatusSource {
    ThreadDatabase,
    DesktopThreadDatabase,
    RemoteInventory,
    #[serde(alias = "session_presence")]
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    Started,
    Steered,
    Delivered,
    Received,
    Queued,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiscoverySource {
    ThreadDatabase,
    CodexState,
    SessionIndex,
    Rollout,
    RemoteInventory,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryDisposition {
    Approved,
    Candidate,
    Quarantined,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PeerTransport {
    Ssh,
    CodexManaged,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PeerState {
    Unknown,
    Verified,
    Mirrored,
    MirroredDegraded,
    SyncFailed,
}

pub fn default_node_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "local-cam".to_string())
}

pub fn now_utc() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}
