use crate::errors::CamError;
use crate::providers::{
    ProviderDeliveryReceipt, ProviderReadinessOutcome, ProviderReadinessRequest,
    ProviderResumeOutcome, ProviderResumeRequest, ProviderSendOutcome, ProviderSendRequest,
    validate_provider_readiness_outcome, validate_provider_resume_outcome,
};
use crate::{core::DeliveryState, delivery::DeliveryAction};
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgyCommandOperation {
    SteerActiveChat,
    WakeKnownSession,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AgyCommandEnvelope {
    pub operation: AgyCommandOperation,
    pub message_id: String,
    pub target_agent: String,
    pub session_id: String,
    pub active_turn_id: Option<String>,
    pub source_agent: Option<String>,
    pub source_node: Option<String>,
    pub correlation_id: Option<String>,
    pub message_type: Option<String>,
    pub body: String,
    pub strict: bool,
}

pub trait AgySendExecutor {
    fn steer_active_chat(
        &self,
        command: &AgyCommandEnvelope,
    ) -> Result<ProviderDeliveryReceipt, CamError>;
    fn wake_known_session(
        &self,
        command: &AgyCommandEnvelope,
    ) -> Result<ProviderDeliveryReceipt, CamError>;
}

pub trait AgyLifecycleExecutor {
    fn check_readiness(
        &self,
        request: &ProviderReadinessRequest,
    ) -> Result<ProviderReadinessOutcome, CamError>;

    fn resume(&self, request: &ProviderResumeRequest) -> Result<ProviderResumeOutcome, CamError>;
}

pub fn send(request: ProviderSendRequest) -> Result<ProviderSendOutcome, CamError> {
    send_with_executor(request, &AgyAgentApiExecutor::discover()?)
}

pub fn send_with_executor(
    request: ProviderSendRequest,
    executor: &impl AgySendExecutor,
) -> Result<ProviderSendOutcome, CamError> {
    match request.delivery_action {
        DeliveryAction::SteerActiveTurn => steer_active_chat(request, executor),
        DeliveryAction::WakeKnownSession => wake_known_session(request, executor),
        DeliveryAction::StoreInVirtualInbox
        | DeliveryAction::QueueFallback
        | DeliveryAction::FailLoudly => Err(CamError::DeliveryFailed(format!(
            "AGY provider cannot handle delivery action `{:?}`",
            request.delivery_action
        ))),
    }
}

pub fn check_readiness_with_executor(
    request: ProviderReadinessRequest,
    executor: &impl AgyLifecycleExecutor,
) -> Result<ProviderReadinessOutcome, CamError> {
    let outcome = executor.check_readiness(&request)?;
    validate_provider_readiness_outcome(&request, &outcome)?;
    Ok(outcome)
}

pub fn resume_with_executor(
    request: ProviderResumeRequest,
    executor: &impl AgyLifecycleExecutor,
) -> Result<ProviderResumeOutcome, CamError> {
    let outcome = executor.resume(&request)?;
    validate_provider_resume_outcome(&request, &outcome)?;
    Ok(outcome)
}

impl AgyCommandEnvelope {
    pub fn for_steer(request: &ProviderSendRequest) -> Result<Self, CamError> {
        if request.delivery_action != DeliveryAction::SteerActiveTurn {
            return Err(CamError::DeliveryFailed(format!(
                "AGY steer envelope requires SteerActiveTurn action, got `{:?}`",
                request.delivery_action
            )));
        }
        if request.expected_delivery != DeliveryState::Steered {
            return Err(CamError::DeliveryFailed(format!(
                "AGY steer envelope requires steered delivery, got `{:?}`",
                request.expected_delivery
            )));
        }
        let session_id = request.thread_id.clone().ok_or_else(|| {
            CamError::DeliveryFailed("AGY steer envelope requires session id".to_string())
        })?;
        validate_nonempty_token("AGY session id", &session_id)?;
        let active_turn_id = request.active_turn_id.clone().ok_or_else(|| {
            CamError::DeliveryFailed("AGY steer envelope requires active turn id".to_string())
        })?;
        validate_nonempty_token("AGY active turn id", &active_turn_id)?;
        Ok(Self::from_request(
            request,
            AgyCommandOperation::SteerActiveChat,
            session_id,
            Some(active_turn_id),
        ))
    }

    pub fn for_wake(request: &ProviderSendRequest) -> Result<Self, CamError> {
        if request.delivery_action != DeliveryAction::WakeKnownSession {
            return Err(CamError::DeliveryFailed(format!(
                "AGY wake envelope requires WakeKnownSession action, got `{:?}`",
                request.delivery_action
            )));
        }
        if request.expected_delivery != DeliveryState::Delivered {
            return Err(CamError::DeliveryFailed(format!(
                "AGY wake envelope requires delivered delivery, got `{:?}`",
                request.expected_delivery
            )));
        }
        let session_id = request.thread_id.clone().ok_or_else(|| {
            CamError::DeliveryFailed("AGY wake envelope requires session id".to_string())
        })?;
        validate_nonempty_token("AGY session id", &session_id)?;
        if request.active_turn_id.is_some() {
            return Err(CamError::DeliveryFailed(
                "AGY wake envelope must not include active turn id".to_string(),
            ));
        }
        Ok(Self::from_request(
            request,
            AgyCommandOperation::WakeKnownSession,
            session_id,
            None,
        ))
    }

    fn from_request(
        request: &ProviderSendRequest,
        operation: AgyCommandOperation,
        session_id: String,
        active_turn_id: Option<String>,
    ) -> Self {
        Self {
            operation,
            message_id: request.message_id.clone(),
            target_agent: request.target_agent.clone(),
            session_id,
            active_turn_id,
            source_agent: request.source_agent.clone(),
            source_node: request.source_node.clone(),
            correlation_id: request.correlation_id.clone(),
            message_type: request.message_type.clone(),
            body: request.body.clone(),
            strict: request.strict,
        }
    }
}

fn steer_active_chat(
    request: ProviderSendRequest,
    executor: &impl AgySendExecutor,
) -> Result<ProviderSendOutcome, CamError> {
    if request.expected_delivery != DeliveryState::Steered {
        return Err(CamError::DeliveryFailed(format!(
            "AGY steer expected delivery `steered`, got `{:?}`",
            request.expected_delivery
        )));
    }
    if request.thread_id.is_none() {
        return Err(CamError::DeliveryFailed(
            "AGY steer requires a known session id".to_string(),
        ));
    }
    if request.active_turn_id.is_none() {
        return Err(CamError::DeliveryFailed(
            "AGY steer requires an active turn id".to_string(),
        ));
    }

    let command = AgyCommandEnvelope::for_steer(&request)?;
    let receipt = executor.steer_active_chat(&command)?;
    Ok(ProviderSendOutcome {
        delivery: DeliveryState::Steered,
        message_id: Some(receipt.message_id),
        target_agent: Some(receipt.target_agent),
        thread_id: Some(receipt.thread_id),
        turn_id: Some(receipt.turn_id),
    })
}

fn wake_known_session(
    request: ProviderSendRequest,
    executor: &impl AgySendExecutor,
) -> Result<ProviderSendOutcome, CamError> {
    if request.expected_delivery != DeliveryState::Delivered {
        return Err(CamError::DeliveryFailed(format!(
            "AGY wake expected delivery `delivered`, got `{:?}`",
            request.expected_delivery
        )));
    }
    if request.thread_id.is_none() {
        return Err(CamError::DeliveryFailed(
            "AGY wake/deliver requires a known session id".to_string(),
        ));
    }

    let command = AgyCommandEnvelope::for_wake(&request)?;
    let receipt = executor.wake_known_session(&command)?;
    Ok(ProviderSendOutcome {
        delivery: DeliveryState::Delivered,
        message_id: Some(receipt.message_id),
        target_agent: Some(receipt.target_agent),
        thread_id: Some(receipt.thread_id),
        turn_id: Some(receipt.turn_id),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgyAgentApiExecutor {
    program: PathBuf,
    ls_address: String,
    csrf_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AgyAgentApiResponse {
    response: Option<Value>,
    error: Option<String>,
}

impl AgyAgentApiExecutor {
    pub fn discover() -> Result<Self, CamError> {
        Ok(Self {
            program: discover_agentapi_program()?,
            ls_address: discover_agentapi_address()?,
            csrf_token: discover_csrf_token(),
        })
    }

    fn send_message(
        &self,
        command: &AgyCommandEnvelope,
    ) -> Result<ProviderDeliveryReceipt, CamError> {
        let output = Command::new(&self.program)
            .arg("send-message")
            .arg(&command.session_id)
            .arg(&command.body)
            .env("ANTIGRAVITY_LS_ADDRESS", &self.ls_address)
            .envs(
                self.csrf_token
                    .as_ref()
                    .map(|token| [("ANTIGRAVITY_CSRF_TOKEN", token.as_str())])
                    .into_iter()
                    .flatten(),
            )
            .output()
            .map_err(|error| {
                CamError::DeliveryFailed(format!(
                    "failed to execute Antigravity agentapi command `{}`: {error}",
                    self.program.display()
                ))
            })?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if !output.status.success() {
            let detail = if stdout.trim().is_empty() {
                stderr.trim().to_string()
            } else {
                stdout.trim().to_string()
            };
            return Err(CamError::DeliveryFailed(format!(
                "Antigravity agentapi send-message failed: {detail}"
            )));
        }
        let proof = parse_agentapi_send_proof(command, &stdout)?;
        Ok(ProviderDeliveryReceipt {
            message_id: command.message_id.clone(),
            target_agent: command.target_agent.clone(),
            thread_id: command.session_id.clone(),
            turn_id: proof,
        })
    }
}

impl AgySendExecutor for AgyAgentApiExecutor {
    fn steer_active_chat(
        &self,
        command: &AgyCommandEnvelope,
    ) -> Result<ProviderDeliveryReceipt, CamError> {
        self.send_message(command)
    }

    fn wake_known_session(
        &self,
        command: &AgyCommandEnvelope,
    ) -> Result<ProviderDeliveryReceipt, CamError> {
        self.send_message(command)
    }
}

fn parse_agentapi_send_proof(
    command: &AgyCommandEnvelope,
    stdout: &str,
) -> Result<String, CamError> {
    let parsed = serde_json::from_str::<AgyAgentApiResponse>(stdout.trim()).map_err(|error| {
        CamError::ProviderContractViolation(format!(
            "invalid Antigravity agentapi JSON response: {error}"
        ))
    })?;
    if let Some(error) = parsed.error {
        if !error.trim().is_empty() {
            return Err(CamError::DeliveryFailed(format!(
                "Antigravity agentapi send-message returned error: {error}"
            )));
        }
    }
    let response = parsed.response.ok_or_else(|| {
        CamError::ProviderContractViolation(
            "Antigravity agentapi send-message response missing response object".to_string(),
        )
    })?;
    if response.is_object() {
        let candidates = [
            "messageId",
            "message_id",
            "turnId",
            "turn_id",
            "cascadeId",
            "cascade_id",
            "conversationId",
            "conversation_id",
        ];
        for field in candidates {
            if let Some(value) = response.get(field).and_then(Value::as_str) {
                validate_nonempty_token(&format!("Antigravity agentapi {field}"), value)?;
                return Ok(value.to_string());
            }
        }
    }
    Ok(format!("agy-agentapi-{}", command.message_id))
}

fn discover_agentapi_program() -> Result<PathBuf, CamError> {
    if let Ok(program) = std::env::var("QEXOW_CAM_AGY_AGENTAPI") {
        let trimmed = program.trim();
        if !trimmed.is_empty() {
            let path = PathBuf::from(trimmed);
            if path.is_file() {
                return Ok(path);
            }
            return Err(CamError::ProviderUnavailable(format!(
                "configured Antigravity agentapi command `{trimmed}` does not exist"
            )));
        }
    }
    let home = home_dir();
    let candidate = home
        .join(".gemini")
        .join("antigravity")
        .join("bin")
        .join(if cfg!(windows) {
            "agentapi.bat"
        } else {
            "agentapi"
        });
    if candidate.is_file() {
        return Ok(candidate);
    }
    Err(CamError::ProviderUnavailable(format!(
        "Antigravity agentapi command not found at `{}`; set QEXOW_CAM_AGY_AGENTAPI",
        candidate.display()
    )))
}

fn discover_agentapi_address() -> Result<String, CamError> {
    if let Ok(address) = std::env::var("ANTIGRAVITY_LS_ADDRESS") {
        let trimmed = normalize_agentapi_address(&address);
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }
    if let Ok(address) = std::env::var("QEXOW_CAM_AGY_LS_ADDRESS") {
        let trimmed = normalize_agentapi_address(&address);
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }
    discover_agentapi_address_from_log().ok_or_else(|| {
        CamError::ProviderUnavailable(
            "Antigravity language-server address not found; start Antigravity or set ANTIGRAVITY_LS_ADDRESS to the HTTP port such as 127.0.0.1:52445".to_string(),
        )
    })
}

fn discover_agentapi_address_from_log() -> Option<String> {
    let log_path = home_dir()
        .join("AppData")
        .join("Roaming")
        .join("Antigravity")
        .join("logs")
        .join("language_server.log");
    let log = fs::read_to_string(log_path).ok()?;
    log.lines().rev().find_map(|line| {
        let marker = "Language server listening on random port at ";
        let rest = line.split(marker).nth(1)?;
        let (port, kind) = rest.split_once(" for ")?;
        if !kind.contains("HTTP") {
            return None;
        }
        Some(format!("127.0.0.1:{}", port.trim()))
    })
}

fn discover_csrf_token() -> Option<String> {
    std::env::var("ANTIGRAVITY_CSRF_TOKEN")
        .ok()
        .or_else(|| std::env::var("QEXOW_CAM_AGY_CSRF_TOKEN").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(discover_csrf_token_from_main_log)
}

fn discover_csrf_token_from_main_log() -> Option<String> {
    let log_path = home_dir()
        .join("AppData")
        .join("Roaming")
        .join("Antigravity")
        .join("logs")
        .join("main.log");
    let log = fs::read_to_string(log_path).ok()?;
    log.lines().rev().find_map(|line| {
        let (_, rest) = line.split_once("--csrf_token ")?;
        rest.split_whitespace().next().map(str::to_string)
    })
}

fn normalize_agentapi_address(address: &str) -> String {
    address
        .trim()
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/')
        .to_string()
}

fn home_dir() -> PathBuf {
    if let Ok(profile) = std::env::var("USERPROFILE") {
        if !profile.trim().is_empty() {
            return PathBuf::from(profile);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home);
        }
    }
    PathBuf::from(".")
}

fn validate_nonempty_token(label: &str, value: &str) -> Result<(), CamError> {
    if value.trim().is_empty() {
        return Err(CamError::DeliveryFailed(format!("{label} cannot be empty")));
    }
    if value.trim() != value {
        return Err(CamError::DeliveryFailed(format!(
            "{label} must not have leading or trailing whitespace"
        )));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(CamError::DeliveryFailed(format!(
            "{label} cannot contain whitespace"
        )));
    }
    Ok(())
}
