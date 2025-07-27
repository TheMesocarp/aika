pub mod universe;

#[derive(Copy, Clone, Debug)]
pub struct Time {
    pub gvt: u64,
    pub cp_hz: u64,
    pub throttle: u64,
    pub terminal: f64,
    pub timestep: f64,
}