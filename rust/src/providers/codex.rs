use crate::errors::CamError;
use crate::providers::{
    ProviderDeliveryReceipt, ProviderReadinessOutcome, ProviderReadinessRequest,
    ProviderResumeOutcome, ProviderResumeRequest, ProviderSendOutcome, ProviderSendRequest,
    validate_provider_readiness_outcome, validate_provider_resume_outcome,
    validate_provider_send_outcome,
};
use crate::{core::DeliveryState, delivery::DeliveryAction};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const CODEX_APP_SERVER_TIMEOUT: Duration = Duration::from_secs(60);
const CODEX_TURN_COMPLETION_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CodexCommandOperation {
    SteerActiveTurn,
    WakeKnownSession,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CodexCommandEnvelope {
    pub operation: CodexCommandOperation,
    pub message_id: String,
    pub target_agent: String,
    pub thread_id: String,
    pub active_turn_id: Option<String>,
    pub cwd: Option<String>,
    pub source_agent: Option<String>,
    pub source_node: Option<String>,
    pub correlation_id: Option<String>,
    pub message_type: Option<String>,
    pub body: String,
    pub strict: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CodexStdioRequest {
    pub method: String,
    pub params: CodexStdioParams,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodexStdioParams {
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_turn_id: Option<String>,
    pub input: Vec<CodexTextInput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CodexTextInput {
    pub r#type: String,
    pub text: String,
    pub text_elements: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexStdioResponse {
    pub message_id: String,
    pub target_agent: String,
    pub thread_id: String,
    pub turn_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodexAppServerProbeOutput {
    pub ok: bool,
    pub program: String,
    pub command: Vec<String>,
    pub initialize_attempted: bool,
    pub initialized: bool,
    pub server_name: Option<String>,
    pub server_version: Option<String>,
    pub user_agent: Option<String>,
    pub codex_home: Option<String>,
    pub platform_family: Option<String>,
    pub platform_os: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CodexLifecycleStdioRequest {
    pub method: String,
    pub params: CodexLifecycleStdioParams,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodexLifecycleStdioParams {
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude_turns: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persist_extended_history: Option<bool>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CodexThreadStartStdioRequest {
    pub method: String,
    pub params: CodexThreadStartStdioParams,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodexThreadStartStdioParams {
    pub cwd: String,
    pub approval_policy: String,
    pub sandbox: String,
    pub ephemeral: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_source: Option<String>,
    pub experimental_raw_events: bool,
    pub persist_extended_history: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexThreadStartOutcome {
    pub target_agent: String,
    pub thread_id: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexThreadStartThenSendOutcome {
    pub thread_start: CodexThreadStartOutcome,
    pub send: ProviderSendOutcome,
}

impl CodexStdioRequest {
    pub fn new(command: CodexCommandEnvelope) -> Self {
        let method = match command.operation {
            CodexCommandOperation::SteerActiveTurn => "turn/steer",
            CodexCommandOperation::WakeKnownSession => "turn/start",
        }
        .to_string();
        let expected_turn_id = command.active_turn_id.clone();
        Self {
            method,
            params: CodexStdioParams {
                thread_id: command.thread_id,
                expected_turn_id,
                input: vec![CodexTextInput {
                    r#type: "text".to_string(),
                    text: command.body,
                    text_elements: Vec::new(),
                }],
                cwd: match command.operation {
                    CodexCommandOperation::WakeKnownSession => command.cwd,
                    CodexCommandOperation::SteerActiveTurn => None,
                },
                approval_policy: match command.operation {
                    CodexCommandOperation::WakeKnownSession => Some("never".to_string()),
                    CodexCommandOperation::SteerActiveTurn => None,
                },
                sandbox: match command.operation {
                    CodexCommandOperation::WakeKnownSession => {
                        Some("danger-full-access".to_string())
                    }
                    CodexCommandOperation::SteerActiveTurn => None,
                },
            },
        }
    }

    pub fn to_json_line(&self) -> Result<String, CamError> {
        app_server_request_lines(&self.method, &self.params)
    }
}

impl CodexLifecycleStdioRequest {
    pub fn for_readiness(request: &ProviderReadinessRequest) -> Self {
        Self {
            method: "thread/resume".to_string(),
            params: CodexLifecycleStdioParams {
                thread_id: request.thread_id.clone(),
                cwd: request.cwd.clone(),
                exclude_turns: Some(true),
                persist_extended_history: Some(false),
            },
        }
    }

    pub fn for_resume(request: &ProviderResumeRequest) -> Self {
        Self {
            method: "thread/resume".to_string(),
            params: CodexLifecycleStdioParams {
                thread_id: request.thread_id.clone(),
                cwd: request.cwd.clone(),
                exclude_turns: Some(true),
                persist_extended_history: Some(false),
            },
        }
    }

    pub fn to_json_line(&self) -> Result<String, CamError> {
        let mut line = serde_json::to_string(self)?;
        line.push('\n');
        Ok(line)
    }
}

impl CodexThreadStartStdioRequest {
    pub fn from_agent(agent: &crate::core::Agent) -> Result<Self, CamError> {
        let cwd = agent.cwd.clone().ok_or_else(|| {
            CamError::DeliveryFailed("Codex thread/start requires agent cwd".to_string())
        })?;
        validate_nonempty_trimmed("Codex thread/start cwd", &cwd)?;
        Ok(Self {
            method: "thread/start".to_string(),
            params: CodexThreadStartStdioParams {
                cwd,
                approval_policy: "never".to_string(),
                sandbox: "danger-full-access".to_string(),
                ephemeral: false,
                model: agent.model.clone(),
                model_provider: agent.model_provider.clone(),
                thread_source: None,
                experimental_raw_events: false,
                persist_extended_history: false,
            },
        })
    }
}

impl CodexStdioResponse {
    pub fn parse_for_command(command: &CodexCommandEnvelope, line: &str) -> Result<Self, CamError> {
        let value = parse_codex_stdio_value(line)?;

        let turn_id = match command.operation {
            CodexCommandOperation::SteerActiveTurn => {
                value.get("turnId").and_then(Value::as_str).ok_or_else(|| {
                    CamError::ProviderContractViolation(
                        "Codex stdio steer response missing turnId proof".to_string(),
                    )
                })?
            }
            CodexCommandOperation::WakeKnownSession => value
                .get("turn")
                .and_then(|turn| turn.get("id"))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    CamError::ProviderContractViolation(
                        "Codex stdio wake response missing turn.id proof".to_string(),
                    )
                })?,
        };
        validate_codex_stdio_proof_token("Codex stdio turn id", turn_id)?;
        Ok(Self {
            message_id: command.message_id.clone(),
            target_agent: command.target_agent.clone(),
            thread_id: command.thread_id.clone(),
            turn_id: turn_id.to_string(),
        })
    }
}

fn parse_codex_stdio_value(line: &str) -> Result<Value, CamError> {
    let value = serde_json::from_str::<Value>(line.trim()).map_err(|error| {
        CamError::ProviderContractViolation(format!("invalid Codex stdio response JSON: {error}"))
    })?;
    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| error.as_str())
            .unwrap_or("unknown app-server error");
        validate_nonempty_trimmed("Codex stdio error", message)?;
        return Err(CamError::DeliveryFailed(format!(
            "Codex stdio app-server error response: {message}"
        )));
    }
    Ok(value)
}

fn optional_value_string(value: &Value, field: &str) -> Result<Option<String>, CamError> {
    match value.get(field) {
        Some(field_value) if field_value.is_null() => Ok(None),
        Some(field_value) => {
            let parsed = field_value.as_str().ok_or_else(|| {
                CamError::ProviderContractViolation(format!(
                    "Codex stdio response {field} proof must be a string"
                ))
            })?;
            validate_codex_stdio_proof_token(&format!("Codex stdio {field}"), parsed)?;
            Ok(Some(parsed.to_string()))
        }
        None => Ok(None),
    }
}

fn optional_warning(value: &Value) -> Result<Option<String>, CamError> {
    match value.get("warning") {
        Some(field_value) if field_value.is_null() => Ok(None),
        Some(field_value) => {
            let parsed = field_value.as_str().ok_or_else(|| {
                CamError::ProviderContractViolation(
                    "Codex stdio response warning proof must be a string".to_string(),
                )
            })?;
            validate_nonempty_trimmed("Codex stdio warning", parsed).map_err(|error| {
                CamError::ProviderContractViolation(format!(
                    "invalid Codex stdio lifecycle warning: {error}"
                ))
            })?;
            Ok(Some(parsed.to_string()))
        }
        None => Ok(None),
    }
}

fn parse_resume_response(
    request: &ProviderResumeRequest,
    line: &str,
) -> Result<ProviderResumeOutcome, CamError> {
    let value = parse_codex_stdio_value(line)?;
    let evidence = parse_lifecycle_evidence(&value, "resume")?;
    Ok(ProviderResumeOutcome {
        ready: evidence.ready,
        provider_resumed: true,
        target_agent: request.target_agent.clone(),
        thread_id: request.thread_id.clone(),
        active_turn_id: evidence.active_turn_id,
        last_turn_id: evidence.last_turn_id,
        transcript_mutated: false,
        warning: evidence.warning,
    })
}

fn parse_readiness_response(
    request: &ProviderReadinessRequest,
    line: &str,
) -> Result<ProviderReadinessOutcome, CamError> {
    let value = parse_codex_stdio_value(line)?;
    let evidence = parse_lifecycle_evidence(&value, "readiness")?;
    Ok(ProviderReadinessOutcome {
        ready: evidence.ready,
        target_agent: request.target_agent.clone(),
        thread_id: request.thread_id.clone(),
        active_turn_id: evidence.active_turn_id,
        last_turn_id: evidence.last_turn_id,
        provider_checked: true,
        transcript_mutated: false,
        warning: evidence.warning,
    })
}

struct CodexLifecycleEvidence {
    ready: bool,
    active_turn_id: Option<String>,
    last_turn_id: Option<String>,
    warning: Option<String>,
}

fn parse_lifecycle_evidence(
    value: &Value,
    operation: &str,
) -> Result<CodexLifecycleEvidence, CamError> {
    let status_type = value
        .get("thread")
        .and_then(|thread| thread.get("status"))
        .and_then(|status| status.get("type"))
        .and_then(Value::as_str)
        .or_else(|| value.get("status").and_then(Value::as_str))
        .ok_or_else(|| {
            CamError::ProviderContractViolation(format!(
                "Codex stdio {operation} response missing thread.status.type proof"
            ))
        })?;
    validate_nonempty_trimmed("Codex stdio lifecycle status", status_type)?;
    let active_turn_id = optional_value_string(value, "activeTurnId")?.or_else(|| {
        value
            .get("thread")
            .and_then(|thread| thread.get("activeTurnId"))
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    let last_turn_id = optional_value_string(value, "lastTurnId")?.or_else(|| {
        value
            .get("thread")
            .and_then(|thread| thread.get("lastTurnId"))
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    let ready = matches!(status_type, "active" | "idle")
        || active_turn_id.is_some()
        || last_turn_id.is_some();
    Ok(CodexLifecycleEvidence {
        ready,
        active_turn_id,
        last_turn_id,
        warning: optional_warning(value)?,
    })
}

fn negative_readiness_from_resume_error(
    request: &ProviderReadinessRequest,
    error: CamError,
) -> Result<ProviderReadinessOutcome, CamError> {
    let message = error.to_string();
    if message.contains("no rollout found for thread id")
        || message.contains("thread not found")
        || message.contains("no active turn to steer")
    {
        Ok(ProviderReadinessOutcome {
            ready: false,
            target_agent: request.target_agent.clone(),
            thread_id: request.thread_id.clone(),
            active_turn_id: None,
            last_turn_id: None,
            provider_checked: true,
            transcript_mutated: false,
            warning: None,
        })
    } else {
        Err(error)
    }
}

impl CodexCommandEnvelope {
    pub fn for_steer(request: &ProviderSendRequest) -> Result<Self, CamError> {
        if request.delivery_action != DeliveryAction::SteerActiveTurn {
            return Err(CamError::DeliveryFailed(format!(
                "Codex steer envelope requires SteerActiveTurn action, got `{:?}`",
                request.delivery_action
            )));
        }
        if request.expected_delivery != DeliveryState::Steered {
            return Err(CamError::DeliveryFailed(format!(
                "Codex steer envelope requires steered delivery, got `{:?}`",
                request.expected_delivery
            )));
        }
        let thread_id = request.thread_id.clone().ok_or_else(|| {
            CamError::DeliveryFailed("Codex steer envelope requires thread id".to_string())
        })?;
        let active_turn_id = request.active_turn_id.clone().ok_or_else(|| {
            CamError::DeliveryFailed("Codex steer envelope requires active turn id".to_string())
        })?;
        Ok(Self::from_request(
            request,
            CodexCommandOperation::SteerActiveTurn,
            thread_id,
            Some(active_turn_id),
        ))
    }

    pub fn for_wake(request: &ProviderSendRequest) -> Result<Self, CamError> {
        if request.delivery_action != DeliveryAction::WakeKnownSession {
            return Err(CamError::DeliveryFailed(format!(
                "Codex wake envelope requires WakeKnownSession action, got `{:?}`",
                request.delivery_action
            )));
        }
        if request.expected_delivery != DeliveryState::Delivered {
            return Err(CamError::DeliveryFailed(format!(
                "Codex wake envelope requires delivered delivery, got `{:?}`",
                request.expected_delivery
            )));
        }
        let thread_id = request.thread_id.clone().ok_or_else(|| {
            CamError::DeliveryFailed("Codex wake envelope requires thread id".to_string())
        })?;
        if request.active_turn_id.is_some() {
            return Err(CamError::DeliveryFailed(
                "Codex wake envelope must not include active turn id".to_string(),
            ));
        }
        Ok(Self::from_request(
            request,
            CodexCommandOperation::WakeKnownSession,
            thread_id,
            None,
        ))
    }

    fn from_request(
        request: &ProviderSendRequest,
        operation: CodexCommandOperation,
        thread_id: String,
        active_turn_id: Option<String>,
    ) -> Self {
        Self {
            operation,
            message_id: request.message_id.clone(),
            target_agent: request.target_agent.clone(),
            thread_id,
            active_turn_id,
            cwd: request.cwd.clone(),
            source_agent: request.source_agent.clone(),
            source_node: request.source_node.clone(),
            correlation_id: request.correlation_id.clone(),
            message_type: request.message_type.clone(),
            body: request.body.clone(),
            strict: request.strict,
        }
    }
}

pub trait CodexSendExecutor {
    fn steer_active_turn(
        &self,
        command: &CodexCommandEnvelope,
    ) -> Result<ProviderDeliveryReceipt, CamError>;
    fn wake_known_session(
        &self,
        command: &CodexCommandEnvelope,
    ) -> Result<ProviderDeliveryReceipt, CamError>;
}

pub trait CodexLifecycleExecutor {
    fn check_readiness(
        &self,
        request: &ProviderReadinessRequest,
    ) -> Result<ProviderReadinessOutcome, CamError>;

    fn resume(&self, request: &ProviderResumeRequest) -> Result<ProviderResumeOutcome, CamError>;
}

pub trait CodexStdioTransport {
    fn execute(&self, request: &CodexStdioRequest) -> Result<String, CamError>;
    fn execute_lifecycle(&self, request: &CodexLifecycleStdioRequest) -> Result<String, CamError>;
    fn execute_thread_start(
        &self,
        request: &CodexThreadStartStdioRequest,
    ) -> Result<String, CamError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexProcessOutput {
    pub status_success: bool,
    pub stdout: String,
    pub stderr: String,
}

pub trait CodexProcessRunner {
    fn run(
        &self,
        program: &str,
        args: &[&str],
        stdin: &str,
        expected_response_id: i64,
    ) -> Result<CodexProcessOutput, CamError>;
}

pub struct StdCommandCodexProcessRunner;

impl CodexProcessRunner for StdCommandCodexProcessRunner {
    fn run(
        &self,
        program: &str,
        args: &[&str],
        stdin: &str,
        expected_response_id: i64,
    ) -> Result<CodexProcessOutput, CamError> {
        let deadline = Instant::now() + CODEX_APP_SERVER_TIMEOUT;
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                CamError::DeliveryFailed(format!(
                    "failed to spawn Codex app-server stdio command `{program}`: {error}"
                ))
            })?;

        let mut child_stdin = child.stdin.take().ok_or_else(|| {
            CamError::DeliveryFailed(
                "failed to open stdin for Codex app-server stdio command".to_string(),
            )
        })?;
        child_stdin.write_all(stdin.as_bytes()).map_err(|error| {
            CamError::DeliveryFailed(format!(
                "failed to write Codex app-server stdio request: {error}"
            ))
        })?;
        child_stdin.flush().map_err(|error| {
            CamError::DeliveryFailed(format!(
                "failed to flush Codex app-server stdio request: {error}"
            ))
        })?;

        let stdout_pipe = child.stdout.take().ok_or_else(|| {
            CamError::DeliveryFailed("failed to open Codex app-server stdout".to_string())
        })?;
        let stderr_pipe = child.stderr.take().ok_or_else(|| {
            CamError::DeliveryFailed("failed to open Codex app-server stderr".to_string())
        })?;
        let (stdout_tx, stdout_rx) = mpsc::channel::<Result<String, String>>();
        let stdout_handle = thread::spawn(move || {
            let reader = BufReader::new(stdout_pipe);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        if stdout_tx.send(Ok(line)).is_err() {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = stdout_tx.send(Err(error.to_string()));
                        return;
                    }
                }
            }
        });
        let stderr_handle = thread::spawn(move || {
            let mut reader = BufReader::new(stderr_pipe);
            let mut stderr = String::new();
            let _ = reader.read_to_string(&mut stderr);
            stderr
        });

        let mut stdout = String::new();
        let mut matched_response = false;
        let process_status;
        loop {
            while let Ok(line_result) = stdout_rx.try_recv() {
                let line = line_result.map_err(|error| {
                    CamError::DeliveryFailed(format!(
                        "failed to read Codex app-server stdio stdout: {error}"
                    ))
                })?;
                stdout.push_str(&line);
                stdout.push('\n');
                if app_server_line_has_id(&line, expected_response_id) {
                    matched_response = true;
                    break;
                }
            }
            if matched_response {
                let _ = child.kill();
                process_status = child.wait().map_err(|error| {
                    CamError::DeliveryFailed(format!(
                        "failed to stop Codex app-server stdio command: {error}"
                    ))
                })?;
                break;
            }
            if let Some(status) = child.try_wait().map_err(|error| {
                CamError::DeliveryFailed(format!(
                    "failed to poll Codex app-server stdio command: {error}"
                ))
            })? {
                process_status = status;
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(CamError::DeliveryFailed(format!(
                    "Codex app-server stdio command timed out after {} seconds",
                    CODEX_APP_SERVER_TIMEOUT.as_secs()
                )));
            }
            thread::sleep(Duration::from_millis(50));
        }

        drop(child_stdin);
        let stderr = if matched_response {
            // Some Codex launchers leave a descendant holding stdio open after the
            // JSON-RPC response arrives. A successful one-shot command must not
            // block forever waiting for those reader threads to finish.
            String::new()
        } else {
            let _ = stdout_handle.join();
            stderr_handle.join().unwrap_or_default()
        };
        let status_success = if matched_response {
            true
        } else {
            process_status.success()
        };

        Ok(CodexProcessOutput {
            status_success,
            stdout,
            stderr,
        })
    }
}

fn app_server_line_has_id(line: &str, expected_response_id: i64) -> bool {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|value| {
            value
                .get("id")
                .and_then(Value::as_i64)
                .map(|id| id == expected_response_id)
        })
        .unwrap_or(false)
}

pub fn probe_app_server(program: Option<String>) -> CodexAppServerProbeOutput {
    let program = program.unwrap_or_else(default_codex_program);
    match probe_app_server_with_runner(&StdCommandCodexProcessRunner, program.clone()) {
        Ok(output) => output,
        Err(error) => CodexAppServerProbeOutput {
            ok: false,
            program: program.clone(),
            command: vec![
                program,
                "app-server".to_string(),
                "--listen".to_string(),
                "stdio://".to_string(),
            ],
            initialize_attempted: true,
            initialized: false,
            server_name: None,
            server_version: None,
            user_agent: None,
            codex_home: None,
            platform_family: None,
            platform_os: None,
            error: Some(error.to_string()),
        },
    }
}

pub fn probe_app_server_with_runner(
    runner: &impl CodexProcessRunner,
    program: String,
) -> Result<CodexAppServerProbeOutput, CamError> {
    validate_nonempty_trimmed("Codex app-server program", &program)?;
    let stdin = app_server_initialize_line()?;
    let command = vec![
        program.clone(),
        "app-server".to_string(),
        "--listen".to_string(),
        "stdio://".to_string(),
    ];
    let output = runner.run(&program, &["app-server", "--listen", "stdio://"], &stdin, 1)?;
    if !output.status_success {
        let stderr = output.stderr.trim();
        let detail = if stderr.is_empty() {
            "process exited unsuccessfully without stderr".to_string()
        } else {
            stderr.to_string()
        };
        return Ok(CodexAppServerProbeOutput {
            ok: false,
            program,
            command,
            initialize_attempted: true,
            initialized: false,
            server_name: None,
            server_version: None,
            user_agent: None,
            codex_home: None,
            platform_family: None,
            platform_os: None,
            error: Some(format!("Codex app-server stdio command failed: {detail}")),
        });
    }
    match parse_app_server_initialize_probe(&output.stdout) {
        Ok(initialize) => Ok(CodexAppServerProbeOutput {
            ok: true,
            program,
            command,
            initialize_attempted: true,
            initialized: true,
            server_name: initialize.server_name,
            server_version: initialize.server_version,
            user_agent: initialize.user_agent,
            codex_home: initialize.codex_home,
            platform_family: initialize.platform_family,
            platform_os: initialize.platform_os,
            error: None,
        }),
        Err(error) => Ok(CodexAppServerProbeOutput {
            ok: false,
            program,
            command,
            initialize_attempted: true,
            initialized: false,
            server_name: None,
            server_version: None,
            user_agent: None,
            codex_home: None,
            platform_family: None,
            platform_os: None,
            error: Some(error.to_string()),
        }),
    }
}

pub struct CodexStdioProcessTransport {
    program: String,
    model: Option<String>,
}

impl CodexStdioProcessTransport {
    pub fn new() -> Self {
        Self {
            program: default_codex_program(),
            model: None,
        }
    }

    pub fn with_program(program: impl Into<String>) -> Result<Self, CamError> {
        let program = program.into();
        validate_nonempty_trimmed("Codex app-server program", &program)?;
        Ok(Self {
            program,
            model: None,
        })
    }

    pub fn with_program_and_model(
        program: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<Self, CamError> {
        let program = program.into();
        let model = model.into();
        validate_nonempty_trimmed("Codex app-server program", &program)?;
        validate_nonempty_trimmed("Codex app-server model", &model)?;
        Ok(Self {
            program,
            model: Some(model),
        })
    }
}

fn default_codex_program() -> String {
    if let Ok(program) = std::env::var("CAM_CODEX_EXE") {
        if !program.trim().is_empty() {
            return program;
        }
    }
    if let Ok(program) = std::env::var("QEXOW_CAM_CODEX_PROGRAM") {
        if !program.trim().is_empty() {
            return program;
        }
    }
    if cfg!(windows) {
        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            let candidate = PathBuf::from(local_app_data)
                .join("OpenAI")
                .join("Codex")
                .join("bin")
                .join("codex.exe");
            if candidate.is_file() {
                return candidate.display().to_string();
            }
        }
    }
    "codex".to_string()
}

impl CodexStdioTransport for CodexStdioProcessTransport {
    fn execute(&self, request: &CodexStdioRequest) -> Result<String, CamError> {
        if request.method == "turn/steer" {
            return Err(CamError::ProviderUnavailable(
                "Codex turn/steer requires the app-server process that owns the active turn; one-shot process transport cannot steer live turns".to_string(),
            ));
        }
        let stdin = app_server_request_lines(&request.method, &request.params)?;
        self.execute_json_line(&stdin)
    }

    fn execute_lifecycle(&self, request: &CodexLifecycleStdioRequest) -> Result<String, CamError> {
        let stdin = app_server_request_lines(&request.method, &request.params)?;
        self.execute_json_line(&stdin)
    }

    fn execute_thread_start(
        &self,
        request: &CodexThreadStartStdioRequest,
    ) -> Result<String, CamError> {
        let stdin = app_server_request_lines(&request.method, &request.params)?;
        self.execute_json_line(&stdin)
    }
}

impl CodexStdioProcessTransport {
    fn execute_json_line(&self, stdin: &str) -> Result<String, CamError> {
        let request_values = stdin
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str::<Value>(line).map_err(|error| {
                    CamError::ProviderContractViolation(format!(
                        "invalid Codex app-server stdio request JSON: {error}"
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if request_values.len() != 2 {
            return Err(CamError::ProviderContractViolation(format!(
                "Codex app-server stdio command expected initialize plus one request, got {} request lines",
                request_values.len()
            )));
        }
        let mut session = match self.model.as_deref() {
            Some(model) => CodexAppServerSession::start_with_model(&self.program, model)?,
            None => CodexAppServerSession::start(&self.program)?,
        };
        session.initialize()?;
        session.write_json_value(&request_values[1])?;
        session.read_result(2)
    }
}

fn app_server_initialize_line() -> Result<String, CamError> {
    let initialize = serde_json::json!({
        "id": 1,
        "method": "initialize",
        "params": {
            "clientInfo": { "name": "qexow-cam", "version": "0.1.0" },
            "capabilities": { "experimentalApi": true }
        }
    });
    Ok(format!("{}\n", serde_json::to_string(&initialize)?))
}

fn app_server_initialized_notification_value() -> Value {
    serde_json::json!({
        "method": "notifications/initialized",
        "params": {}
    })
}

fn app_server_request_lines(method: &str, params: &impl Serialize) -> Result<String, CamError> {
    let request = app_server_request_value(2, method, params)?;
    Ok(format!(
        "{}{}\n",
        app_server_initialize_line()?,
        serde_json::to_string(&request)?
    ))
}

fn app_server_request_value(
    id: i64,
    method: &str,
    params: &impl Serialize,
) -> Result<Value, CamError> {
    let request = serde_json::json!({
        "id": id,
        "method": method,
        "params": params
    });
    Ok(request)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodexAppServerInitializeProbe {
    server_name: Option<String>,
    server_version: Option<String>,
    user_agent: Option<String>,
    codex_home: Option<String>,
    platform_family: Option<String>,
    platform_os: Option<String>,
}

fn parse_app_server_initialize_probe(
    stdout: &str,
) -> Result<CodexAppServerInitializeProbe, CamError> {
    let result = parse_app_server_stdout_result(stdout, 1)?;
    let value = serde_json::from_str::<Value>(&result).map_err(|error| {
        CamError::ProviderContractViolation(format!(
            "invalid Codex app-server initialize result JSON: {error}"
        ))
    })?;
    let server_info = value.get("serverInfo");
    let server_name = server_info
        .and_then(|server_info| server_info.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let server_version = server_info
        .and_then(|server_info| server_info.get("version"))
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok(CodexAppServerInitializeProbe {
        server_name,
        server_version,
        user_agent: optional_result_string(&value, "userAgent"),
        codex_home: optional_result_string(&value, "codexHome"),
        platform_family: optional_result_string(&value, "platformFamily"),
        platform_os: optional_result_string(&value, "platformOs"),
    })
}

fn optional_result_string(value: &Value, field: &str) -> Option<String> {
    value.get(field).and_then(Value::as_str).map(str::to_string)
}

fn parse_app_server_stdout_result(stdout: &str, expected_id: i64) -> Result<String, CamError> {
    for line in stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let value = serde_json::from_str::<Value>(line).map_err(|error| {
            CamError::ProviderContractViolation(format!(
                "invalid Codex app-server stdio response JSON: {error}"
            ))
        })?;
        if value.get("id").and_then(Value::as_i64) != Some(expected_id) {
            continue;
        }
        if let Some(error) = value.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| error.as_str())
                .unwrap_or("unknown app-server error");
            validate_nonempty_trimmed("Codex app-server error", message)?;
            return Err(CamError::DeliveryFailed(format!(
                "Codex app-server stdio command returned error: {message}"
            )));
        }
        let result = value.get("result").ok_or_else(|| {
            CamError::ProviderContractViolation(
                "Codex app-server stdio response missing result".to_string(),
            )
        })?;
        return Ok(serde_json::to_string(result)?);
    }

    Err(CamError::DeliveryFailed(format!(
        "Codex app-server stdio command produced no response for id {expected_id}"
    )))
}

pub struct CodexStdioExecutor<T> {
    transport: T,
}

impl<T> CodexStdioExecutor<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }
}

impl<T: CodexStdioTransport> CodexSendExecutor for CodexStdioExecutor<T> {
    fn steer_active_turn(
        &self,
        command: &CodexCommandEnvelope,
    ) -> Result<ProviderDeliveryReceipt, CamError> {
        let request = CodexStdioRequest::new(command.clone());
        let response = self.transport.execute(&request)?;
        let response = CodexStdioResponse::parse_for_command(command, &response)?;
        Ok(ProviderDeliveryReceipt {
            message_id: response.message_id,
            target_agent: response.target_agent,
            thread_id: response.thread_id,
            turn_id: response.turn_id,
        })
    }

    fn wake_known_session(
        &self,
        command: &CodexCommandEnvelope,
    ) -> Result<ProviderDeliveryReceipt, CamError> {
        let request = CodexStdioRequest::new(command.clone());
        let response = self.transport.execute(&request)?;
        let response = CodexStdioResponse::parse_for_command(command, &response)?;
        Ok(ProviderDeliveryReceipt {
            message_id: response.message_id,
            target_agent: response.target_agent,
            thread_id: response.thread_id,
            turn_id: response.turn_id,
        })
    }
}

impl<T: CodexStdioTransport> CodexLifecycleExecutor for CodexStdioExecutor<T> {
    fn check_readiness(
        &self,
        request: &ProviderReadinessRequest,
    ) -> Result<ProviderReadinessOutcome, CamError> {
        let stdio_request = CodexLifecycleStdioRequest::for_readiness(request);
        match self.transport.execute_lifecycle(&stdio_request) {
            Ok(response) => parse_readiness_response(request, &response),
            Err(error) => negative_readiness_from_resume_error(request, error),
        }
    }

    fn resume(&self, request: &ProviderResumeRequest) -> Result<ProviderResumeOutcome, CamError> {
        let stdio_request = CodexLifecycleStdioRequest::for_resume(request);
        let response = self.transport.execute_lifecycle(&stdio_request)?;
        parse_resume_response(request, &response)
    }
}

pub fn send(request: ProviderSendRequest) -> Result<ProviderSendOutcome, CamError> {
    send_with_executor(
        request,
        &CodexStdioExecutor::new(CodexStdioProcessTransport::new()),
    )
}

pub fn send_with_executor(
    request: ProviderSendRequest,
    executor: &impl CodexSendExecutor,
) -> Result<ProviderSendOutcome, CamError> {
    match request.delivery_action {
        DeliveryAction::SteerActiveTurn => steer_active_turn(request, executor),
        DeliveryAction::WakeKnownSession => wake_known_session(request, executor),
        DeliveryAction::StoreInVirtualInbox
        | DeliveryAction::QueueFallback
        | DeliveryAction::FailLoudly => Err(CamError::DeliveryFailed(format!(
            "Codex provider cannot handle delivery action `{:?}`",
            request.delivery_action
        ))),
    }
}

pub fn check_readiness_with_executor(
    request: ProviderReadinessRequest,
    executor: &impl CodexLifecycleExecutor,
) -> Result<ProviderReadinessOutcome, CamError> {
    let outcome = executor.check_readiness(&request)?;
    validate_provider_readiness_outcome(&request, &outcome)?;
    Ok(outcome)
}

pub fn resume_with_executor(
    request: ProviderResumeRequest,
    executor: &impl CodexLifecycleExecutor,
) -> Result<ProviderResumeOutcome, CamError> {
    let outcome = executor.resume(&request)?;
    validate_provider_resume_outcome(&request, &outcome)?;
    Ok(outcome)
}

pub fn read_transcript(
    request: crate::providers::ProviderTranscriptRequest,
) -> Result<crate::providers::ProviderTranscriptOutcome, CamError> {
    let session_path = find_codex_session_file(&request.thread_id)?.ok_or_else(|| {
        CamError::ProviderUnavailable(format!(
            "Codex session transcript file was not found for thread `{}`",
            request.thread_id
        ))
    })?;
    let transcript = fs::read_to_string(&session_path).map_err(|error| {
        CamError::ProviderUnavailable(format!(
            "failed to read Codex session transcript `{}`: {error}",
            session_path.display()
        ))
    })?;
    let message = latest_assistant_message_from_session_jsonl(&transcript).ok_or_else(|| {
        CamError::ProviderUnavailable(format!(
            "Codex session transcript `{}` contains no assistant response content",
            session_path.display()
        ))
    })?;
    Ok(crate::providers::ProviderTranscriptOutcome {
        target_agent: request.target_agent,
        thread_id: request.thread_id,
        provider_checked: true,
        transcript_available: true,
        transcript_source: format!("codex_session_jsonl:{}", session_path.display()),
        latest_turn_id: message.turn_id,
        summary: Some(message.text),
        transcript_mutated: false,
        warning: None,
    })
}

fn find_codex_session_file(thread_id: &str) -> Result<Option<PathBuf>, CamError> {
    validate_codex_stdio_proof_token("Codex transcript thread id", thread_id)?;
    let root = codex_home().join("sessions");
    if !root.is_dir() {
        return Ok(None);
    }
    find_session_file_by_name(&root, thread_id)
}

fn find_session_file_by_name(root: &Path, thread_id: &str) -> Result<Option<PathBuf>, CamError> {
    for entry in fs::read_dir(root).map_err(|error| {
        CamError::ProviderUnavailable(format!(
            "failed to read Codex sessions directory `{}`: {error}",
            root.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            CamError::ProviderUnavailable(format!(
                "failed to inspect Codex sessions directory `{}`: {error}",
                root.display()
            ))
        })?;
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_session_file_by_name(&path, thread_id)? {
                return Ok(Some(found));
            }
            continue;
        }
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.contains(thread_id) && name.ends_with(".jsonl"))
        {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn codex_home() -> PathBuf {
    if let Ok(home) = std::env::var("CODEX_HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home);
        }
    }
    home_dir().join(".codex")
}

fn home_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home);
        }
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        if !profile.trim().is_empty() {
            return PathBuf::from(profile);
        }
    }
    PathBuf::from(".")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodexTranscriptMessage {
    turn_id: Option<String>,
    text: String,
}

fn latest_assistant_message_from_session_jsonl(transcript: &str) -> Option<CodexTranscriptMessage> {
    transcript
        .lines()
        .filter_map(latest_assistant_message_from_session_line)
        .last()
}

fn latest_assistant_message_from_session_line(line: &str) -> Option<CodexTranscriptMessage> {
    let value = serde_json::from_str::<Value>(line).ok()?;
    if value.get("type").and_then(Value::as_str) == Some("event_msg") {
        let payload = value.get("payload")?;
        if payload.get("type").and_then(Value::as_str) == Some("task_complete") {
            let text = payload.get("last_agent_message").and_then(Value::as_str)?;
            if text.trim().is_empty() {
                return None;
            }
            return Some(CodexTranscriptMessage {
                turn_id: payload
                    .get("turn_id")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                text: text.to_string(),
            });
        }
        if payload.get("type").and_then(Value::as_str) == Some("agent_message") {
            return payload.get("message").and_then(Value::as_str).map(|text| {
                CodexTranscriptMessage {
                    turn_id: None,
                    text: text.to_string(),
                }
            });
        }
    }
    if value.get("type").and_then(Value::as_str) != Some("response_item") {
        return None;
    }
    let payload = value.get("payload")?;
    if payload.get("type").and_then(Value::as_str) != Some("message") {
        return None;
    }
    if payload.get("role").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let text = payload
        .get("content")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|item| item.get("text").or_else(|| item.get("output_text")))
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    if text.trim().is_empty() {
        None
    } else {
        Some(CodexTranscriptMessage {
            turn_id: None,
            text,
        })
    }
}

pub fn start_thread_with_program(
    agent: &crate::core::Agent,
    program: Option<String>,
) -> Result<CodexThreadStartOutcome, CamError> {
    let transport =
        codex_stdio_process_transport_for_program_and_model(program, agent.model.clone())?;
    start_thread_with_transport(agent, &transport)
}

fn codex_stdio_process_transport_for_program_and_model(
    program: Option<String>,
    model: Option<String>,
) -> Result<CodexStdioProcessTransport, CamError> {
    match (program, model) {
        (Some(program), Some(model)) => {
            CodexStdioProcessTransport::with_program_and_model(program, model)
        }
        (Some(program), None) => CodexStdioProcessTransport::with_program(program),
        (None, Some(model)) => {
            CodexStdioProcessTransport::with_program_and_model(default_codex_program(), model)
        }
        (None, None) => Ok(CodexStdioProcessTransport::new()),
    }
}

pub fn start_thread_with_transport(
    agent: &crate::core::Agent,
    transport: &impl CodexStdioTransport,
) -> Result<CodexThreadStartOutcome, CamError> {
    if agent.kind != crate::core::AgentKind::Codex {
        return Err(CamError::DeliveryFailed(
            "Codex thread/start requires a Codex agent".to_string(),
        ));
    }
    let request = CodexThreadStartStdioRequest::from_agent(agent)?;
    let response = transport.execute_thread_start(&request)?;
    parse_thread_start_response(agent, &response)
}

pub fn start_thread_then_wake_with_program(
    agent: &crate::core::Agent,
    message: &crate::core::Message,
    program: Option<String>,
) -> Result<CodexThreadStartThenSendOutcome, CamError> {
    start_thread_then_wake_with_program_and_model(agent, message, program, None)
}

pub fn start_thread_then_wake_with_program_and_model(
    agent: &crate::core::Agent,
    message: &crate::core::Message,
    program: Option<String>,
    model: Option<String>,
) -> Result<CodexThreadStartThenSendOutcome, CamError> {
    let mut owner = CodexAppServerOwner::start_for_program_and_model(program, model)?;
    start_thread_then_wake_with_owner(agent, message, &mut owner)
}

pub fn start_thread_then_wake_with_owner(
    agent: &crate::core::Agent,
    message: &crate::core::Message,
    owner: &mut CodexAppServerOwner,
) -> Result<CodexThreadStartThenSendOutcome, CamError> {
    if agent.kind != crate::core::AgentKind::Codex {
        return Err(CamError::DeliveryFailed(
            "Codex thread/start then wake requires a Codex agent".to_string(),
        ));
    }
    if agent.thread_id.is_some() {
        return Err(CamError::DeliveryFailed(
            "Codex thread/start then wake requires an agent without a thread id".to_string(),
        ));
    }
    owner.initialize()?;
    let thread_start = owner.start_thread(agent)?;

    let command = CodexCommandEnvelope {
        operation: CodexCommandOperation::WakeKnownSession,
        message_id: message.message_id.clone(),
        target_agent: agent.name.clone(),
        thread_id: thread_start.thread_id.clone(),
        active_turn_id: None,
        cwd: agent.cwd.clone(),
        source_agent: message.source_agent.clone(),
        source_node: message.source_node.clone(),
        correlation_id: message.correlation_id.clone(),
        message_type: message.message_type.clone(),
        body: message.body.clone(),
        strict: message.strict,
    };
    let receipt = owner.execute_send_command(&command)?;
    Ok(CodexThreadStartThenSendOutcome {
        thread_start,
        send: ProviderSendOutcome {
            delivery: DeliveryState::Delivered,
            message_id: Some(receipt.message_id),
            target_agent: Some(receipt.target_agent),
            thread_id: Some(receipt.thread_id),
            turn_id: Some(receipt.turn_id),
        },
    })
}

pub struct CodexAppServerOwner {
    session: CodexAppServerSession,
}

pub struct CodexOwnerRegistry {
    owners_by_thread_id: BTreeMap<String, CodexAppServerOwner>,
}

pub struct CodexOwnerResumeOutcome {
    pub resume: ProviderResumeOutcome,
    pub owner_retained: bool,
}

impl CodexOwnerRegistry {
    pub fn new() -> Self {
        Self {
            owners_by_thread_id: BTreeMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.owners_by_thread_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.owners_by_thread_id.is_empty()
    }

    pub fn has_owner_for_thread(&self, thread_id: &str) -> bool {
        self.owners_by_thread_id.contains_key(thread_id)
    }

    pub fn start_thread_then_wake_with_program_and_model(
        &mut self,
        agent: &crate::core::Agent,
        message: &crate::core::Message,
        program: Option<String>,
        model: Option<String>,
    ) -> Result<CodexThreadStartThenSendOutcome, CamError> {
        let mut owner = CodexAppServerOwner::start_for_program_and_model(program, model)?;
        let outcome = start_thread_then_wake_with_owner(agent, message, &mut owner)?;
        self.owners_by_thread_id
            .insert(outcome.thread_start.thread_id.clone(), owner);
        Ok(outcome)
    }

    pub fn resume_existing_thread_with_program_and_model(
        &mut self,
        agent: &crate::core::Agent,
        program: Option<String>,
        model: Option<String>,
    ) -> Result<CodexOwnerResumeOutcome, CamError> {
        let mut owner = CodexAppServerOwner::start_for_program_and_model(program, model)?;
        let outcome = owner.resume_existing_thread(agent)?;
        let owner_retained =
            outcome.ready && (outcome.active_turn_id.is_some() || outcome.last_turn_id.is_some());
        if owner_retained {
            self.owners_by_thread_id
                .insert(outcome.thread_id.clone(), owner);
        }
        Ok(CodexOwnerResumeOutcome {
            resume: outcome,
            owner_retained,
        })
    }

    pub fn wake_known_session_with_program_and_model(
        &mut self,
        request: ProviderSendRequest,
        program: Option<String>,
        model: Option<String>,
    ) -> Result<ProviderSendOutcome, CamError> {
        if request.delivery_action != DeliveryAction::WakeKnownSession {
            return Err(CamError::DeliveryFailed(format!(
                "Codex owner registry wake requires WakeKnownSession action, got `{:?}`",
                request.delivery_action
            )));
        }
        if request.expected_delivery != DeliveryState::Delivered {
            return Err(CamError::DeliveryFailed(format!(
                "Codex owner registry wake requires delivered delivery, got `{:?}`",
                request.expected_delivery
            )));
        }
        let thread_id = request.thread_id.clone().ok_or_else(|| {
            CamError::DeliveryFailed("Codex owner registry wake requires thread id".to_string())
        })?;
        if !self.owners_by_thread_id.contains_key(&thread_id) {
            let mut owner = CodexAppServerOwner::start_for_program_and_model(program, model)?;
            owner.initialize()?;
            self.owners_by_thread_id.insert(thread_id.clone(), owner);
        }
        let command = CodexCommandEnvelope::for_wake(&request)?;
        let owner = self
            .owners_by_thread_id
            .get_mut(&thread_id)
            .ok_or_else(|| {
                CamError::ProviderUnavailable(format!(
                    "Codex owner registry has no live app-server owner for thread `{thread_id}`"
                ))
            })?;
        let receipt = owner.wake_known_session(&command)?;
        let outcome = ProviderSendOutcome {
            delivery: DeliveryState::Delivered,
            message_id: Some(receipt.message_id),
            target_agent: Some(receipt.target_agent),
            thread_id: Some(receipt.thread_id),
            turn_id: Some(receipt.turn_id),
        };
        validate_provider_send_outcome(&request, &outcome)?;
        Ok(outcome)
    }

    pub fn steer_active_turn(
        &mut self,
        request: ProviderSendRequest,
    ) -> Result<ProviderSendOutcome, CamError> {
        if request.delivery_action != DeliveryAction::SteerActiveTurn {
            return Err(CamError::DeliveryFailed(format!(
                "Codex owner registry steer requires SteerActiveTurn action, got `{:?}`",
                request.delivery_action
            )));
        }
        if request.expected_delivery != DeliveryState::Steered {
            return Err(CamError::DeliveryFailed(format!(
                "Codex owner registry steer requires steered delivery, got `{:?}`",
                request.expected_delivery
            )));
        }
        let thread_id = request.thread_id.clone().ok_or_else(|| {
            CamError::DeliveryFailed("Codex owner registry steer requires thread id".to_string())
        })?;
        let command = CodexCommandEnvelope::for_steer(&request)?;
        let owner = self
            .owners_by_thread_id
            .get_mut(&thread_id)
            .ok_or_else(|| {
                CamError::ProviderUnavailable(format!(
                    "Codex owner registry has no live app-server owner for thread `{thread_id}`"
                ))
            })?;
        let receipt = owner.steer_active_turn(&command)?;
        let outcome = ProviderSendOutcome {
            delivery: DeliveryState::Steered,
            message_id: Some(receipt.message_id),
            target_agent: Some(receipt.target_agent),
            thread_id: Some(receipt.thread_id),
            turn_id: Some(receipt.turn_id),
        };
        validate_provider_send_outcome(&request, &outcome)?;
        Ok(outcome)
    }
}

impl Default for CodexOwnerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl CodexAppServerOwner {
    pub fn start(program: &str) -> Result<Self, CamError> {
        Ok(Self {
            session: CodexAppServerSession::start(program)?,
        })
    }

    pub fn start_for_program(program: Option<String>) -> Result<Self, CamError> {
        Self::start_for_program_and_model(program, None)
    }

    pub fn start_for_program_and_model(
        program: Option<String>,
        model: Option<String>,
    ) -> Result<Self, CamError> {
        let program = match program {
            Some(program) => {
                validate_nonempty_trimmed("Codex app-server program", &program)?;
                program
            }
            None => default_codex_program(),
        };
        match model {
            Some(model) => {
                validate_nonempty_trimmed("Codex app-server model", &model)?;
                Self::start_with_model(&program, &model)
            }
            None => Self::start(&program),
        }
    }

    pub fn start_with_model(program: &str, model: &str) -> Result<Self, CamError> {
        Ok(Self {
            session: CodexAppServerSession::start_with_model(program, model)?,
        })
    }

    pub fn initialize(&mut self) -> Result<(), CamError> {
        self.session.initialize()
    }

    pub fn start_thread(
        &mut self,
        agent: &crate::core::Agent,
    ) -> Result<CodexThreadStartOutcome, CamError> {
        let request = CodexThreadStartStdioRequest::from_agent(agent)?;
        let response = self.session.request(&request.method, &request.params)?;
        parse_thread_start_response(agent, &response)
    }

    pub fn resume_existing_thread(
        &mut self,
        agent: &crate::core::Agent,
    ) -> Result<ProviderResumeOutcome, CamError> {
        self.initialize()?;
        let request = ProviderResumeRequest::from_agent(agent)?;
        let stdio_request = CodexLifecycleStdioRequest::for_resume(&request);
        let response = self
            .session
            .request(&stdio_request.method, &stdio_request.params)?;
        let outcome = parse_resume_response(&request, &response)?;
        validate_provider_resume_outcome(&request, &outcome)?;
        Ok(outcome)
    }

    pub fn wake_known_session(
        &mut self,
        command: &CodexCommandEnvelope,
    ) -> Result<ProviderDeliveryReceipt, CamError> {
        if command.operation != CodexCommandOperation::WakeKnownSession {
            return Err(CamError::DeliveryFailed(
                "Codex app-server owner wake requires a wake-known-session command".to_string(),
            ));
        }
        self.resume_thread_for_wake(command)?;
        self.execute_send_command(command)
    }

    fn resume_thread_for_wake(&mut self, command: &CodexCommandEnvelope) -> Result<(), CamError> {
        let request = ProviderResumeRequest {
            target_agent: command.target_agent.clone(),
            thread_id: command.thread_id.clone(),
            active_turn_id: None,
            cwd: command.cwd.clone(),
        };
        let stdio_request = CodexLifecycleStdioRequest::for_resume(&request);
        let response = self
            .session
            .request(&stdio_request.method, &stdio_request.params)?;
        let outcome = parse_resume_response(&request, &response)?;
        validate_provider_resume_outcome(&request, &outcome)?;
        if !outcome.ready {
            return Err(CamError::ProviderUnavailable(format!(
                "Codex thread `{}` is not ready after resume",
                command.thread_id
            )));
        }
        Ok(())
    }

    pub fn steer_active_turn(
        &mut self,
        command: &CodexCommandEnvelope,
    ) -> Result<ProviderDeliveryReceipt, CamError> {
        if command.operation != CodexCommandOperation::SteerActiveTurn {
            return Err(CamError::DeliveryFailed(
                "Codex app-server owner steer requires a steer-active-turn command".to_string(),
            ));
        }
        self.execute_send_command(command)
    }

    fn execute_send_command(
        &mut self,
        command: &CodexCommandEnvelope,
    ) -> Result<ProviderDeliveryReceipt, CamError> {
        let request = CodexStdioRequest::new(command.clone());
        let response = self.session.request(&request.method, &request.params)?;
        let response = CodexStdioResponse::parse_for_command(command, &response)?;
        wait_for_codex_assistant_response(&response.thread_id, &response.turn_id)?;
        Ok(ProviderDeliveryReceipt {
            message_id: response.message_id,
            target_agent: response.target_agent,
            thread_id: response.thread_id,
            turn_id: response.turn_id,
        })
    }
}

fn wait_for_codex_assistant_response(thread_id: &str, turn_id: &str) -> Result<(), CamError> {
    let deadline = Instant::now() + CODEX_TURN_COMPLETION_TIMEOUT;
    loop {
        if let Some(path) = find_codex_session_file(thread_id)? {
            let transcript = fs::read_to_string(&path)?;
            if assistant_message_for_turn_from_session_jsonl(&transcript, turn_id).is_some() {
                return Ok(());
            }
            if transcript_turn_was_aborted(&transcript, turn_id) {
                return Err(CamError::ProviderUnavailable(format!(
                    "Codex turn `{turn_id}` for thread `{thread_id}` was aborted before assistant response content was available"
                )));
            }
        }
        if Instant::now() >= deadline {
            return Err(CamError::ProviderUnavailable(format!(
                "Codex turn `{turn_id}` for thread `{thread_id}` did not produce assistant response content within {} seconds",
                CODEX_TURN_COMPLETION_TIMEOUT.as_secs()
            )));
        }
        thread::sleep(Duration::from_millis(500));
    }
}

fn assistant_message_for_turn_from_session_jsonl(
    transcript: &str,
    turn_id: &str,
) -> Option<CodexTranscriptMessage> {
    transcript
        .lines()
        .filter_map(|line| {
            let value = serde_json::from_str::<Value>(line).ok()?;
            if value.get("type").and_then(Value::as_str) != Some("event_msg") {
                return None;
            }
            let payload = value.get("payload")?;
            if payload.get("type").and_then(Value::as_str) != Some("task_complete") {
                return None;
            }
            if payload.get("turn_id").and_then(Value::as_str) != Some(turn_id) {
                return None;
            }
            let text = payload.get("last_agent_message").and_then(Value::as_str)?;
            if text.trim().is_empty() {
                return None;
            }
            Some(CodexTranscriptMessage {
                turn_id: Some(turn_id.to_string()),
                text: text.to_string(),
            })
        })
        .last()
}

fn transcript_turn_was_aborted(transcript: &str, turn_id: &str) -> bool {
    transcript.lines().any(|line| {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return false;
        };
        if value.get("type").and_then(Value::as_str) != Some("event_msg") {
            return false;
        }
        let Some(payload) = value.get("payload") else {
            return false;
        };
        payload.get("type").and_then(Value::as_str) == Some("turn_aborted")
            && payload.get("turn_id").and_then(Value::as_str) == Some(turn_id)
    })
}

pub struct CodexAppServerSession {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout_rx: mpsc::Receiver<Result<String, String>>,
    stdout_handle: Option<thread::JoinHandle<()>>,
    stderr_handle: Option<thread::JoinHandle<String>>,
    stdout: String,
    next_request_id: i64,
    initialized: bool,
}

impl CodexAppServerSession {
    pub fn start(program: &str) -> Result<Self, CamError> {
        Self::start_with_args(program, &[])
    }

    pub fn start_with_model(program: &str, model: &str) -> Result<Self, CamError> {
        validate_nonempty_trimmed("Codex model", model)?;
        Self::start_with_args(program, &["-m", model])
    }

    fn start_with_args(program: &str, prefix_args: &[&str]) -> Result<Self, CamError> {
        let mut args = prefix_args.to_vec();
        args.extend(["app-server", "--listen", "stdio://"]);
        let mut child = Command::new(program)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                let command = std::iter::once(program.to_string())
                    .chain(args.iter().map(|arg| arg.to_string()))
                    .collect::<Vec<_>>()
                    .join(" ");
                CamError::DeliveryFailed(format!(
                    "failed to spawn Codex app-server stdio command `{command}`: {error}"
                ))
            })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            CamError::DeliveryFailed(
                "failed to open stdin for Codex app-server stdio command".to_string(),
            )
        })?;
        let stdout_pipe = child.stdout.take().ok_or_else(|| {
            CamError::DeliveryFailed("failed to open Codex app-server stdout".to_string())
        })?;
        let stderr_pipe = child.stderr.take().ok_or_else(|| {
            CamError::DeliveryFailed("failed to open Codex app-server stderr".to_string())
        })?;
        let (stdout_tx, stdout_rx) = mpsc::channel::<Result<String, String>>();
        let stdout_handle = thread::spawn(move || {
            let reader = BufReader::new(stdout_pipe);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        if stdout_tx.send(Ok(line)).is_err() {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = stdout_tx.send(Err(error.to_string()));
                        return;
                    }
                }
            }
        });
        let stderr_handle = thread::spawn(move || {
            let mut reader = BufReader::new(stderr_pipe);
            let mut stderr = String::new();
            let _ = reader.read_to_string(&mut stderr);
            stderr
        });
        Ok(Self {
            child,
            stdin,
            stdout_rx,
            stdout_handle: Some(stdout_handle),
            stderr_handle: Some(stderr_handle),
            stdout: String::new(),
            next_request_id: 2,
            initialized: false,
        })
    }

    pub fn initialize(&mut self) -> Result<(), CamError> {
        if self.initialized {
            return Ok(());
        }
        self.write_json_value(&serde_json::from_str::<Value>(
            app_server_initialize_line()?.trim(),
        )?)?;
        self.read_result(1)?;
        self.write_json_value(&app_server_initialized_notification_value())?;
        self.initialized = true;
        Ok(())
    }

    pub fn request(&mut self, method: &str, params: &impl Serialize) -> Result<String, CamError> {
        self.initialize()?;
        let request_id = self.next_request_id;
        self.next_request_id += 1;
        self.write_json_value(&app_server_request_value(request_id, method, params)?)?;
        self.read_result(request_id)
    }

    fn write_json_value(&mut self, value: &Value) -> Result<(), CamError> {
        writeln!(self.stdin, "{}", serde_json::to_string(value)?).map_err(|error| {
            CamError::DeliveryFailed(format!(
                "failed to write Codex app-server stdio request: {error}"
            ))
        })?;
        self.stdin.flush().map_err(|error| {
            CamError::DeliveryFailed(format!(
                "failed to flush Codex app-server stdio request: {error}"
            ))
        })
    }

    fn read_result(&mut self, expected_response_id: i64) -> Result<String, CamError> {
        let deadline = Instant::now() + CODEX_APP_SERVER_TIMEOUT;
        loop {
            while let Ok(line_result) = self.stdout_rx.try_recv() {
                let line = line_result.map_err(|error| {
                    CamError::DeliveryFailed(format!(
                        "failed to read Codex app-server stdio stdout: {error}"
                    ))
                })?;
                self.stdout.push_str(&line);
                self.stdout.push('\n');
                if app_server_line_has_id(&line, expected_response_id) {
                    return parse_app_server_stdout_result(&line, expected_response_id);
                }
            }
            if let Some(status) = self.child.try_wait().map_err(|error| {
                CamError::DeliveryFailed(format!(
                    "failed to poll Codex app-server stdio command: {error}"
                ))
            })? {
                let stderr = self.take_stderr();
                let detail = if stderr.trim().is_empty() {
                    format!("process exited before response id {expected_response_id}: {status}")
                } else {
                    stderr
                };
                return Err(CamError::DeliveryFailed(format!(
                    "Codex app-server stdio command failed: {detail}"
                )));
            }
            if Instant::now() >= deadline {
                return Err(CamError::DeliveryFailed(format!(
                    "Codex app-server stdio command timed out after {} seconds",
                    CODEX_APP_SERVER_TIMEOUT.as_secs()
                )));
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn take_stderr(&mut self) -> String {
        self.stderr_handle
            .take()
            .and_then(|handle| {
                if handle.is_finished() {
                    handle.join().ok()
                } else {
                    None
                }
            })
            .unwrap_or_default()
    }
}

impl Drop for CodexAppServerSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(handle) = self.stdout_handle.take() {
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
        if let Some(handle) = self.stderr_handle.take() {
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
    }
}

fn parse_thread_start_response(
    agent: &crate::core::Agent,
    line: &str,
) -> Result<CodexThreadStartOutcome, CamError> {
    let value = parse_codex_stdio_value(line)?;
    let thread = value.get("thread").ok_or_else(|| {
        CamError::ProviderContractViolation(
            "Codex thread/start response missing thread proof".to_string(),
        )
    })?;
    let thread_id = thread.get("id").and_then(Value::as_str).ok_or_else(|| {
        CamError::ProviderContractViolation(
            "Codex thread/start response missing thread.id proof".to_string(),
        )
    })?;
    validate_codex_stdio_proof_token("Codex thread/start thread id", thread_id)?;
    let status = thread
        .get("status")
        .and_then(|status| status.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("idle");
    validate_nonempty_trimmed("Codex thread/start status", status)?;
    Ok(CodexThreadStartOutcome {
        target_agent: agent.name.clone(),
        thread_id: thread_id.to_string(),
        status: status.to_string(),
    })
}

fn steer_active_turn(
    request: ProviderSendRequest,
    executor: &impl CodexSendExecutor,
) -> Result<ProviderSendOutcome, CamError> {
    if request.expected_delivery != DeliveryState::Steered {
        return Err(CamError::DeliveryFailed(format!(
            "Codex steer expected delivery `steered`, got `{:?}`",
            request.expected_delivery
        )));
    }
    if request.thread_id.is_none() {
        return Err(CamError::DeliveryFailed(
            "Codex steer requires a known thread id".to_string(),
        ));
    }
    if request.active_turn_id.is_none() {
        return Err(CamError::DeliveryFailed(
            "Codex steer requires an active turn id".to_string(),
        ));
    }

    let command = CodexCommandEnvelope::for_steer(&request)?;
    let receipt = executor.steer_active_turn(&command)?;
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
    executor: &impl CodexSendExecutor,
) -> Result<ProviderSendOutcome, CamError> {
    if request.expected_delivery != DeliveryState::Delivered {
        return Err(CamError::DeliveryFailed(format!(
            "Codex wake expected delivery `delivered`, got `{:?}`",
            request.expected_delivery
        )));
    }
    if request.thread_id.is_none() {
        return Err(CamError::DeliveryFailed(
            "Codex wake/deliver requires a known thread id".to_string(),
        ));
    }

    let command = CodexCommandEnvelope::for_wake(&request)?;
    let receipt = executor.wake_known_session(&command)?;
    Ok(ProviderSendOutcome {
        delivery: DeliveryState::Delivered,
        message_id: Some(receipt.message_id),
        target_agent: Some(receipt.target_agent),
        thread_id: Some(receipt.thread_id),
        turn_id: Some(receipt.turn_id),
    })
}

fn validate_nonempty_token(label: &str, value: &str) -> Result<(), CamError> {
    validate_nonempty_trimmed(label, value)?;
    if value.chars().any(char::is_whitespace) {
        return Err(CamError::DeliveryFailed(format!(
            "{label} cannot contain whitespace"
        )));
    }
    Ok(())
}

fn validate_codex_stdio_proof_token(label: &str, value: &str) -> Result<(), CamError> {
    validate_nonempty_token(label, value).map_err(|error| {
        CamError::ProviderContractViolation(format!("invalid Codex stdio delivery proof: {error}"))
    })
}

fn validate_nonempty_trimmed(label: &str, value: &str) -> Result<(), CamError> {
    if value.trim().is_empty() {
        return Err(CamError::DeliveryFailed(format!("{label} cannot be empty")));
    }
    if value.trim() != value {
        return Err(CamError::DeliveryFailed(format!(
            "{label} must not have leading or trailing whitespace"
        )));
    }
    Ok(())
}
