use crate::worlds::Event;
use std::collections::{BTreeMap, BinaryHeap};

mod snapshot;

pub use snapshot::Snapshot;

/// A logger for recording snapshots of the world.
pub struct Logger {
    snapshots: BinaryHeap<Snapshot>,
    events: BinaryHeap<Event>,
}

impl Logger {
    /// Create a new logger.
    pub fn new() -> Self {
        Logger {
            snapshots: BinaryHeap::new(),
            events: BinaryHeap::new(),
        }
    }
    /// Log a snapshot of the world.
    pub fn log(
        &mut self,
        timestamp: f64,
        shared_state: Option<Vec<u8>>,
        agent_states: BTreeMap<usize, Vec<u8>>,
        event: Event,
    ) {
        self.snapshots.push(Snapshot {
            timestamp,
            shared_state,
            agent_states,
        });
        self.events.push(event);
    }
    /// Get the events logged.
    pub fn get_snapshots(&self) -> BinaryHeap<Snapshot> {
        self.snapshots.clone()
    }
}
