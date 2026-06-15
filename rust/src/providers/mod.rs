pub mod agy;
pub mod codex;

use crate::core::{Agent, AgentKind, DeliveryState, Message};
use crate::delivery::DeliveryAction;
use crate::errors::CamError;
use agy::{AgyLifecycleExecutor, AgySendExecutor};
use codex::{
    CodexLifecycleExecutor, CodexSendExecutor, CodexStdioExecutor, CodexStdioProcessTransport,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSendRequest {
    pub message_id: String,
    pub target_agent: String,
    pub thread_id: Option<String>,
    pub active_turn_id: Option<String>,
    pub cwd: Option<String>,
    pub source_agent: Option<String>,
    pub source_node: Option<String>,
    pub correlation_id: Option<String>,
    pub message_type: Option<String>,
    pub body: String,
    pub strict: bool,
    pub delivery_action: DeliveryAction,
    pub expected_delivery: DeliveryState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSendOutcome {
    pub delivery: DeliveryState,
    pub message_id: Option<String>,
    pub target_agent: Option<String>,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDeliveryReceipt {
    pub message_id: String,
    pub target_agent: String,
    pub thread_id: String,
    pub turn_id: String,
}

pub trait ProviderRouter {
    fn send(
        &self,
        agent: &Agent,
        request: ProviderSendRequest,
    ) -> Result<ProviderSendOutcome, CamError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderReadinessRequest {
    pub target_agent: String,
    pub thread_id: String,
    pub active_turn_id: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderReadinessOutcome {
    pub ready: bool,
    pub target_agent: String,
    pub thread_id: String,
    pub active_turn_id: Option<String>,
    pub last_turn_id: Option<String>,
    pub provider_checked: bool,
    pub transcript_mutated: bool,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderResumeRequest {
    pub target_agent: String,
    pub thread_id: String,
    pub active_turn_id: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderResumeOutcome {
    pub ready: bool,
    pub provider_resumed: bool,
    pub target_agent: String,
    pub thread_id: String,
    pub active_turn_id: Option<String>,
    pub last_turn_id: Option<String>,
    pub transcript_mutated: bool,
    pub warning: Option<String>,
}

pub trait ProviderLifecycleRouter {
    fn check_readiness(
        &self,
        agent: &Agent,
        request: ProviderReadinessRequest,
    ) -> Result<ProviderReadinessOutcome, CamError>;

    fn resume(
        &self,
        agent: &Agent,
        request: ProviderResumeRequest,
    ) -> Result<ProviderResumeOutcome, CamError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderTranscriptRequest {
    pub target_agent: String,
    pub thread_id: String,
    pub latest_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderTranscriptOutcome {
    pub target_agent: String,
    pub thread_id: String,
    pub provider_checked: bool,
    pub transcript_available: bool,
    pub transcript_source: String,
    pub latest_turn_id: Option<String>,
    pub summary: Option<String>,
    pub transcript_mutated: bool,
    pub warning: Option<String>,
}

pub trait ProviderTranscriptReader {
    fn read_transcript(
        &self,
        agent: &Agent,
        request: ProviderTranscriptRequest,
    ) -> Result<ProviderTranscriptOutcome, CamError>;
}

pub struct DefaultProviderRouter;

impl ProviderRouter for DefaultProviderRouter {
    fn send(
        &self,
        agent: &Agent,
        request: ProviderSendRequest,
    ) -> Result<ProviderSendOutcome, CamError> {
        match agent.kind {
            AgentKind::Codex => codex::send(request),
            AgentKind::AgySession => agy::send(request),
            AgentKind::RemoteMirror => Err(CamError::ProviderUnavailable(
                "remote mirror delivery must be sent through the owning peer CAM".to_string(),
            )),
            AgentKind::VirtualInbox => Err(CamError::InvalidState(
                "virtual inbox delivery should not use provider adapters".to_string(),
            )),
        }
    }
}

impl ProviderLifecycleRouter for DefaultProviderRouter {
    fn check_readiness(
        &self,
        agent: &Agent,
        _request: ProviderReadinessRequest,
    ) -> Result<ProviderReadinessOutcome, CamError> {
        Err(lifecycle_unavailable_error(agent, "readiness"))
    }

    fn resume(
        &self,
        agent: &Agent,
        _request: ProviderResumeRequest,
    ) -> Result<ProviderResumeOutcome, CamError> {
        Err(lifecycle_unavailable_error(agent, "resume"))
    }
}

impl ProviderTranscriptReader for DefaultProviderRouter {
    fn read_transcript(
        &self,
        agent: &Agent,
        request: ProviderTranscriptRequest,
    ) -> Result<ProviderTranscriptOutcome, CamError> {
        match agent.kind {
            AgentKind::Codex => codex::read_transcript(request),
            AgentKind::AgySession => Err(transcript_unavailable_error(agent)),
            AgentKind::RemoteMirror => Err(transcript_unavailable_error(agent)),
            AgentKind::VirtualInbox => Err(transcript_unavailable_error(agent)),
        }
    }
}

pub struct CodexProviderRouter<E> {
    codex_executor: E,
}

impl<E> CodexProviderRouter<E> {
    pub fn new(codex_executor: E) -> Self {
        Self { codex_executor }
    }
}

impl<E: CodexSendExecutor> ProviderRouter for CodexProviderRouter<E> {
    fn send(
        &self,
        agent: &Agent,
        request: ProviderSendRequest,
    ) -> Result<ProviderSendOutcome, CamError> {
        match agent.kind {
            AgentKind::Codex => codex::send_with_executor(request, &self.codex_executor),
            AgentKind::AgySession => agy::send(request),
            AgentKind::RemoteMirror => Err(CamError::ProviderUnavailable(
                "remote mirror delivery must be sent through the owning peer CAM".to_string(),
            )),
            AgentKind::VirtualInbox => Err(CamError::InvalidState(
                "virtual inbox delivery should not use provider adapters".to_string(),
            )),
        }
    }
}

impl<E: CodexLifecycleExecutor> ProviderLifecycleRouter for CodexProviderRouter<E> {
    fn check_readiness(
        &self,
        agent: &Agent,
        request: ProviderReadinessRequest,
    ) -> Result<ProviderReadinessOutcome, CamError> {
        match agent.kind {
            AgentKind::Codex => codex::check_readiness_with_executor(request, &self.codex_executor),
            AgentKind::AgySession => Err(lifecycle_unavailable_error(agent, "readiness")),
            AgentKind::RemoteMirror => Err(lifecycle_unavailable_error(agent, "readiness")),
            AgentKind::VirtualInbox => Err(lifecycle_unavailable_error(agent, "readiness")),
        }
    }

    fn resume(
        &self,
        agent: &Agent,
        request: ProviderResumeRequest,
    ) -> Result<ProviderResumeOutcome, CamError> {
        match agent.kind {
            AgentKind::Codex => codex::resume_with_executor(request, &self.codex_executor),
            AgentKind::AgySession => Err(lifecycle_unavailable_error(agent, "resume")),
            AgentKind::RemoteMirror => Err(lifecycle_unavailable_error(agent, "resume")),
            AgentKind::VirtualInbox => Err(lifecycle_unavailable_error(agent, "resume")),
        }
    }
}

pub struct AgyProviderRouter<E> {
    agy_executor: E,
}

impl<E> AgyProviderRouter<E> {
    pub fn new(agy_executor: E) -> Self {
        Self { agy_executor }
    }
}

impl<E: AgySendExecutor> ProviderRouter for AgyProviderRouter<E> {
    fn send(
        &self,
        agent: &Agent,
        request: ProviderSendRequest,
    ) -> Result<ProviderSendOutcome, CamError> {
        match agent.kind {
            AgentKind::Codex => codex::send(request),
            AgentKind::AgySession => agy::send_with_executor(request, &self.agy_executor),
            AgentKind::RemoteMirror => Err(CamError::ProviderUnavailable(
                "remote mirror delivery must be sent through the owning peer CAM".to_string(),
            )),
            AgentKind::VirtualInbox => Err(CamError::InvalidState(
                "virtual inbox delivery should not use provider adapters".to_string(),
            )),
        }
    }
}

impl<E: AgyLifecycleExecutor> ProviderLifecycleRouter for AgyProviderRouter<E> {
    fn check_readiness(
        &self,
        agent: &Agent,
        request: ProviderReadinessRequest,
    ) -> Result<ProviderReadinessOutcome, CamError> {
        match agent.kind {
            AgentKind::Codex => Err(lifecycle_unavailable_error(agent, "readiness")),
            AgentKind::AgySession => {
                agy::check_readiness_with_executor(request, &self.agy_executor)
            }
            AgentKind::RemoteMirror => Err(lifecycle_unavailable_error(agent, "readiness")),
            AgentKind::VirtualInbox => Err(lifecycle_unavailable_error(agent, "readiness")),
        }
    }

    fn resume(
        &self,
        agent: &Agent,
        request: ProviderResumeRequest,
    ) -> Result<ProviderResumeOutcome, CamError> {
        match agent.kind {
            AgentKind::Codex => Err(lifecycle_unavailable_error(agent, "resume")),
            AgentKind::AgySession => agy::resume_with_executor(request, &self.agy_executor),
            AgentKind::RemoteMirror => Err(lifecycle_unavailable_error(agent, "resume")),
            AgentKind::VirtualInbox => Err(lifecycle_unavailable_error(agent, "resume")),
        }
    }
}

pub struct ProviderRouterSet<C, A> {
    codex_executor: C,
    agy_executor: A,
}

impl<C, A> ProviderRouterSet<C, A> {
    pub fn new(codex_executor: C, agy_executor: A) -> Self {
        Self {
            codex_executor,
            agy_executor,
        }
    }
}

impl<C: CodexSendExecutor, A: AgySendExecutor> ProviderRouter for ProviderRouterSet<C, A> {
    fn send(
        &self,
        agent: &Agent,
        request: ProviderSendRequest,
    ) -> Result<ProviderSendOutcome, CamError> {
        match agent.kind {
            AgentKind::Codex => codex::send_with_executor(request, &self.codex_executor),
            AgentKind::AgySession => agy::send_with_executor(request, &self.agy_executor),
            AgentKind::RemoteMirror => Err(CamError::ProviderUnavailable(
                "remote mirror delivery must be sent through the owning peer CAM".to_string(),
            )),
            AgentKind::VirtualInbox => Err(CamError::InvalidState(
                "virtual inbox delivery should not use provider adapters".to_string(),
            )),
        }
    }
}

impl<C: CodexLifecycleExecutor, A: AgyLifecycleExecutor> ProviderLifecycleRouter
    for ProviderRouterSet<C, A>
{
    fn check_readiness(
        &self,
        agent: &Agent,
        request: ProviderReadinessRequest,
    ) -> Result<ProviderReadinessOutcome, CamError> {
        match agent.kind {
            AgentKind::Codex => codex::check_readiness_with_executor(request, &self.codex_executor),
            AgentKind::AgySession => {
                agy::check_readiness_with_executor(request, &self.agy_executor)
            }
            AgentKind::RemoteMirror => Err(lifecycle_unavailable_error(agent, "readiness")),
            AgentKind::VirtualInbox => Err(lifecycle_unavailable_error(agent, "readiness")),
        }
    }

    fn resume(
        &self,
        agent: &Agent,
        request: ProviderResumeRequest,
    ) -> Result<ProviderResumeOutcome, CamError> {
        match agent.kind {
            AgentKind::Codex => codex::resume_with_executor(request, &self.codex_executor),
            AgentKind::AgySession => agy::resume_with_executor(request, &self.agy_executor),
            AgentKind::RemoteMirror => Err(lifecycle_unavailable_error(agent, "resume")),
            AgentKind::VirtualInbox => Err(lifecycle_unavailable_error(agent, "resume")),
        }
    }
}

pub type LiveCodexStdioProviderRouter =
    CodexProviderRouter<CodexStdioExecutor<CodexStdioProcessTransport>>;

pub fn live_codex_stdio_provider_router(
    program: Option<String>,
) -> Result<LiveCodexStdioProviderRouter, CamError> {
    let transport = match program {
        Some(program) => CodexStdioProcessTransport::with_program(program)?,
        None => CodexStdioProcessTransport::new(),
    };
    Ok(CodexProviderRouter::new(CodexStdioExecutor::new(transport)))
}

pub fn boxed_live_codex_stdio_provider_router(
    program: Option<String>,
) -> Result<Box<dyn ProviderRouter>, CamError> {
    Ok(Box::new(live_codex_stdio_provider_router(program)?))
}

pub fn boxed_live_codex_stdio_provider_lifecycle_router(
    program: Option<String>,
) -> Result<Box<dyn ProviderLifecycleRouter>, CamError> {
    Ok(Box::new(live_codex_stdio_provider_router(program)?))
}

impl ProviderSendRequest {
    pub fn from_delivery_decision(
        agent: &Agent,
        message: &Message,
        delivery_action: DeliveryAction,
        expected_delivery: DeliveryState,
    ) -> Self {
        Self {
            message_id: message.message_id.clone(),
            target_agent: agent.name.clone(),
            thread_id: agent.thread_id.clone(),
            active_turn_id: agent.active_turn_id.clone(),
            cwd: agent.cwd.clone(),
            source_agent: message.source_agent.clone(),
            source_node: message.source_node.clone(),
            correlation_id: message.correlation_id.clone(),
            message_type: message.message_type.clone(),
            body: message.body.clone(),
            strict: message.strict,
            delivery_action,
            expected_delivery,
        }
    }
}

impl ProviderReadinessRequest {
    pub fn from_agent(agent: &Agent) -> Result<Self, CamError> {
        ensure_provider_lifecycle_target(agent)?;
        let thread_id = agent.thread_id.clone().ok_or_else(|| {
            CamError::DeliveryFailed(
                "provider readiness request requires thread/session identity".to_string(),
            )
        })?;
        validate_provider_token("provider readiness request thread_id", &thread_id)?;
        Ok(Self {
            target_agent: agent.name.clone(),
            thread_id,
            active_turn_id: agent.active_turn_id.clone(),
            cwd: agent.cwd.clone(),
        })
    }
}

impl ProviderResumeRequest {
    pub fn from_agent(agent: &Agent) -> Result<Self, CamError> {
        ensure_provider_lifecycle_target(agent)?;
        let thread_id = agent.thread_id.clone().ok_or_else(|| {
            CamError::DeliveryFailed(
                "provider resume request requires thread/session identity".to_string(),
            )
        })?;
        validate_provider_token("provider resume request thread_id", &thread_id)?;
        Ok(Self {
            target_agent: agent.name.clone(),
            thread_id,
            active_turn_id: agent.active_turn_id.clone(),
            cwd: agent.cwd.clone(),
        })
    }
}

impl ProviderTranscriptRequest {
    pub fn from_agent(agent: &Agent, latest_only: bool) -> Result<Self, CamError> {
        ensure_provider_lifecycle_target(agent)?;
        let thread_id = agent.thread_id.clone().ok_or_else(|| {
            CamError::DeliveryFailed(
                "provider transcript request requires thread/session identity".to_string(),
            )
        })?;
        validate_provider_token("provider transcript request thread_id", &thread_id)?;
        Ok(Self {
            target_agent: agent.name.clone(),
            thread_id,
            latest_only,
        })
    }
}

pub fn validate_provider_send_outcome(
    request: &ProviderSendRequest,
    outcome: &ProviderSendOutcome,
) -> Result<(), CamError> {
    match outcome.delivery {
        DeliveryState::Steered | DeliveryState::Delivered => {}
        DeliveryState::Started
        | DeliveryState::Received
        | DeliveryState::Queued
        | DeliveryState::Failed => Err(CamError::DeliveryFailed(format!(
            "provider send outcome cannot claim outbound delivery state `{}`",
            format_provider_delivery(&outcome.delivery)
        )))?,
    }

    if outcome.delivery != request.expected_delivery {
        return Err(CamError::DeliveryFailed(format!(
            "provider send outcome `{}` does not match requested delivery `{}`",
            format_provider_delivery(&outcome.delivery),
            format_provider_delivery(&request.expected_delivery)
        )));
    }

    let turn_id = outcome.turn_id.as_deref().ok_or_else(|| {
        CamError::DeliveryFailed("provider send outcome is missing turn_id proof".to_string())
    })?;
    validate_provider_token("provider send outcome turn_id", turn_id)?;

    let outcome_message_id = outcome.message_id.as_deref().ok_or_else(|| {
        CamError::DeliveryFailed("provider send outcome is missing message_id proof".to_string())
    })?;
    if outcome_message_id != request.message_id {
        return Err(CamError::DeliveryFailed(format!(
            "provider send outcome echoed message_id `{outcome_message_id}` instead of `{}`",
            request.message_id
        )));
    }

    let outcome_target_agent = outcome.target_agent.as_deref().ok_or_else(|| {
        CamError::DeliveryFailed("provider send outcome is missing target_agent proof".to_string())
    })?;
    if outcome_target_agent != request.target_agent {
        return Err(CamError::DeliveryFailed(format!(
            "provider send outcome targeted `{outcome_target_agent}` instead of `{}`",
            request.target_agent
        )));
    }

    let request_thread_id = request.thread_id.as_deref().ok_or_else(|| {
        CamError::DeliveryFailed(
            "provider send request is missing thread/session identity".to_string(),
        )
    })?;
    let outcome_thread_id = outcome.thread_id.as_deref().ok_or_else(|| {
        CamError::DeliveryFailed("provider send outcome is missing thread_id proof".to_string())
    })?;
    if request_thread_id != outcome_thread_id {
        return Err(CamError::DeliveryFailed(format!(
            "provider send outcome attempted to remap thread `{request_thread_id}` to `{outcome_thread_id}`"
        )));
    }

    Ok(())
}

pub fn validate_provider_transcript_outcome(
    request: &ProviderTranscriptRequest,
    outcome: &ProviderTranscriptOutcome,
) -> Result<(), CamError> {
    validate_provider_lifecycle_identity(
        "transcript",
        &request.target_agent,
        &request.thread_id,
        &outcome.target_agent,
        &outcome.thread_id,
    )?;
    if outcome.transcript_mutated {
        return Err(CamError::DeliveryFailed(
            "provider transcript outcome must not mutate transcript content".to_string(),
        ));
    }
    if !outcome.provider_checked {
        return Err(CamError::DeliveryFailed(
            "provider transcript outcome did not prove provider was checked".to_string(),
        ));
    }
    if outcome.transcript_available && outcome.transcript_source.trim().is_empty() {
        return Err(CamError::DeliveryFailed(
            "provider transcript outcome is missing transcript_source".to_string(),
        ));
    }
    if !outcome.transcript_available && outcome.latest_turn_id.is_some() {
        return Err(CamError::DeliveryFailed(
            "provider transcript outcome cannot include latest_turn_id when transcript is unavailable"
                .to_string(),
        ));
    }
    if !outcome.transcript_available && outcome.summary.is_some() {
        return Err(CamError::DeliveryFailed(
            "provider transcript outcome cannot include summary when transcript is unavailable"
                .to_string(),
        ));
    }
    if outcome.transcript_source.trim() != outcome.transcript_source {
        return Err(CamError::DeliveryFailed(
            "provider transcript outcome transcript_source must not have leading or trailing whitespace"
                .to_string(),
        ));
    }
    validate_optional_provider_token(
        "provider transcript outcome latest_turn_id",
        outcome.latest_turn_id.as_deref(),
    )?;
    Ok(())
}

pub fn validate_provider_readiness_outcome(
    request: &ProviderReadinessRequest,
    outcome: &ProviderReadinessOutcome,
) -> Result<(), CamError> {
    validate_provider_lifecycle_identity(
        "readiness",
        &request.target_agent,
        &request.thread_id,
        &outcome.target_agent,
        &outcome.thread_id,
    )?;
    if outcome.transcript_mutated {
        return Err(CamError::DeliveryFailed(
            "provider readiness outcome must not mutate transcript content".to_string(),
        ));
    }
    if !outcome.provider_checked {
        return Err(CamError::DeliveryFailed(
            "provider readiness outcome did not prove provider was checked".to_string(),
        ));
    }
    if outcome.ready {
        validate_lifecycle_turn_proof(
            "provider readiness outcome",
            outcome.active_turn_id.as_deref(),
            outcome.last_turn_id.as_deref(),
        )?;
    }
    validate_optional_provider_token(
        "provider readiness outcome active_turn_id",
        outcome.active_turn_id.as_deref(),
    )?;
    validate_optional_provider_token(
        "provider readiness outcome last_turn_id",
        outcome.last_turn_id.as_deref(),
    )?;
    validate_optional_provider_token(
        "provider readiness outcome warning",
        outcome.warning.as_deref(),
    )?;
    Ok(())
}

pub fn validate_provider_resume_outcome(
    request: &ProviderResumeRequest,
    outcome: &ProviderResumeOutcome,
) -> Result<(), CamError> {
    validate_provider_lifecycle_identity(
        "resume",
        &request.target_agent,
        &request.thread_id,
        &outcome.target_agent,
        &outcome.thread_id,
    )?;
    if outcome.transcript_mutated {
        return Err(CamError::DeliveryFailed(
            "provider resume outcome must not mutate transcript content".to_string(),
        ));
    }
    if !outcome.provider_resumed {
        return Err(CamError::DeliveryFailed(
            "provider resume outcome did not prove provider was resumed".to_string(),
        ));
    }
    if !outcome.ready {
        return Err(CamError::DeliveryFailed(
            "provider resume outcome did not prove readiness".to_string(),
        ));
    }
    validate_optional_provider_token(
        "provider resume outcome active_turn_id",
        outcome.active_turn_id.as_deref(),
    )?;
    validate_optional_provider_token(
        "provider resume outcome last_turn_id",
        outcome.last_turn_id.as_deref(),
    )?;
    validate_optional_provider_token(
        "provider resume outcome warning",
        outcome.warning.as_deref(),
    )?;
    Ok(())
}

fn ensure_provider_lifecycle_target(agent: &Agent) -> Result<(), CamError> {
    match agent.kind {
        AgentKind::Codex | AgentKind::AgySession => Ok(()),
        AgentKind::VirtualInbox => Err(CamError::InvalidState(
            "virtual inbox readiness must not enter provider lifecycle adapters".to_string(),
        )),
        AgentKind::RemoteMirror => Err(CamError::InvalidState(
            "remote mirror readiness must be checked through the owning peer CAM".to_string(),
        )),
    }
}

fn validate_provider_lifecycle_identity(
    label: &str,
    expected_target_agent: &str,
    expected_thread_id: &str,
    actual_target_agent: &str,
    actual_thread_id: &str,
) -> Result<(), CamError> {
    validate_provider_token(
        &format!("provider {label} outcome target_agent"),
        actual_target_agent,
    )?;
    if actual_target_agent != expected_target_agent {
        return Err(CamError::DeliveryFailed(format!(
            "provider {label} outcome targeted `{actual_target_agent}` instead of `{expected_target_agent}`"
        )));
    }
    validate_provider_token(
        &format!("provider {label} outcome thread_id"),
        actual_thread_id,
    )?;
    if actual_thread_id != expected_thread_id {
        return Err(CamError::DeliveryFailed(format!(
            "provider {label} outcome attempted to remap thread `{expected_thread_id}` to `{actual_thread_id}`"
        )));
    }
    Ok(())
}

fn validate_lifecycle_turn_proof(
    label: &str,
    active_turn_id: Option<&str>,
    last_turn_id: Option<&str>,
) -> Result<(), CamError> {
    if active_turn_id.is_none() && last_turn_id.is_none() {
        return Err(CamError::DeliveryFailed(format!(
            "{label} is missing active_turn_id or last_turn_id proof"
        )));
    }
    Ok(())
}

fn validate_optional_provider_token(label: &str, value: Option<&str>) -> Result<(), CamError> {
    if let Some(value) = value {
        validate_provider_token(label, value)?;
    }
    Ok(())
}

fn transcript_unavailable_error(agent: &Agent) -> CamError {
    match agent.kind {
        AgentKind::Codex => CamError::ProviderUnavailable(
            "Codex transcript reading requires real app-server transcript evidence from the owning session".to_string(),
        ),
        AgentKind::AgySession => CamError::ProviderUnavailable(
            "AGY transcript reading requires a verified real Antigravity transcript primitive".to_string(),
        ),
        AgentKind::RemoteMirror => CamError::ProviderUnavailable(
            "remote mirror transcript must be read through the owning peer CAM".to_string(),
        ),
        AgentKind::VirtualInbox => CamError::InvalidState(
            "virtual inbox transcript should not use provider adapters".to_string(),
        ),
    }
}

fn lifecycle_unavailable_error(agent: &Agent, operation: &str) -> CamError {
    match agent.kind {
        AgentKind::Codex => CamError::ProviderUnavailable(format!(
            "Codex provider {operation} requires real app-server lifecycle evidence from the owning session"
        )),
        AgentKind::AgySession => CamError::ProviderUnavailable(format!(
            "AGY provider {operation} requires a verified real Antigravity primitive"
        )),
        AgentKind::RemoteMirror => CamError::ProviderUnavailable(
            "remote mirror readiness must be checked through the owning peer CAM".to_string(),
        ),
        AgentKind::VirtualInbox => CamError::InvalidState(
            "virtual inbox readiness should not use provider adapters".to_string(),
        ),
    }
}

fn validate_provider_token(label: &str, value: &str) -> Result<(), CamError> {
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

fn format_provider_delivery(delivery: &DeliveryState) -> &'static str {
    match delivery {
        DeliveryState::Started => "started",
        DeliveryState::Steered => "steered",
        DeliveryState::Delivered => "delivered",
        DeliveryState::Received => "received",
        DeliveryState::Queued => "queued",
        DeliveryState::Failed => "failed",
    }
}
