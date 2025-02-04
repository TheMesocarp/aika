use futures::future::BoxFuture;

use super::{Event, Mailbox};

/// An agent that can be run in a simulation.
pub trait Agent: Send {
    fn step<'a>(
        &mut self,
        state: &mut Option<&[u8]>,
        time: &f64,
        mailbox: &mut Option<Mailbox<'a>>,
    ) -> BoxFuture<'a, Event>;
    fn get_state(&self) -> Option<&[u8]>;
}
