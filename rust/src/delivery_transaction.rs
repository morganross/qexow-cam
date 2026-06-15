use crate::core::{Agent, AgentKind, AgentStatus, DeliveryState, Message, Route, now_utc};
use crate::delivery::DeliveryDecision;
use crate::errors::CamError;
use crate::peers::{PeerMessageSendRequest, PeerMessageSender};
use crate::providers::{
    ProviderRouter, ProviderSendOutcome, ProviderSendRequest, validate_provider_send_outcome,
};
use crate::state::AppState;

pub fn execute_direct_delivery(
    state: &mut AppState,
    agent: &Agent,
    message: &mut Message,
    decision: &DeliveryDecision,
    provider_router: &dyn ProviderRouter,
    peer_message_sender: &dyn PeerMessageSender,
) -> Result<&'static str, DirectDeliveryError> {
    let outcome = if agent.kind == AgentKind::RemoteMirror {
        attempt_peer_mirror_delivery(state, agent, message, decision, peer_message_sender)
    } else {
        attempt_provider_delivery(agent, message, decision, provider_router)
    }
    .map_err(DirectDeliveryError::from)?;

    validate_direct_delivery_success_outcome(&outcome).map_err(DirectDeliveryError::Unqueueable)?;
    message.delivery = outcome.delivery;
    message.thread_id = outcome.thread_id.or(message.thread_id.clone());
    message.turn_id = outcome.turn_id;
    message.updated_at = now_utc();
    apply_provider_success_to_agent(state, &agent.name, message)
        .map_err(DirectDeliveryError::Terminal)?;
    message_success_event_type(&message.delivery).map_err(DirectDeliveryError::Terminal)
}

#[derive(Debug)]
pub enum DirectDeliveryError {
    Recoverable(CamError),
    Unqueueable(CamError),
    Terminal(CamError),
}

impl From<CamError> for DirectDeliveryError {
    fn from(error: CamError) -> Self {
        if error.queue_fallback_allowed() {
            DirectDeliveryError::Recoverable(error)
        } else if matches!(
            error,
            CamError::ProviderContractViolation(_) | CamError::PeerProtocolViolation(_)
        ) {
            DirectDeliveryError::Unqueueable(error)
        } else {
            DirectDeliveryError::Terminal(error)
        }
    }
}

fn attempt_provider_delivery(
    agent: &Agent,
    message: &Message,
    decision: &DeliveryDecision,
    provider_router: &dyn ProviderRouter,
) -> Result<ProviderSendOutcome, CamError> {
    let request = ProviderSendRequest::from_delivery_decision(
        agent,
        message,
        decision.action.clone(),
        decision.state.clone(),
    );

    let outcome = provider_router
        .send(agent, request.clone())
        .map_err(|error| contextualize_provider_send_error(&agent.name, error))?;
    validate_provider_send_outcome(&request, &outcome).map_err(|error| {
        CamError::ProviderContractViolation(format!(
            "provider send outcome violated direct delivery contract for `{}`: {error}",
            agent.name
        ))
    })?;
    Ok(outcome)
}

fn contextualize_provider_send_error(agent_name: &str, error: CamError) -> CamError {
    let message = format!("provider send failed for `{agent_name}`: {error}");
    match error {
        CamError::DeliveryFailed(_) | CamError::ProviderUnavailable(_) => {
            CamError::ProviderUnavailable(message)
        }
        CamError::ProviderContractViolation(_) => CamError::ProviderContractViolation(message),
        CamError::InvalidState(_) => CamError::InvalidState(message),
        CamError::InvalidCommand(_) => CamError::InvalidCommand(message),
        CamError::PeerProtocolViolation(_) => CamError::PeerProtocolViolation(message),
        CamError::PeerTransportFailed(_) => CamError::PeerTransportFailed(message),
        CamError::NotFound(_) => CamError::NotFound(message),
        CamError::Io(error) => CamError::Io(error),
        CamError::Json(error) => CamError::Json(error),
    }
}

