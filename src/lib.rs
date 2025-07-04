use mesocarp::MesoError;
use thiserror::Error;

pub mod agents;
pub mod event;
pub mod messages;
pub mod mt;
pub mod st;

/// Error enum for provide feedback on simulation errors
#[derive(Debug, Error)]
pub enum SimError {
    #[error(
        "Attempted to process an event whos execution timestamp doesn't match simulation time."
    )]
    TimeTravel,
    #[error("Terminal time stamp hit, no more scheduling allowed.")]
    PastTerminal,
    #[error("Maximum number of agents already specified. If you want to add more agents, you need to configure the GVT to support more.")]
    MaximumAgentsAllowed,
    #[error("Cannot start parallel simulation, not all specified agents have been configured or provided.")]
    NotAllAgentsRegistered,
    #[error("Thread panicked!")]
    ThreadPanic,
    #[error("Mail delivered to the wrong address, fire the mail man.")]
    MismatchedDeliveryAddress,
    #[error("Error found when utilizing `mesocarp`: {0}.")]
    MesoError(#[from] MesoError),
    #[error("Local clocks on a `Planet` were out of sync.")]
    ClockSyncIssue,
    #[error("Invalid world ID: {0}")]
    InvalidWorldId(usize),
    #[error("Configuration error: {0}")]
    ConfigError(String),
}
