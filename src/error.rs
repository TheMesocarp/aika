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
    LPsFull,
    MismatchLPsCount,
    NoState,
    NoEvents,
    NoClock,
    PollError,
    ThreadJoinError,
    InvalidIndex,
    NotRealtime,
    TokioError(String),
    Mesocarp(String),
}
