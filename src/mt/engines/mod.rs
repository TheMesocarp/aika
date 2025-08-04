//! `aika::mt::engines` contains all the core multi-threaded parallel simulation engines and their infrastructure. Currently,
//!  there is only one engine, `hlocal`, however there are plans for both `hdist` and `clocal`.
pub mod hlocal;

/// Time information for hybrid model simulations.
#[derive(Copy, Clone, Debug)]
pub struct HTime {
    /// Current GVT value.
    pub gvt: u64,
    /// Checkpoint frequency in block count.
    pub cp_hz: u64,
    /// Throttle window size in block count.
    pub throttle: u64,
    /// Latest terminal time of the simulation.
    pub terminal: u64,
}
