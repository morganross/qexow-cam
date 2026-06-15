use crate::core::{
    Agent, AgentKind, AgentStatus, ChatStatus, ChatStatusSource, Config, DeliveryState,
    DiscoveryDisposition, DiscoveryRow, Message, Peer, PeerState, Route, ThreadSource, now_utc,
};
use crate::delivery::DeliveryAction;
use crate::errors::CamError;
use crate::resume::AgentResumeResult;
use crate::services::{AgentReadOptions, AgentReadSnapshot};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::process::Command;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InventoryExport {
    pub inventory_version: u32,
    pub node_name: String,
    pub exported_at: String,
    pub agents: Vec<Agent>,
    pub peers: Vec<Peer>,
    pub discovery: InventoryDiscoverySummary,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InventoryDiscoverySummary {
    pub rows: usize,
    pub approved: usize,
    pub candidate: usize,
    pub quarantined: usize,
    pub rejected: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RemoteMirrorApplyResult {
    pub peer_name: String,
    pub remote_agents_seen: usize,
    pub mirrored_agents_added: usize,
    pub mirrored_agents_updated: usize,
    pub mirrored_agents_skipped: usize,
    pub mirrored_agents_stale: usize,
    pub collision_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PeerInventoryCommand {
    pub program: String,
    pub args: Vec<String>,
    pub remote_command: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerInventoryFetch {
    pub command: PeerInventoryCommand,
    pub stdout: String,
}

#[derive(Debug)]
pub struct PeerInventoryFetchError {
    pub error: CamError,
    pub remote_command_attempted: bool,
}

impl PeerInventoryFetchError {
    pub fn before_remote_command(error: CamError) -> Self {
        Self {
            error,
            remote_command_attempted: false,
        }
    }

    pub fn after_remote_command(error: CamError) -> Self {
        Self {
            error,
            remote_command_attempted: true,
        }
    }
}

pub trait PeerInventoryFetcher {
    fn fetch_inventory(&self, peer: &Peer) -> Result<PeerInventoryFetch, PeerInventoryFetchError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerMessageSendRequest {
    pub remote_agent_name: String,
    pub remote_thread_id: Option<String>,
    pub remote_cwd: Option<String>,
    pub message: Message,
    pub expected_delivery: DeliveryState,
    pub delivery_action: DeliveryAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PeerMessageCommand {
    pub program: String,
    pub args: Vec<String>,
    pub remote_command: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerMessageSendOutcome {
    pub delivery: DeliveryState,
    pub message_id: Option<String>,
    pub target_agent: Option<String>,
    pub receipt_nonce: Option<String>,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug)]
pub struct PeerMessageSendError {
    pub error: CamError,
    pub remote_command_attempted: bool,
}

impl PeerMessageSendError {
    pub fn before_remote_command(error: CamError) -> Self {
        Self {
            error,
            remote_command_attempted: false,
        }
    }

    pub fn after_remote_command(error: CamError) -> Self {
        Self {
            error,
            remote_command_attempted: true,
        }
    }
}

pub trait PeerMessageSender {
    fn send_message(
        &self,
        peer: &Peer,
        request: &PeerMessageSendRequest,
    ) -> Result<PeerMessageSendOutcome, PeerMessageSendError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerAgentReadCommand {
    pub program: String,
    pub args: Vec<String>,
    pub remote_command: String,
}

#[derive(Debug)]
pub struct PeerAgentReadError {
    pub error: CamError,
    pub remote_command_attempted: bool,
}

impl PeerAgentReadError {
    pub fn before_remote_command(error: CamError) -> Self {
        Self {
            error,
            remote_command_attempted: false,
        }
    }

    pub fn after_remote_command(error: CamError) -> Self {
        Self {
            error,
            remote_command_attempted: true,
        }
    }
}

pub trait PeerAgentReadFetcher {
    fn read_agent(
        &self,
        peer: &Peer,
        remote_agent_name: &str,
        options: AgentReadOptions,
    ) -> Result<AgentReadSnapshot, PeerAgentReadError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerAgentResumeCommand {
    pub program: String,
    pub args: Vec<String>,
    pub remote_command: String,
}

#[derive(Debug)]
pub struct PeerAgentResumeError {
    pub error: CamError,
    pub remote_command_attempted: bool,
}

impl PeerAgentResumeError {
    pub fn before_remote_command(error: CamError) -> Self {
        Self {
            error,
            remote_command_attempted: false,
        }
    }

    pub fn after_remote_command(error: CamError) -> Self {
        Self {
            error,
            remote_command_attempted: true,
        }
    }
}

pub trait PeerAgentResumeFetcher {
    fn resume_agent(
        &self,
        peer: &Peer,
        remote_agent_name: &str,
    ) -> Result<AgentResumeResult, PeerAgentResumeError>;
}

pub struct SshPeerMessageSender;

pub struct SshPeerAgentReadFetcher;

pub struct SshPeerAgentResumeFetcher;

pub struct SshInventoryFetcher;

impl PeerMessageSender for SshPeerMessageSender {
    fn send_message(
        &self,
        peer: &Peer,
        request: &PeerMessageSendRequest,
    ) -> Result<PeerMessageSendOutcome, PeerMessageSendError> {
        let command = build_ssh_message_command(peer, request)
            .map_err(PeerMessageSendError::before_remote_command)?;
        let output = Command::new(&command.program)
            .args(&command.args)
            .output()
            .map_err(|error| PeerMessageSendError::after_remote_command(error.into()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let detail = if stderr.is_empty() {
                format!("ssh exited with status {}", output.status)
            } else {
                stderr
            };
            return Err(PeerMessageSendError::after_remote_command(
                CamError::DeliveryFailed(format!(
                    "peer message send command failed for `{}`: {detail}",
                    peer.name
                )),
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        parse_peer_message_send_output(&stdout).map_err(PeerMessageSendError::after_remote_command)
    }
}

impl PeerAgentReadFetcher for SshPeerAgentReadFetcher {
    fn read_agent(
        &self,
        peer: &Peer,
        remote_agent_name: &str,
        options: AgentReadOptions,
    ) -> Result<AgentReadSnapshot, PeerAgentReadError> {
        let command = build_ssh_agent_read_command(peer, remote_agent_name, options)
            .map_err(PeerAgentReadError::before_remote_command)?;
        let output = Command::new(&command.program)
            .args(&command.args)
            .output()
            .map_err(|error| PeerAgentReadError::after_remote_command(error.into()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let detail = if stderr.is_empty() {
                format!("ssh exited with status {}", output.status)
            } else {
                stderr
            };
            return Err(PeerAgentReadError::after_remote_command(
                CamError::PeerTransportFailed(format!(
                    "peer agent read command failed for `{}`: {detail}",
                    peer.name
                )),
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        parse_peer_agent_read_output(&stdout).map_err(PeerAgentReadError::after_remote_command)
    }
}

impl PeerAgentResumeFetcher for SshPeerAgentResumeFetcher {
    fn resume_agent(
        &self,
        peer: &Peer,
        remote_agent_name: &str,
    ) -> Result<AgentResumeResult, PeerAgentResumeError> {
        let command = build_ssh_agent_resume_command(peer, remote_agent_name)
            .map_err(PeerAgentResumeError::before_remote_command)?;
        let output = Command::new(&command.program)
            .args(&command.args)
            .output()
            .map_err(|error| PeerAgentResumeError::after_remote_command(error.into()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let detail = if stderr.is_empty() {
                format!("ssh exited with status {}", output.status)
            } else {
                stderr
            };
            return Err(PeerAgentResumeError::after_remote_command(
                CamError::PeerTransportFailed(format!(
                    "peer agent resume command failed for `{}`: {detail}",
                    peer.name
                )),
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        parse_peer_agent_resume_output(&stdout).map_err(PeerAgentResumeError::after_remote_command)
    }
}

impl PeerInventoryFetcher for SshInventoryFetcher {
    fn fetch_inventory(&self, peer: &Peer) -> Result<PeerInventoryFetch, PeerInventoryFetchError> {
        let command = build_ssh_inventory_command(peer)
            .map_err(PeerInventoryFetchError::before_remote_command)?;
        let output = Command::new(&command.program)
            .args(&command.args)
            .output()
            .map_err(|error| PeerInventoryFetchError::after_remote_command(error.into()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let detail = if stderr.is_empty() {
                format!("ssh exited with status {}", output.status)
            } else {
                stderr
            };
            return Err(PeerInventoryFetchError::after_remote_command(
                CamError::PeerTransportFailed(format!(
                    "peer inventory export command failed for `{}`: {detail}",
                    peer.name
                )),
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        if stdout.trim().is_empty() {
            return Err(PeerInventoryFetchError::after_remote_command(
                CamError::PeerProtocolViolation(format!(
                    "peer inventory export command returned empty stdout for `{}`",
                    peer.name
                )),
            ));
        }

        Ok(PeerInventoryFetch { command, stdout })
    }
}

pub fn build_ssh_agent_resume_command(
    peer: &Peer,
    remote_agent_name: &str,
) -> Result<PeerAgentResumeCommand, CamError> {
    if peer.transport != crate::core::PeerTransport::Ssh {
        return Err(CamError::InvalidState(format!(
            "peer `{}` uses transport `{:?}`, but peer agent resume only supports ssh",
            peer.name, peer.transport
        )));
    }
    validate_peer_delivery_token("remote_agent_name", remote_agent_name)?;

    let ssh_target = peer.ssh_target.as_deref().ok_or_else(|| {
        CamError::InvalidState(format!("peer `{}` is missing ssh_target", peer.name))
    })?;
    let remote_command =
        build_remote_agent_resume_command(peer.remote_root.as_deref(), remote_agent_name);
    let mut args = vec!["-o".to_string(), "BatchMode=yes".to_string()];
    if let Some(key_path) = &peer.key_path {
        args.push("-i".to_string());
        args.push(key_path.clone());
    }
    args.push(ssh_target.to_string());
    args.push(remote_command.clone());

    Ok(PeerAgentResumeCommand {
        program: "ssh".to_string(),
        args,
        remote_command,
    })
}

pub fn parse_peer_agent_resume_output(stdout: &str) -> Result<AgentResumeResult, CamError> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Err(CamError::PeerProtocolViolation(
            "peer agent resume stdout was empty".to_string(),
        ));
    }
    serde_json::from_str(trimmed).map_err(|error| {
        CamError::PeerProtocolViolation(format!("invalid remote CAM agent resume JSON: {error}"))
    })
}

pub fn build_ssh_agent_read_command(
    peer: &Peer,
    remote_agent_name: &str,
    options: AgentReadOptions,
) -> Result<PeerAgentReadCommand, CamError> {
    if peer.transport != crate::core::PeerTransport::Ssh {
        return Err(CamError::InvalidState(format!(
            "peer `{}` uses transport `{:?}`, but peer agent read only supports ssh",
            peer.name, peer.transport
        )));
    }
    validate_peer_delivery_token("remote_agent_name", remote_agent_name)?;

    let ssh_target = peer.ssh_target.as_deref().ok_or_else(|| {
        CamError::InvalidState(format!("peer `{}` is missing ssh_target", peer.name))
    })?;
    let remote_command =
        build_remote_agent_read_command(peer.remote_root.as_deref(), remote_agent_name, options);
    let mut args = vec!["-o".to_string(), "BatchMode=yes".to_string()];
    if let Some(key_path) = &peer.key_path {
        args.push("-i".to_string());
        args.push(key_path.clone());
    }
    args.push(ssh_target.to_string());
    args.push(remote_command.clone());

    Ok(PeerAgentReadCommand {
        program: "ssh".to_string(),
        args,
        remote_command,
    })
}

pub fn parse_peer_agent_read_output(stdout: &str) -> Result<AgentReadSnapshot, CamError> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Err(CamError::PeerProtocolViolation(
            "peer agent read stdout was empty".to_string(),
        ));
    }
    serde_json::from_str(trimmed).map_err(|error| {
        CamError::PeerProtocolViolation(format!("invalid remote CAM agent read JSON: {error}"))
    })
}

pub fn build_ssh_message_command(
    peer: &Peer,
    request: &PeerMessageSendRequest,
) -> Result<PeerMessageCommand, CamError> {
    if peer.transport != crate::core::PeerTransport::Ssh {
        return Err(CamError::InvalidState(format!(
            "peer `{}` uses transport `{:?}`, but peer message send only supports ssh",
            peer.name, peer.transport
        )));
    }

    let ssh_target = peer.ssh_target.as_deref().ok_or_else(|| {
        CamError::InvalidState(format!("peer `{}` is missing ssh_target", peer.name))
    })?;
    let remote_command = build_remote_message_command(peer.remote_root.as_deref(), request);
    let mut args = vec!["-o".to_string(), "BatchMode=yes".to_string()];
    if let Some(key_path) = &peer.key_path {
        args.push("-i".to_string());
        args.push(key_path.clone());
    }
    args.push(ssh_target.to_string());
    args.push(remote_command.clone());

    Ok(PeerMessageCommand {
        program: "ssh".to_string(),
        args,
        remote_command,
    })
}

pub fn parse_peer_message_send_output(stdout: &str) -> Result<PeerMessageSendOutcome, CamError> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Err(CamError::PeerProtocolViolation(
            "peer message send stdout was empty".to_string(),
        ));
    }
    let result: RemoteSendResult = serde_json::from_str(trimmed).map_err(|error| {
        CamError::PeerProtocolViolation(format!("invalid remote CAM send JSON: {error}"))
    })?;
    if !result.ok {
        return Err(CamError::DeliveryFailed(result.error.unwrap_or_else(
            || "remote CAM send returned ok=false".to_string(),
        )));
    }
    if result.error.is_some() {
        return Err(CamError::PeerProtocolViolation(
            "remote CAM send returned ok=true with error text".to_string(),
        ));
    }
    if !matches!(
        result.delivery,
        DeliveryState::Steered | DeliveryState::Delivered
    ) {
        return Err(CamError::PeerProtocolViolation(format!(
            "remote CAM send returned non-immediate delivery state `{:?}`",
            result.delivery
        )));
    }
    let message_id = result.message_id.as_deref().ok_or_else(|| {
        CamError::PeerProtocolViolation(format!(
            "remote CAM send returned `{:?}` without message_id proof",
            result.delivery
        ))
    })?;
    validate_peer_message_id(message_id)?;

    let target_agent = result.target_agent.as_deref().ok_or_else(|| {
        CamError::PeerProtocolViolation(format!(
            "remote CAM send returned `{:?}` without target_agent proof",
            result.delivery
        ))
    })?;
    validate_peer_delivery_token("target_agent", target_agent)?;

    let receipt_nonce = result.receipt_nonce.as_deref().ok_or_else(|| {
        CamError::PeerProtocolViolation(format!(
            "remote CAM send returned `{:?}` without receipt_nonce proof",
            result.delivery
        ))
    })?;
    validate_peer_message_id(receipt_nonce)?;

    let thread_id = result.thread_id.as_deref().ok_or_else(|| {
        CamError::PeerProtocolViolation(format!(
            "remote CAM send returned `{:?}` without thread_id proof",
            result.delivery
        ))
    })?;
    validate_peer_delivery_token("thread_id", thread_id)?;

    let turn_id = result.turn_id.as_deref().ok_or_else(|| {
        CamError::PeerProtocolViolation(format!(
            "remote CAM send returned `{:?}` without turn_id proof",
            result.delivery
        ))
    })?;
    validate_peer_delivery_token("turn_id", turn_id)?;

    Ok(PeerMessageSendOutcome {
        delivery: result.delivery,
        message_id: result.message_id,
        target_agent: result.target_agent,
        receipt_nonce: result.receipt_nonce,
        thread_id: result.thread_id,
        turn_id: result.turn_id,
        error: result.error,
    })
}

fn validate_peer_message_id(message_id: &str) -> Result<(), CamError> {
    let parsed = Uuid::parse_str(message_id).map_err(|_| {
        CamError::PeerProtocolViolation("remote CAM send message_id must be a UUID".to_string())
    })?;
    if parsed.to_string() != message_id {
        return Err(CamError::PeerProtocolViolation(
            "remote CAM send message_id must be canonical lowercase UUID".to_string(),
        ));
    }
    Ok(())
}

fn validate_peer_delivery_token(label: &str, value: &str) -> Result<(), CamError> {
    if value.trim().is_empty() {
        return Err(CamError::PeerProtocolViolation(format!(
            "remote CAM send {label} cannot be empty"
        )));
    }
    if value.trim() != value {
        return Err(CamError::PeerProtocolViolation(format!(
            "remote CAM send {label} must not have leading or trailing whitespace"
        )));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(CamError::PeerProtocolViolation(format!(
            "remote CAM send {label} cannot contain whitespace"
        )));
    }
    Ok(())
}

pub fn build_ssh_inventory_command(peer: &Peer) -> Result<PeerInventoryCommand, CamError> {
    if peer.transport != crate::core::PeerTransport::Ssh {
        return Err(CamError::InvalidState(format!(
            "peer `{}` uses transport `{:?}`, but peer sync only supports ssh",
            peer.name, peer.transport
        )));
    }

    let ssh_target = peer.ssh_target.as_deref().ok_or_else(|| {
        CamError::InvalidState(format!("peer `{}` is missing ssh_target", peer.name))
    })?;
    let remote_command = build_remote_inventory_command(peer.remote_root.as_deref());
    let mut args = vec!["-o".to_string(), "BatchMode=yes".to_string()];
    if let Some(key_path) = &peer.key_path {
        args.push("-i".to_string());
        args.push(key_path.clone());
    }
    args.push(ssh_target.to_string());
    args.push(remote_command.clone());

    Ok(PeerInventoryCommand {
        program: "ssh".to_string(),
        args,
        remote_command,
    })
}

pub fn parse_inventory_export(stdout: &str) -> Result<InventoryExport, CamError> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Err(CamError::InvalidState(
            "peer inventory export stdout was empty".to_string(),
        ));
    }
    Ok(serde_json::from_str(trimmed)?)
}

fn build_remote_message_command(
    remote_root: Option<&str>,
    request: &PeerMessageSendRequest,
) -> String {
    let executable = remote_cam_executable(remote_root);
    let mut send_parts = vec![
        executable.clone(),
        "send".to_string(),
        shell_single_quote(&request.remote_agent_name),
        shell_single_quote(&request.message.body),
    ];
    if let Some(source_agent) = &request.message.source_agent {
        send_parts.push("--from".to_string());
        send_parts.push(shell_single_quote(source_agent));
    }
    if let Some(source_node) = &request.message.source_node {
        send_parts.push("--source-node".to_string());
        send_parts.push(shell_single_quote(source_node));
    }
    if let Some(correlation_id) = &request.message.correlation_id {
        send_parts.push("--correlation-id".to_string());
        send_parts.push(shell_single_quote(correlation_id));
    }
    if let Some(message_type) = &request.message.message_type {
        send_parts.push("--message-type".to_string());
        send_parts.push(shell_single_quote(message_type));
    }
    if request.message.strict {
        send_parts.push("--strict".to_string());
    }
    send_parts.push("--receipt-nonce".to_string());
    send_parts.push(shell_single_quote(&request.message.message_id));
    let send_command = send_parts.join(" ");

    let send_command = match (&request.remote_thread_id, &request.remote_cwd) {
        (Some(thread_id), Some(cwd)) => {
            let ensure_command = [
                executable.clone(),
                "agent".to_string(),
                "status".to_string(),
                shell_single_quote(&request.remote_agent_name),
                ">/dev/null".to_string(),
                "2>&1".to_string(),
                "||".to_string(),
                executable.clone(),
                "agent".to_string(),
                "create".to_string(),
                shell_single_quote(&request.remote_agent_name),
                "--cwd".to_string(),
                shell_single_quote(cwd),
                "--thread-id".to_string(),
                shell_single_quote(thread_id),
                "--source".to_string(),
                "codex".to_string(),
                ">/dev/null".to_string(),
            ]
            .join(" ");
            format!("({ensure_command}) && {send_command}")
        }
        _ => send_command,
    };

    match remote_root {
        Some(root) => format!("cd {} && {send_command}", shell_single_quote(root)),
        None => send_command,
    }
}

fn build_remote_agent_resume_command(remote_root: Option<&str>, remote_agent_name: &str) -> String {
    let executable = remote_cam_executable(remote_root);
    let resume_command = [
        executable,
        "agent".to_string(),
        "resume".to_string(),
        shell_single_quote(remote_agent_name),
    ]
    .join(" ");

    match remote_root {
        Some(root) => format!("cd {} && {resume_command}", shell_single_quote(root)),
        None => resume_command,
    }
}

fn build_remote_agent_read_command(
    remote_root: Option<&str>,
    remote_agent_name: &str,
    options: AgentReadOptions,
) -> String {
    let executable = remote_cam_executable(remote_root);
    let mut parts = vec![
        executable,
        "agent".to_string(),
        "read".to_string(),
        shell_single_quote(remote_agent_name),
    ];
    if options.latest_only {
        parts.push("--latest".to_string());
    }
    if options.include_turns {
        parts.push("--include-turns".to_string());
    }
    if let Some(limit) = options.turn_limit {
        parts.push("--turns".to_string());
        parts.push(limit.to_string());
    }
    if let Some(wait_seconds) = options.wait_seconds {
        parts.push("--wait-seconds".to_string());
        parts.push(wait_seconds.to_string());
    }
    let read_command = parts.join(" ");

    match remote_root {
        Some(root) => format!("cd {} && {read_command}", shell_single_quote(root)),
        None => read_command,
    }
}

fn build_remote_inventory_command(remote_root: Option<&str>) -> String {
    let executable = remote_cam_executable(remote_root);
    match remote_root {
        Some(root) => format!(
            "cd {} && {executable} inventory export",
            shell_single_quote(root),
        ),
        None => format!("{executable} inventory export"),
    }
}

fn remote_cam_executable(remote_root: Option<&str>) -> String {
    if remote_root.is_some() {
        "CAM_HOME=./home ./qexow-cam".to_string()
    } else {
        "$(command -v qexow-cam 2>/dev/null || command -v cam 2>/dev/null || printf qexow-cam)"
            .to_string()
    }
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[derive(Debug, Deserialize)]
struct RemoteSendResult {
    ok: bool,
    delivery: DeliveryState,
    message_id: Option<String>,
    target_agent: Option<String>,
    receipt_nonce: Option<String>,
    thread_id: Option<String>,
    turn_id: Option<String>,
    error: Option<String>,
}

pub fn build_inventory(
    config: &Config,
    agents: &[Agent],
    peers: &[Peer],
    discovery_rows: &[DiscoveryRow],
) -> InventoryExport {
    let exported_agents = inventory_agents_with_discovered_chats(agents, discovery_rows);
    InventoryExport {
        inventory_version: 1,
        node_name: config.node_name.clone(),
        exported_at: now_utc(),
        agents: exported_agents,
        peers: peers.to_vec(),
        discovery: summarize_discovery(discovery_rows),
        notes: vec![
            "inventory export is the stable peer mirror contract".to_string(),
            "remote consumers must preserve route metadata and must not treat remote mirrors as local agents".to_string(),
            "inventory includes explicit agents plus approved local Codex discovery rows synthesized for peer mirroring".to_string(),
        ],
    }
}

fn inventory_agents_with_discovered_chats(
    agents: &[Agent],
    discovery_rows: &[DiscoveryRow],
) -> Vec<Agent> {
    let mut exported = agents.to_vec();
    let mut known_thread_ids = agents
        .iter()
        .filter_map(|agent| agent.thread_id.as_deref())
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>();
    let mut known_names = agents
        .iter()
        .map(|agent| agent.name.clone())
        .collect::<BTreeSet<_>>();

    for row in discovery_rows {
        if !is_mirrorable_discovery_row(row) {
            continue;
        }
        let Some(thread_id) = row.thread_id.as_deref() else {
            continue;
        };
        if !known_thread_ids.insert(thread_id.to_string()) {
            continue;
        }
        let Some(cwd) = row.cwd.clone() else {
            continue;
        };

        let base_name = discovery_inventory_agent_name(&row.title);
        let name = unique_inventory_agent_name(&base_name, thread_id, &mut known_names);
        exported.push(Agent {
            name,
            kind: AgentKind::Codex,
            thread_id: Some(thread_id.to_string()),
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
            created_at: row.updated_at.clone(),
            updated_at: row.updated_at.clone(),
            last_error: Some(format!(
                "inventory-only agent synthesized from remote discovery row; original disposition `{}`: {}",
                format_discovery_disposition(&row.disposition),
                row.reason
            )),
        });
    }

    exported
}

fn is_mirrorable_discovery_row(row: &DiscoveryRow) -> bool {
    matches!(row.route, Route::Local)
        && row.thread_source == ThreadSource::Codex
        && row
            .thread_id
            .as_deref()
            .is_some_and(is_canonical_inventory_token)
        && row
            .cwd
            .as_deref()
            .is_some_and(|cwd| !cwd.is_empty() && cwd.trim() == cwd)
        && row.chat_status != ChatStatus::Unknown
        && row.chat_status_source == ChatStatusSource::ThreadDatabase
}

fn format_discovery_disposition(disposition: &DiscoveryDisposition) -> &'static str {
    match disposition {
        DiscoveryDisposition::Approved => "approved",
        DiscoveryDisposition::Candidate => "candidate",
        DiscoveryDisposition::Quarantined => "quarantined",
        DiscoveryDisposition::Rejected => "rejected",
    }
}

fn unique_inventory_agent_name(
    base_name: &str,
    thread_id: &str,
    known_names: &mut BTreeSet<String>,
) -> String {
    if known_names.insert(base_name.to_string()) {
        return base_name.to_string();
    }
    let suffix = thread_id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .take(8)
        .collect::<String>();
    let candidate = format!("{base_name}-{suffix}");
    if known_names.insert(candidate.clone()) {
        return candidate;
    }
    let mut counter = 2usize;
    loop {
        let candidate = format!("{base_name}-{suffix}-{counter}");
        if known_names.insert(candidate.clone()) {
            return candidate;
        }
        counter += 1;
    }
}

fn discovery_inventory_agent_name(title: &str) -> String {
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

pub fn export_inventory(inventory: &InventoryExport) -> Result<String, CamError> {
    Ok(serde_json::to_string_pretty(inventory)?)
}

pub fn apply_trusted_peer_inventory(
    peer: &mut Peer,
    inventory: &InventoryExport,
    local_agents: &mut Vec<Agent>,
) -> Result<RemoteMirrorApplyResult, CamError> {
    match apply_remote_inventory(&peer.name, inventory, local_agents) {
        Ok(result) => {
            let now = now_utc();
            peer.remote_node_name = Some(inventory.node_name.clone());
            peer.last_sync_at = Some(now.clone());
            peer.last_sync_error = None;
            peer.last_sync_error_kind = None;
            peer.inventory_source = Some("inventory_export".to_string());
            peer.updated_at = now;
            peer.state = if result.collision_count == 0 && result.mirrored_agents_stale == 0 {
                PeerState::Mirrored
            } else {
                PeerState::MirroredDegraded
            };
            Ok(result)
        }
        Err(error) => {
            let now = now_utc();
            peer.state = PeerState::SyncFailed;
            peer.last_sync_at = Some(now.clone());
            peer.last_sync_error = Some(error.to_string());
            peer.last_sync_error_kind = Some(error.kind().to_string());
            peer.inventory_source = Some("inventory_export".to_string());
            peer.updated_at = now;
            Err(error)
        }
    }
}

pub fn apply_remote_inventory(
    peer_name: &str,
    inventory: &InventoryExport,
    local_agents: &mut Vec<Agent>,
) -> Result<RemoteMirrorApplyResult, CamError> {
    validate_remote_inventory_contract(peer_name, inventory)?;

    let mut result = RemoteMirrorApplyResult {
        peer_name: peer_name.to_string(),
        remote_agents_seen: inventory.agents.len(),
        mirrored_agents_added: 0,
        mirrored_agents_updated: 0,
        mirrored_agents_skipped: 0,
        mirrored_agents_stale: 0,
        collision_count: 0,
    };
    let mut current_mirror_names = BTreeSet::new();

    for remote_agent in &inventory.agents {
        if should_skip_remote_inventory_agent(remote_agent) {
            result.mirrored_agents_skipped += 1;
            continue;
        }

        let mirror_name = format!("{peer_name}::{}", remote_agent.name);
        current_mirror_names.insert(mirror_name.clone());
        let mirror = remote_agent_to_mirror(peer_name, &mirror_name, remote_agent);

        match local_agents
            .iter_mut()
            .find(|agent| agent.name == mirror_name)
        {
            Some(existing) if is_same_peer_mirror(existing, peer_name) => {
                *existing = merge_peer_mirror_update(existing, &mirror);
                result.mirrored_agents_updated += 1;
            }
            Some(_) => {
                result.collision_count += 1;
                result.mirrored_agents_skipped += 1;
            }
            None => {
                local_agents.push(mirror);
                result.mirrored_agents_added += 1;
            }
        }
    }

    let now = now_utc();
    for agent in local_agents.iter_mut() {
        if is_same_peer_mirror(agent, peer_name) && !current_mirror_names.contains(&agent.name) {
            agent.status = AgentStatus::Unknown;
            agent.last_error = Some(format!(
                "remote mirror was not present in latest inventory export from peer `{peer_name}`"
            ));
            agent.updated_at = now.clone();
            result.mirrored_agents_stale += 1;
        }
    }

    Ok(result)
}

fn validate_remote_inventory_contract(
    peer_name: &str,
    inventory: &InventoryExport,
) -> Result<(), CamError> {
    if inventory.inventory_version != 1 {
        return Err(CamError::InvalidState(format!(
            "unsupported inventory version {}; expected 1",
            inventory.inventory_version
        )));
    }

    let mut thread_ids = BTreeSet::new();
    for remote_agent in &inventory.agents {
        validate_remote_inventory_agent_basics(peer_name, remote_agent)?;
        if should_skip_remote_inventory_agent(remote_agent) {
            continue;
        }
        validate_mirrorable_remote_agent(peer_name, remote_agent)?;
        if let Some(thread_id) = remote_agent.thread_id.as_deref()
            && !thread_ids.insert(thread_id.to_string())
        {
            return Err(CamError::InvalidState(format!(
                "remote inventory from peer `{peer_name}` has duplicate thread/session id `{thread_id}`"
            )));
        }
    }
    Ok(())
}

fn should_skip_remote_inventory_agent(remote_agent: &Agent) -> bool {
    matches!(
        remote_agent.kind,
        AgentKind::RemoteMirror | AgentKind::VirtualInbox
    )
}

fn validate_remote_inventory_agent_basics(
    peer_name: &str,
    remote_agent: &Agent,
) -> Result<(), CamError> {
    if !is_canonical_inventory_token(&remote_agent.name) {
        return Err(CamError::InvalidState(format!(
            "remote inventory from peer `{peer_name}` contains non-canonical agent name `{}`",
            remote_agent.name
        )));
    }
    if remote_agent.status == AgentStatus::Active && remote_agent.active_turn_id.is_none() {
        return Err(CamError::InvalidState(format!(
            "remote inventory from peer `{peer_name}` agent `{}` is active without active turn proof",
            remote_agent.name
        )));
    }
    if remote_agent.active_turn_id.is_some() && remote_agent.thread_id.is_none() {
        return Err(CamError::InvalidState(format!(
            "remote inventory from peer `{peer_name}` agent `{}` has active turn proof without thread/session identity",
            remote_agent.name
        )));
    }
    if !is_optional_canonical_inventory_token(remote_agent.thread_id.as_deref()) {
        return Err(CamError::InvalidState(format!(
            "remote inventory from peer `{peer_name}` agent `{}` has non-canonical thread/session id",
            remote_agent.name
        )));
    }
    if !is_optional_canonical_inventory_token(remote_agent.active_turn_id.as_deref()) {
        return Err(CamError::InvalidState(format!(
            "remote inventory from peer `{peer_name}` agent `{}` has non-canonical active turn id",
            remote_agent.name
        )));
    }
    if !is_optional_canonical_inventory_token(remote_agent.last_turn_id.as_deref()) {
        return Err(CamError::InvalidState(format!(
            "remote inventory from peer `{peer_name}` agent `{}` has non-canonical last turn id",
            remote_agent.name
        )));
    }

    Ok(())
}

fn validate_mirrorable_remote_agent(peer_name: &str, remote_agent: &Agent) -> Result<(), CamError> {
    if !matches!(remote_agent.route, Route::Local) {
        return Err(CamError::InvalidState(format!(
            "remote inventory from peer `{peer_name}` contains mirrorable agent `{}` with non-local route",
            remote_agent.name
        )));
    }

    if let Some(thread_id) = remote_agent.thread_id.as_deref()
        && !is_canonical_inventory_token(thread_id)
    {
        return Err(CamError::InvalidState(format!(
            "remote inventory from peer `{peer_name}` mirrorable agent `{}` has non-canonical thread/session id",
            remote_agent.name
        )));
    }

    Ok(())
}

fn is_optional_canonical_inventory_token(value: Option<&str>) -> bool {
    value.map_or(true, is_canonical_inventory_token)
}

fn is_canonical_inventory_token(value: &str) -> bool {
    !value.is_empty() && value.trim() == value && !value.chars().any(char::is_whitespace)
}

fn remote_agent_to_mirror(peer_name: &str, mirror_name: &str, remote_agent: &Agent) -> Agent {
    let now = now_utc();
    Agent {
        name: mirror_name.to_string(),
        kind: AgentKind::RemoteMirror,
        thread_id: remote_agent.thread_id.clone(),
        thread_source: ThreadSource::RemoteMirror,
        cwd: remote_agent.cwd.clone(),
        route: Route::Peer {
            peer_name: peer_name.to_string(),
        },
        status: remote_agent.status.clone(),
        chat_status: remote_agent.chat_status.clone(),
        chat_status_source: remote_agent.chat_status_source.clone(),
        active_turn_id: remote_agent.active_turn_id.clone(),
        last_turn_id: remote_agent.last_turn_id.clone(),
        model: remote_agent.model.clone(),
        model_provider: remote_agent.model_provider.clone(),
        effort: remote_agent.effort.clone(),
        service_tier: remote_agent.service_tier.clone(),
        created_at: now.clone(),
        updated_at: now,
        last_error: remote_agent.last_error.clone(),
    }
}

fn merge_peer_mirror_update(existing: &Agent, incoming: &Agent) -> Agent {
    let mut merged = incoming.clone();
    merged.created_at = existing.created_at.clone();

    if merged.chat_status == ChatStatus::Unknown {
        merged.chat_status_source = ChatStatusSource::Unknown;
    }
    if merged.thread_id.is_none() {
        merged.thread_id = existing.thread_id.clone();
    }
    if merged.cwd.is_none() {
        merged.cwd = existing.cwd.clone();
    }
    if merged.active_turn_id.is_none() {
        merged.active_turn_id = existing.active_turn_id.clone();
    }
    if merged.last_turn_id.is_none() {
        merged.last_turn_id = existing.last_turn_id.clone();
    }
    if merged.model.is_none() {
        merged.model = existing.model.clone();
    }
    if merged.model_provider.is_none() {
        merged.model_provider = existing.model_provider.clone();
    }
    if merged.effort.is_none() {
        merged.effort = existing.effort.clone();
    }
    if merged.service_tier.is_none() {
        merged.service_tier = existing.service_tier.clone();
    }
    if merged.last_error.is_none() {
        merged.last_error = existing.last_error.clone();
    }

    merged
}

fn is_same_peer_mirror(agent: &Agent, peer_name: &str) -> bool {
    agent.kind == AgentKind::RemoteMirror
        && matches!(
            &agent.route,
            Route::Peer { peer_name: existing } if existing == peer_name
        )
}

fn summarize_discovery(rows: &[DiscoveryRow]) -> InventoryDiscoverySummary {
    InventoryDiscoverySummary {
        rows: rows.len(),
        approved: count(rows, DiscoveryDisposition::Approved),
        candidate: count(rows, DiscoveryDisposition::Candidate),
        quarantined: count(rows, DiscoveryDisposition::Quarantined),
        rejected: count(rows, DiscoveryDisposition::Rejected),
    }
}

fn count(rows: &[DiscoveryRow], disposition: DiscoveryDisposition) -> usize {
    rows.iter()
        .filter(|row| row.disposition == disposition)
        .count()
}
