//! # Aika
//! 
//! A Rust-native coordination layer for multi-agent systems supporting single-threaded and
//! multi-threaded execution. Built on discrete event simulation principles from the 1980s-90s.
//!
//! ## Quick Start
//!
//! ### Single-threaded simulation
//! ```rust,no_run
//! use aika::st::World;
//! use aika::agents::Agent;
//! 
//! let mut world = World::<8, 128, 1, u8>::init(1000.0, 1.0, 0)?;
//! let agent = MyAgent::new();
//! world.spawn_agent(Box::new(agent));
//! world.init_support_layers(None)?;
//! world.schedule(1, 0)?;
//! world.run()?;
//! # Ok::<(), aika::SimError>(())
//! ```
//!
//! ### Multi-threaded hybrid simulation
//! ```rust,no_run
//! use aika::mt::hybrid::{HybridEngine, config::HybridConfig};
//! 
//! let config = HybridConfig::new(4, 512)
//!     .with_time_bounds(1000.0, 1.0)
//!     .with_optimistic_sync(50, 100)
//!     .with_uniform_worlds(1024, 10, 256);
//!     
//! let mut engine = HybridEngine::<128, 128, 2, MyMessageType>::create(config)?;
//! // spawn agents and schedule initial events...
//! let result = engine.run();
//! ```
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
    pub use crate::AikaError;
    pub use crate::agents::{Agent, ThreadedAgent, WorldContext, PlanetContext, AgentSupport};
    pub use crate::objects::{Action, Event, Msg, AntiMsg};
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
}
