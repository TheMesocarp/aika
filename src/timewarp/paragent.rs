use crate::{
    logger::Lumi,
    worlds::{Event, Message},
};

use super::antimessage::Annihilator;

pub enum HandlerOutput {
    Messages(Annihilator),
    Event(Event),
    Nan,
}

pub trait LogicalProcess {
    fn step(&mut self, time: &u64, state: &mut Lumi) -> Event;
    fn process_message(&self, msg: Message, time: u64, state: &mut Lumi) -> HandlerOutput;
}
