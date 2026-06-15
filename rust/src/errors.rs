use std::error::Error;
use std::fmt::{self, Display};

#[derive(Debug)]
pub enum CamError {
    Io(std::io::Error),
    Json(serde_json::Error),
    InvalidCommand(String),
    InvalidState(String),
    NotFound(String),
    DeliveryFailed(String),
    ProviderUnavailable(String),
    PeerTransportFailed(String),
    ProviderContractViolation(String),
    PeerProtocolViolation(String),
}

impl CamError {
    pub fn kind(&self) -> &'static str {
        match self {
            CamError::Io(_) => "io",
            CamError::Json(_) => "json",
            CamError::InvalidCommand(_) => "invalid_command",
            CamError::InvalidState(_) => "invalid_state",
            CamError::NotFound(_) => "not_found",
            CamError::DeliveryFailed(_) => "delivery_failed",
            CamError::ProviderUnavailable(_) => "provider_unavailable",
            CamError::PeerTransportFailed(_) => "peer_transport_failed",
            CamError::ProviderContractViolation(_) => "provider_contract_violation",
            CamError::PeerProtocolViolation(_) => "peer_protocol_violation",
        }
    }

    pub fn queue_fallback_allowed(&self) -> bool {
        matches!(
            self,
            CamError::DeliveryFailed(_)
                | CamError::ProviderUnavailable(_)
                | CamError::PeerTransportFailed(_)
        )
    }
}

impl Display for CamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CamError::Io(error) => write!(formatter, "I/O error: {error}"),
            CamError::Json(error) => write!(formatter, "JSON error: {error}"),
            CamError::InvalidCommand(message) => write!(formatter, "invalid command: {message}"),
            CamError::InvalidState(message) => write!(formatter, "invalid state: {message}"),
            CamError::NotFound(message) => write!(formatter, "not found: {message}"),
            CamError::DeliveryFailed(message) => write!(formatter, "delivery failed: {message}"),
            CamError::ProviderUnavailable(message) => {
                write!(formatter, "provider unavailable: {message}")
            }
            CamError::PeerTransportFailed(message) => {
                write!(formatter, "peer transport failed: {message}")
            }
            CamError::ProviderContractViolation(message) => {
                write!(formatter, "provider contract violation: {message}")
            }
            CamError::PeerProtocolViolation(message) => {
                write!(formatter, "peer protocol violation: {message}")
            }
        }
    }
}

impl Error for CamError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            CamError::Io(error) => Some(error),
            CamError::Json(error) => Some(error),
            CamError::InvalidCommand(_)
            | CamError::InvalidState(_)
            | CamError::NotFound(_)
            | CamError::DeliveryFailed(_)
            | CamError::ProviderUnavailable(_)
            | CamError::PeerTransportFailed(_)
            | CamError::ProviderContractViolation(_)
            | CamError::PeerProtocolViolation(_) => None,
        }
    }
}

impl From<std::io::Error> for CamError {
    fn from(error: std::io::Error) -> Self {
        CamError::Io(error)
    }
}

impl From<serde_json::Error> for CamError {
    fn from(error: serde_json::Error) -> Self {
        CamError::Json(error)
    }
}
