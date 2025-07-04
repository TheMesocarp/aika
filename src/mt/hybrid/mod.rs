// here we'll do our custom time warp variant built around `Planet`

use bytemuck::{Pod, Zeroable};

use crate::mt::hybrid::{config::HybridConfig, galaxy::Galaxy, planet::Planet};

pub mod galaxy;
pub mod planet;
pub mod config;

pub struct HybridEngine<
    const INTER_SLOTS: usize,
    const CLOCK_SLOTS: usize,
    const CLOCK_HEIGHT: usize,
    MessageType: Pod + Zeroable + Clone,
> {
    pub galaxy: Galaxy<INTER_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>,
    pub planets: Vec<Planet<INTER_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>>,
    pub config: HybridConfig
}