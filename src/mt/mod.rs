//! Multi-threaded simulation execution with support for optimistic and conservative synchronization.
//! Currently implements hybrid synchronization based on Clustered Time Warp architecture for
//! parallel discrete event simulation across multiple threads.
pub mod hybrid;
