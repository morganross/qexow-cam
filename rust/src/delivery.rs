use crate::core::{Agent, AgentKind, AgentStatus, DeliveryState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryDecision {
    pub state: DeliveryState,
    pub action: DeliveryAction,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryAction {
    SteerActiveTurn,
    WakeKnownSession,
    StoreInVirtualInbox,
    QueueFallback,
    FailLoudly,
}

pub fn decide_delivery(target: Option<&Agent>, strict: bool) -> DeliveryDecision {
    let Some(agent) = target else {
        if strict {
            return DeliveryDecision::failed("unknown target and strict delivery is enabled");
        }
        return DeliveryDecision::queued("unknown target; queued fallback is the only safe path");
    };

    if agent.kind == AgentKind::VirtualInbox {
        return DeliveryDecision {
            state: DeliveryState::Received,
            action: DeliveryAction::StoreInVirtualInbox,
            reason: "target is an explicit virtual inbox".to_string(),
        };
    }

    if agent.status == AgentStatus::Active {
        if agent.active_turn_id.is_some() {
            if agent.thread_id.is_none() {
                return DeliveryDecision::failed(
                    "target has active turn proof but no session/thread identity",
                );
            }
            return DeliveryDecision {
                state: DeliveryState::Steered,
                action: DeliveryAction::SteerActiveTurn,
                reason: "target has active attention and an active turn".to_string(),
            };
        }
        return DeliveryDecision::failed(
            "target claims active attention but has no active turn proof",
        );
    }

    if agent.thread_id.is_some() {
        return DeliveryDecision {
            state: DeliveryState::Delivered,
            action: DeliveryAction::WakeKnownSession,
            reason: "target is inactive but has a known session/thread identity".to_string(),
        };
    }

    if agent.kind == AgentKind::RemoteMirror {
        return DeliveryDecision {
            state: DeliveryState::Delivered,
            action: DeliveryAction::WakeKnownSession,
            reason:
                "target is a remote mirror; owning peer can create or wake the provider session"
                    .to_string(),
        };
    }

    if strict {
        return DeliveryDecision::failed(
            "target has no deliverable session/thread and strict is enabled",
        );
    }

    DeliveryDecision::queued("target is not directly deliverable; queued fallback is visible")
}

impl DeliveryDecision {
    fn failed(reason: &str) -> Self {
        Self {
            state: DeliveryState::Failed,
            action: DeliveryAction::FailLoudly,
            reason: reason.to_string(),
        }
    }

    fn queued(reason: &str) -> Self {
        Self {
            state: DeliveryState::Queued,
            action: DeliveryAction::QueueFallback,
            reason: reason.to_string(),
        }
    }
}
