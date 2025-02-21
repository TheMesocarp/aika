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
    ClockSubmissionFailed,
    NoState,
    NoEvents,
    NoClock,
    InvalidIndex,
    NotRealtime,
    TokioError(String),
}
