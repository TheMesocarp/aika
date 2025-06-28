use mesocarp::MesoError;

pub mod agents;
pub mod messages;
pub mod mt;
pub mod st;

/// Error enum for provide feedback on simulation errors
#[derive(Debug)]
pub enum SimError {
    TimeTravel,
    PastTerminal,
    MesoError(MesoError),
}
