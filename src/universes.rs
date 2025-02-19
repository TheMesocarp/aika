use crate::worlds::SimError;
use rayon::iter::{IntoParallelRefMutIterator, ParallelIterator};

use super::worlds::*;

/// A universe is a collection of worlds that can be run in parallel.
pub struct Universe<const LOGS: usize, const SLOTS: usize, const HEIGHT: usize> {
    pub worlds: Vec<World<LOGS, SLOTS, HEIGHT>>,
}

impl<const LOGS: usize, const SLOTS: usize, const HEIGHT: usize> Universe<LOGS, SLOTS, HEIGHT> {
    /// Create a new universe.
    pub fn new() -> Self {
        Universe { worlds: Vec::new() }
    }

    /// Add a world to the universe.
    pub fn add_world(&mut self, world: World<LOGS, SLOTS, HEIGHT>) {
        self.worlds.push(world);
    }

    /// Run all worlds in the universe in parallel.
    pub fn run_parallel(&mut self) -> Vec<Result<(), SimError>> {
        self.worlds
            .par_iter_mut()
            .map(|world| world.run())
            .collect()
    }
}
