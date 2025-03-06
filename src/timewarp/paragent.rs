use crate::{
    logger::Lumi,
    worlds::{Event, Message},
};

use super::antimessage::Annihilator;

/// Wrapper for the output of the message processing function. Allows a message to trigger both local events and response messages
pub enum HandlerOutput {
    Messages(Annihilator),
    Event(Event),
    Nan,
}

/// LP trait for parallel agents. These are for fully isolated processes, communications are implemented with `process_message`
pub trait LogicalProcess: Send {
    fn step(&mut self, time: &u64, state: &mut Lumi) -> Event;
    fn process_message(&mut self, msg: Message, time: u64, state: &mut Lumi) -> HandlerOutput;
}
