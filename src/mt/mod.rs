//! Multi-threaded simulation execution with support for optimistic and conservative synchronization.
//! Currently implements hybrid synchronization based on Clustered Time Warp architecture for
//! parallel discrete event simulation across multiple threads.

pub mod engines;
pub mod consensus;
pub(crate) mod logging;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum RunMode {
    Fast,
    Debug,
}
