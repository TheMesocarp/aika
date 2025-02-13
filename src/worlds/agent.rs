use super::{Event, Mailbox};

/// An agent that can be run in a simulation.
pub trait Agent: Send {
    fn step(&mut self, state: &mut Option<Vec<u8>>, time: &u64, mailbox: &mut Mailbox) -> Event;

    fn get_state(&self) -> Option<&[u8]>;
}
