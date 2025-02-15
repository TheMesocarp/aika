use std::ffi::c_void;

use crate::logger::History;

use super::{Event, Mailbox};

pub enum Supports<'a> {
    Mailbox(&'a mut Mailbox),
    Logger(&'a mut History),
    None,
    Both(&'a mut Mailbox, &'a mut History),
}

/// An agent that can be run in a simulation.
pub trait Agent: Send {
    fn step(&mut self, state: &mut Option<*mut c_void>, time: &u64, supports: Supports) -> Event;
}