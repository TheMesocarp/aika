use mesocarp::MesoError;

/// Error enum for provide feedback on simulation errors
#[derive(Debug)]
pub enum SimError {
    TimeTravel,
    PastTerminal,
    MesoError(MesoError)
}
