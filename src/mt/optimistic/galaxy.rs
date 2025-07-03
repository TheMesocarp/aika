use bytemuck::{Pod, Zeroable};

use crate::mt::optimistic::planet::Planet;

pub struct Galaxy<
    const INTER_SLOTS: usize,
    const CLOCK_SLOTS: usize,
    const CLOCK_HEIGHT: usize,
    MessageType: Pod + Zeroable + Clone,
> {
    pub planets: Vec<Planet<INTER_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>>,
}
