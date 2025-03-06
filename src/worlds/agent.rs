use crate::logger::Lumi;

use super::{Event, Mailbox};

/// Supports for an agent in a single threaded world to communicate with the world and other agents.
pub enum Supports<'a> {
    Mailbox(&'a mut Mailbox),
    Logger(&'a mut Lumi),
    None,
    Both(&'a mut Mailbox, &'a mut Lumi),
}

/// An agent that can be run in a simulation.
pub trait Agent: Send {
    fn step(&mut self, time: &u64, supports: Supports) -> Event;
}