fn attempt_peer_mirror_delivery(
    state: &AppState,
    agent: &Agent,
    message: &Message,
    decision: &DeliveryDecision,
    peer_message_sender: &dyn PeerMessageSender,
) -> Result<ProviderSendOutcome, CamError> {
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
        })?
        .to_string();
    let request = PeerMessageSendRequest {
        remote_agent_name,
        remote_thread_id: agent.thread_id.clone(),
        remote_cwd: agent.cwd.clone(),
        message: message.clone(),
        expected_delivery: decision.state.clone(),
        delivery_action: decision.action.clone(),
    };
    let outcome = peer_message_sender
        .send_message(peer, &request)
        .map_err(|error| {
            let remote_command_attempted = error.remote_command_attempted;
            let source = error.error;
            if matches!(source, CamError::PeerProtocolViolation(_)) {
                return source;
            }
            let stage = if remote_command_attempted {
                "after remote command"
            } else {
                "before remote command"
            };
            CamError::PeerTransportFailed(format!(
                "peer `{peer_name}` remote mirror send failed {stage}: {}",
                source
            ))
        })?;
    if outcome.error.is_some() {
        return Err(CamError::PeerProtocolViolation(
            "remote mirror send outcome returned success proof with error text".to_string(),
        ));
    }
    let stale_active_recovered_by_wake = decision.state == DeliveryState::Steered
        && decision.action == crate::delivery::DeliveryAction::SteerActiveTurn
        && outcome.delivery == DeliveryState::Delivered;
    if matches!(
        outcome.delivery,
        DeliveryState::Delivered | DeliveryState::Steered
    ) && outcome.delivery != decision.state
        && !stale_active_recovered_by_wake
    {
        return Err(CamError::PeerProtocolViolation(format!(
            "remote mirror send outcome `{}` does not match requested delivery `{}`",
            format_delivery(&outcome.delivery),
            format_delivery(&decision.state)
        )));
    }
    let actual_target_agent = outcome.target_agent.as_deref().ok_or_else(|| {
        CamError::PeerProtocolViolation(
            "remote mirror send outcome is missing target_agent proof".to_string(),
        )
    })?;
    if actual_target_agent != request.remote_agent_name {
        return Err(CamError::PeerProtocolViolation(format!(
            "remote mirror send outcome targeted `{actual_target_agent}` instead of `{}`",
            request.remote_agent_name
        )));
    }
    let receipt_nonce = outcome.receipt_nonce.as_deref().ok_or_else(|| {
        CamError::PeerProtocolViolation(
            "remote mirror send outcome is missing receipt_nonce proof".to_string(),
        )
    })?;
    if receipt_nonce != message.message_id {
        return Err(CamError::PeerProtocolViolation(format!(
            "remote mirror send outcome echoed receipt_nonce `{receipt_nonce}` instead of `{}`",
            message.message_id
        )));
    }
    let actual_thread_id = outcome.thread_id.as_deref().ok_or_else(|| {
        CamError::PeerProtocolViolation(
            "remote mirror send outcome is missing thread_id proof".to_string(),
        )
    })?;
    if let Some(expected_thread_id) = agent.thread_id.as_deref()
        && expected_thread_id != actual_thread_id
    {
        return Err(CamError::PeerProtocolViolation(format!(
            "remote mirror send outcome attempted to remap thread `{expected_thread_id}` to `{actual_thread_id}`"
        )));
    }
    Ok(ProviderSendOutcome {
        delivery: outcome.delivery,
        message_id: Some(message.message_id.clone()),
        target_agent: Some(agent.name.clone()),
        thread_id: outcome.thread_id,
        turn_id: outcome.turn_id,
    })
}

fn validate_direct_delivery_success_outcome(outcome: &ProviderSendOutcome) -> Result<(), CamError> {
    if !matches!(
        outcome.delivery,
        DeliveryState::Delivered | DeliveryState::Steered
    ) {
        return Err(CamError::DeliveryFailed(format!(
            "invalid direct success delivery state `{}`",
            format_delivery(&outcome.delivery)
        )));
    }
    let turn_id = outcome.turn_id.as_deref().ok_or_else(|| {
        CamError::DeliveryFailed("direct success delivery is missing turn_id proof".to_string())
    })?;
    validate_delivery_token("direct success turn_id", turn_id)
}

fn apply_provider_success_to_agent(
    state: &mut AppState,
    agent_name: &str,
    message: &Message,
) -> Result<(), CamError> {
    let agent = state.find_agent_mut(agent_name).ok_or_else(|| {
        CamError::InvalidState(format!(
            "provider delivery succeeded for missing agent `{agent_name}`"
        ))
    })?;
    if let Some(thread_id) = &message.thread_id {
        agent.thread_id = Some(thread_id.clone());
    }
    if let Some(turn_id) = &message.turn_id {
        agent.last_turn_id = Some(turn_id.clone());
        if message.delivery == DeliveryState::Delivered {
            agent.active_turn_id = None;
            agent.status = AgentStatus::Idle;
        } else if message.delivery == DeliveryState::Steered {
            agent.active_turn_id = Some(turn_id.clone());
            agent.status = AgentStatus::Active;
        }
        agent.updated_at = now_utc();
    }
    Ok(())
}

fn message_success_event_type(delivery: &DeliveryState) -> Result<&'static str, CamError> {
    match delivery {
        DeliveryState::Steered => Ok("message.steered"),
        DeliveryState::Delivered => Ok("message.delivered"),
        other => Err(CamError::InvalidState(format!(
            "direct provider success cannot log delivery state `{}`",
            format_delivery(other)
        ))),
    }
}

fn validate_delivery_token(label: &str, value: &str) -> Result<(), CamError> {
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

fn format_delivery(delivery: &DeliveryState) -> &'static str {
    match delivery {
        DeliveryState::Started => "started",
        DeliveryState::Steered => "steered",
        DeliveryState::Delivered => "delivered",
        DeliveryState::Received => "received",
        DeliveryState::Queued => "queued",
        DeliveryState::Failed => "failed",
    }
}
