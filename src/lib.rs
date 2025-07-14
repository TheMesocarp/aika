//! # Aika
//!
//! A Rust-native coordination layer for multi-agent systems supporting single-threaded and
//! multi-threaded execution. Built on discrete event simulation principles from the 1980s-90s.
//!
//! ## Architecture
//!
//! - [`st`] - Single-threaded discrete event simulation
//! - [`mt::hybrid`] - Multi-threaded optimistic synchronization
//! - [`agents`] - Agent traits and execution contexts
//! - [`objects`] - Core simulation data structures

use mesocarp::MesoError;
use thiserror::Error;

pub mod agents;
pub mod mt;
pub mod objects;
pub mod st;

pub mod prelude {
    pub use crate::agents::{Agent, AgentSupport, PlanetContext, ThreadedAgent, WorldContext};
    pub use crate::objects::{Action, AntiMsg, Event, Msg};
    pub use crate::AikaError;
    pub use bytemuck::{Pod, Zeroable};
}

/// Error enum for provide feedback on simulation errors
#[derive(Debug, Error)]
pub enum AikaError {
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
    #[error("current processor is receiving messages from {0} blocks; too far in the past! Messaging is lagging somewhere")]
    DistantBlocks(usize),
    #[error("Mismatched block sizes for block number {0}")]
    MismatchBlockSizes(usize),
}
