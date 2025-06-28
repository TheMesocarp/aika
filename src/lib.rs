use mesocarp::MesoError;

pub mod st;
pub mod mt;
pub mod agents;
pub mod messages;

/// Error enum for provide feedback on simulation errors
#[derive(Debug)]
pub enum SimError {
    TimeTravel,
    PastTerminal,
    MesoError(MesoError)
}