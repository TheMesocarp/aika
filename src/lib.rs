use mesocarp::MesoError;

pub mod agents;
pub mod event;
pub mod messages;
pub mod mt;
pub mod st;

/// Error enum for provide feedback on simulation errors
#[derive(Debug)]
pub enum SimError {
    TimeTravel,
    PastTerminal,
    MaximumAgentsAllowed,
    NotAllAgentsRegistered,
    ThreadPanic,
    MesoError(MesoError),
}
