use crate::logger::Lumi;

use super::{Event, Mailbox};

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