use crate::worlds::SimError;
use anyhow::Result;
use rayon::iter::{IntoParallelRefMutIterator, ParallelIterator};

use super::worlds::*;

/// A universe is a collection of worlds that can be run in parallel.
pub struct Universe<const SLOTS: usize, const HEIGHT: usize> {
    pub worlds: Vec<World<SLOTS, HEIGHT>>,
}

impl<const SLOTS: usize, const HEIGHT: usize> Universe<SLOTS, HEIGHT> {
    /// Create a new universe.
    pub fn new() -> Self {
        Universe { worlds: Vec::new() }
    }
    /// Add a world to the universe.
    pub fn add_world(&mut self, world: World<SLOTS, HEIGHT>) {
        self.worlds.push(world);
    }
    /// Run all worlds in the universe in parallel.
    pub async fn run_parallel(
        &mut self,
        _live: bool,
        _logs: bool,
        _mail: bool,
    ) -> Result<Vec<Result<(), SimError>>> {
        let mut handles = vec![];
        let worlds = std::mem::take(&mut self.worlds);
        for mut world in worlds {
            let handle = tokio::spawn(async move { world.run().await });
            handles.push(handle);
        }
        let results = futures::future::join_all(handles).await;
        let results = results
            .into_iter()
            .map(|result| match result {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(e),
                Err(e) => Err(SimError::TokioError(e.to_string())),
            })
            .collect();

        Ok(results)
    }
    /// pause all worlds in the universe
    pub fn pause_all(&mut self) -> Result<(), Vec<SimError>> {
        let errors: Vec<_> = self
            .worlds
            .par_iter_mut()
            .filter_map(|world| world.pause().err())
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
    /// resume all worlds in the universe
    pub fn resume_all(&mut self) -> Result<(), Vec<SimError>> {
        let errors: Vec<_> = self
            .worlds
            .par_iter_mut()
            .filter_map(|world| world.resume().err())
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}
