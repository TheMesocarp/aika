//! # Aika
//!
//! A Rust-native coordination layer for multi-agent systems supporting single-threaded and
//! multi-threaded execution. Built on discrete event simulation principles from the 1980s-90s.
//!
//! ## Architecture
//!
//! - [`st`] - Single-threaded discrete event simulator.
//! - [`mt`] - Multi-threaded discrete event simulators. Currently a hybrid synchronization model is under construction.
//! - [`objects`] - Internal simulation objects, like `Event` or `Msg`.
//! - [`actors`] - Actor traits and context.
use mesocarp::MesoError;
use thiserror::Error;

use crate::mt::consensus::ComputeLayout;

pub mod actors;
pub mod env;
pub mod mt;
pub mod objects;
pub mod st;

pub mod prelude {
    pub use crate::objects::{Event, Msg, SchedulingTask};
    pub use crate::AikaError;
    pub use bytemuck::{Pod, Zeroable};
}

/// Error enum for provide feedback on simulation errors
#[derive(Debug, Error, PartialEq)]
pub enum AikaError {
    #[error(
        "Attempted to process an event whos execution timestamp doesn't match simulation time."
    )]
    TimeTravel,
    #[error("Terminal time stamp hit, no more scheduling allowed.")]
    PastTerminal,
    #[error(
        "Terminal time stamp hit, no more scheduling allowed, though messages are still in transit"
    )]
    PastTerminalButGVTBehind,
    #[error("Maximum number of clusters already specified. If you want to add more clusters, you need to change your configuration.")]
    MaximumClustersAllowed,
    #[error("Cannot start parallel simulation, not all specified clusters have been configured or provided.")]
    NotAllClustersRegistered,
    #[error("Thread panicked!")]
    ThreadPanic,
    #[error("Mail delivered to the wrong address, fire the mail man.")]
    MismatchedDeliveryAddress(usize, usize, usize),
    #[error("Error found when utilizing `mesocarp`: {0}.")]
    MesoError(#[from] MesoError),
    #[error("Local clocks on a `Planet` were out of sync.")]
    ClockSyncIssue,
    #[error("Invalid cluster ID! Only {0} clusters, but ID provided is {1}.")]
    InvalidClusterId(usize, usize),
    #[error("Invalid actor ID! Only {0} actors, on cluster {1} but ID provided is {2}.")]
    InvalidActorId(usize, usize, usize),
    #[error("Planet {0} Time {1}: Rolled back past the GVT safe point at {2}. GVT is moving ahead too fast!")]
    GVTPastLocalClock(usize, u64, u64),
    #[error("GVT is backtracking. Submitted blocks are decrementing in time.")]
    GVTisDecreasing,
    #[error("Must set a terminal time or simulation will never terminate. Try calling `Galaxy::set_time_scale(terminal, timestep)` before spawning clusters.")]
    MustSetTerminalTime,
    #[error("Must set a block duration for multi-threaded hybrid simulations. Try calling `Galaxy::with_block_duration(dur)` before spawning clusters.")]
    MustSetBlockDuration,
    #[error("Attempt to message an actor that doesnt have messaging implemented")]
    MessagedNonReceiver,
    #[error("Attempt to message an actor at an ID that doesnt exist! max ID: {0}, attempted: {1}")]
    MessagedNonExistent(usize, usize),
    #[error("Stager must be configured before spawning execution clusters.")]
    UnconfiguredStager,
    #[error("No actors have been added to cluster {0}! Aika will not start a wasteful sim.")]
    NoActors(usize),
    #[error("Failed to initialize the logging files for set up.")]
    LoggingSetupFailure,
    #[error("Write to log failed.")]
    LoggingWriteError,
    #[error("Attempted to register a compute producer that expected one layout, but another was found: {0}")]
    ComputeLayoutExpectationMismatch(ComputeLayout),
    #[error("Received a message from too distant in the past, GVT safe point is no longer safe.")]
    DistantBlocks(usize),
    #[error("Producers submitting different duration blocks.")]
    MismatchBlockRanges,
}
