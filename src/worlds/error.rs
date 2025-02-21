/// Error enum for provide feedback on simulation errors
#[derive(Debug, Clone)]
pub enum SimError {
    TimeTravel,
    PastTerminal,
    ScheduleFailed,
    PlaybackFroze,
    MailboxFull,
    MailboxEmpty,
    RollbackTimeMismatch,
    NoState,
    NoEvents,
    NoClock,
    InvalidIndex,
    NotRealtime,
    TokioError(String),
}
