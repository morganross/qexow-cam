use crate::core::{Agent, AgentKind, AgentStatus, Route};
use crate::errors::CamError;
use crate::providers::{
    ProviderLifecycleRouter, ProviderReadinessOutcome, ProviderReadinessRequest,
    ProviderResumeOutcome, ProviderResumeRequest, validate_provider_readiness_outcome,
    validate_provider_resume_outcome,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentResumeResult {
    pub ok: bool,
    pub implemented: bool,
    pub agent: String,
    pub kind: String,
    pub route: String,
    pub thread_id: Option<String>,
    pub active_turn_id: Option<String>,
    pub last_turn_id: Option<String>,
    pub ready: bool,
    pub provider_checked: bool,
    pub provider_resumed: bool,
    pub readiness_ready: Option<bool>,
    pub readiness_error: Option<String>,
    pub resume_attempted: bool,
    pub message: String,
    pub warning: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug)]
pub struct LifecycleAttemptError {
    pub error: CamError,
    pub provider_checked: bool,
    pub readiness_ready: Option<bool>,
    pub readiness_error: Option<String>,
    pub resume_attempted: bool,
}

impl From<CamError> for LifecycleAttemptError {
    fn from(error: CamError) -> Self {
        Self {
            error,
            provider_checked: false,
            readiness_ready: None,
            readiness_error: None,
            resume_attempted: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeAttemptContext {
    pub provider_checked: bool,
    pub readiness_ready: Option<bool>,
    pub readiness_error: Option<String>,
}

pub enum ResumeReadinessDecision {
    Ready(AgentResumeResult),
    AttemptResume(ResumeAttemptContext),
    Blocked(LifecycleAttemptError),
}

impl AgentResumeResult {
    pub fn for_agent(agent: &Agent) -> Self {
        match agent.kind {
            AgentKind::VirtualInbox => Self {
                ok: true,
                implemented: true,
                agent: agent.name.clone(),
                kind: format_kind(&agent.kind).to_string(),
                route: format_route(&agent.route),
                thread_id: agent.thread_id.clone(),
                active_turn_id: agent.active_turn_id.clone(),
                last_turn_id: agent.last_turn_id.clone(),
                ready: true,
                provider_checked: false,
                provider_resumed: false,
                readiness_ready: None,
                readiness_error: None,
                resume_attempted: false,
                message: "virtual inbox is ready; no provider resume is required".to_string(),
                warning: None,
                error: None,
            },
            AgentKind::Codex | AgentKind::AgySession
                if agent.status == AgentStatus::Active && agent.active_turn_id.is_some() =>
            {
                Self {
                    ok: true,
                    implemented: false,
                    agent: agent.name.clone(),
                    kind: format_kind(&agent.kind).to_string(),
                    route: format_route(&agent.route),
                    thread_id: agent.thread_id.clone(),
                    active_turn_id: agent.active_turn_id.clone(),
                    last_turn_id: agent.last_turn_id.clone(),
                    ready: true,
                    provider_checked: false,
                    provider_resumed: false,
                    readiness_ready: None,
                    readiness_error: None,
                    resume_attempted: false,
                    message: "agent already has an active turn; no new session was created"
                        .to_string(),
                    warning: Some(
                        "provider resume is unavailable, but the agent already has an active turn"
                            .to_string(),
                    ),
                    error: None,
                }
            }
            AgentKind::Codex | AgentKind::AgySession
                if agent.status == AgentStatus::Active && agent.thread_id.is_some() =>
            {
                Self {
                    ok: false,
                    implemented: false,
                    agent: agent.name.clone(),
                    kind: format_kind(&agent.kind).to_string(),
                    route: format_route(&agent.route),
                    thread_id: agent.thread_id.clone(),
                    active_turn_id: agent.active_turn_id.clone(),
                    last_turn_id: agent.last_turn_id.clone(),
                    ready: false,
                    provider_checked: false,
                    provider_resumed: false,
                    readiness_ready: None,
                    readiness_error: None,
                    resume_attempted: false,
                    message:
                        "agent is marked active but has no active turn proof; provider resume is required"
                            .to_string(),
                    warning: Some(
                        "active status without active_turn_id is not treated as attention proof"
                            .to_string(),
                    ),
                    error: Some("missing active turn proof".to_string()),
                }
            }
            AgentKind::Codex | AgentKind::AgySession if agent.thread_id.is_none() => Self {
                ok: false,
                implemented: false,
                agent: agent.name.clone(),
                kind: format_kind(&agent.kind).to_string(),
                route: format_route(&agent.route),
                thread_id: None,
                active_turn_id: agent.active_turn_id.clone(),
                last_turn_id: agent.last_turn_id.clone(),
                ready: false,
                provider_checked: false,
                provider_resumed: false,
                readiness_ready: None,
                readiness_error: None,
                resume_attempted: false,
                message: "agent has no thread/session id to resume; identity was preserved"
                    .to_string(),
                warning: None,
                error: Some("missing thread/session id".to_string()),
            },
            AgentKind::Codex | AgentKind::AgySession => Self {
                ok: false,
                implemented: false,
                agent: agent.name.clone(),
                kind: format_kind(&agent.kind).to_string(),
                route: format_route(&agent.route),
                thread_id: agent.thread_id.clone(),
                active_turn_id: agent.active_turn_id.clone(),
                last_turn_id: agent.last_turn_id.clone(),
                ready: false,
                provider_checked: false,
                provider_resumed: false,
                readiness_ready: None,
                readiness_error: None,
                resume_attempted: false,
                message:
                    "agent resume is recognized, but provider resume is unavailable from a real provider primitive"
                        .to_string(),
                warning: Some(format!(
                    "{} resume/wake is unavailable from a real provider primitive; identity was preserved and no new session was created",
                    format_kind(&agent.kind)
                )),
                error: Some("resume unavailable until provider adapter is implemented".to_string()),
            },
            AgentKind::RemoteMirror => Self {
                ok: false,
                implemented: false,
                agent: agent.name.clone(),
                kind: format_kind(&agent.kind).to_string(),
                route: format_route(&agent.route),
                thread_id: agent.thread_id.clone(),
                active_turn_id: agent.active_turn_id.clone(),
                last_turn_id: agent.last_turn_id.clone(),
                ready: false,
                provider_checked: false,
                provider_resumed: false,
                readiness_ready: None,
                readiness_error: None,
                resume_attempted: false,
                message: "remote mirror agents must be resumed through their owning peer CAM"
                    .to_string(),
                warning: None,
                error: Some(
                    "remote mirror agents must be resumed through their owning peer CAM".to_string(),
                ),
            },
        }
    }

    pub fn event_type(&self) -> &'static str {
        if self.kind == "virtual_inbox" {
            "agent.resume.noop"
        } else if self.ok {
            "agent.resume.ready"
        } else if self.kind == "remote_mirror" || self.thread_id.is_none() {
            "agent.resume.rejected"
        } else {
            "agent.resume.blocked"
        }
    }
}

pub fn decide_resume_after_readiness(
    readiness_result: Result<AgentResumeResult, CamError>,
) -> ResumeReadinessDecision {
    match readiness_result {
        Ok(result) if result.ready => ResumeReadinessDecision::Ready(result),
        Ok(readiness) => ResumeReadinessDecision::AttemptResume(ResumeAttemptContext {
            provider_checked: readiness.provider_checked,
            readiness_ready: readiness.readiness_ready,
            readiness_error: None,
        }),
        Err(error) if is_lifecycle_unavailable_error(&error) => {
            ResumeReadinessDecision::AttemptResume(ResumeAttemptContext {
                provider_checked: false,
                readiness_ready: None,
                readiness_error: Some(error.to_string()),
            })
        }
        Err(error) => ResumeReadinessDecision::Blocked(LifecycleAttemptError {
            readiness_error: Some(error.to_string()),
            error,
            provider_checked: false,
            readiness_ready: None,
            resume_attempted: false,
        }),
    }
}

pub fn apply_resume_attempt_result(
    resume_result: Result<AgentResumeResult, CamError>,
    context: ResumeAttemptContext,
) -> Result<AgentResumeResult, LifecycleAttemptError> {
    let mut result = resume_result.map_err(|error| LifecycleAttemptError {
        error,
        provider_checked: context.provider_checked,
        readiness_ready: context.readiness_ready,
        readiness_error: context.readiness_error.clone(),
        resume_attempted: true,
    })?;
    result.provider_checked = context.provider_checked;
    result.readiness_ready = context.readiness_ready;
    result.readiness_error = context.readiness_error;
    result.resume_attempted = true;
    Ok(result)
}

pub fn resume_with_readiness_check(
    agent: &Agent,
    provider_lifecycle_router: &dyn ProviderLifecycleRouter,
) -> Result<AgentResumeResult, LifecycleAttemptError> {
    let readiness_result = ProviderReadinessRequest::from_agent(agent)
        .and_then(|request| provider_lifecycle_router.check_readiness(agent, request))
        .and_then(|outcome| apply_provider_readiness_outcome(agent, outcome));

    match decide_resume_after_readiness(readiness_result) {
        ResumeReadinessDecision::Ready(result) => Ok(result),
        ResumeReadinessDecision::AttemptResume(context) => {
            let resume_result = ProviderResumeRequest::from_agent(agent)
                .and_then(|request| provider_lifecycle_router.resume(agent, request))
                .and_then(|outcome| apply_provider_resume_outcome(agent, outcome));
            apply_resume_attempt_result(resume_result, context)
        }
        ResumeReadinessDecision::Blocked(error) => Err(error),
    }
}

pub fn should_attempt_provider_resume(agent: &Agent) -> bool {
    matches!(agent.kind, AgentKind::Codex | AgentKind::AgySession)
        && matches!(agent.route, Route::Local)
        && (agent.status != AgentStatus::Active || agent.active_turn_id.is_none())
        && agent.thread_id.is_some()
}

pub fn result_from_lifecycle_error(
    agent: &Agent,
    error: LifecycleAttemptError,
) -> AgentResumeResult {
    let mut result = AgentResumeResult::for_agent(agent);
    result.provider_checked = error.provider_checked;
    result.readiness_ready = error.readiness_ready;
    result.readiness_error = error.readiness_error;
    result.resume_attempted = error.resume_attempted;
    result.error = Some(error.error.to_string());
    result
}

pub fn apply_provider_readiness_outcome(
    agent: &Agent,
    outcome: ProviderReadinessOutcome,
) -> Result<AgentResumeResult, CamError> {
    let request = ProviderReadinessRequest::from_agent(agent)?;
    validate_provider_readiness_outcome(&request, &outcome)?;
    Ok(AgentResumeResult {
        ok: outcome.ready,
        implemented: true,
        agent: agent.name.clone(),
        kind: format_kind(&agent.kind).to_string(),
        route: format_route(&agent.route),
        thread_id: Some(outcome.thread_id),
        active_turn_id: outcome.active_turn_id,
        last_turn_id: outcome.last_turn_id,
        ready: outcome.ready,
        provider_checked: outcome.provider_checked,
        provider_resumed: false,
        readiness_ready: Some(outcome.ready),
        readiness_error: None,
        resume_attempted: false,
        message: "provider readiness checked without transcript mutation".to_string(),
        warning: outcome.warning,
        error: None,
    })
}

pub fn apply_provider_resume_outcome(
    agent: &Agent,
    outcome: ProviderResumeOutcome,
) -> Result<AgentResumeResult, CamError> {
    let request = ProviderResumeRequest::from_agent(agent)?;
    validate_provider_resume_outcome(&request, &outcome)?;
    Ok(AgentResumeResult {
        ok: outcome.ready,
        implemented: true,
        agent: agent.name.clone(),
        kind: format_kind(&agent.kind).to_string(),
        route: format_route(&agent.route),
        thread_id: Some(outcome.thread_id),
        active_turn_id: outcome.active_turn_id,
        last_turn_id: outcome.last_turn_id,
        ready: outcome.ready,
        provider_checked: true,
        provider_resumed: outcome.provider_resumed,
        readiness_ready: None,
        readiness_error: None,
        resume_attempted: true,
        message: "provider resume proved readiness without transcript mutation".to_string(),
        warning: outcome.warning,
        error: None,
    })
}

fn is_lifecycle_unavailable_error(error: &CamError) -> bool {
    matches!(error, CamError::ProviderUnavailable(message) if message.contains("provider") && message.contains("not available"))
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
