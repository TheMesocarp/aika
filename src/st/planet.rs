use mesocarp::comms::mailbox::ThreadedMessengerUser;

use crate::{agents::{ThreadedAgent, WorldContext}, event::LocalEventSystem, messages::{LocalMailSystem, Mail, Msg}};

pub struct Planet<const INTER_SLOTS: usize, const LOCAL_SLOTS: usize, const CLOCK_SLOTS: usize, const CLOCK_HEIGHT: usize, MessageType: Clone> {
    pub agents: Vec<Box<dyn ThreadedAgent<LOCAL_SLOTS, Msg<MessageType>>>>,
    pub context: WorldContext<LOCAL_SLOTS, Msg<MessageType>>, 
    pub event_system: LocalEventSystem<CLOCK_SLOTS, CLOCK_HEIGHT>,
    pub local_messages: LocalMailSystem<LOCAL_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>,
    pub interworld_messages: ThreadedMessengerUser<INTER_SLOTS, Mail<MessageType>>
}

impl<const INTER_SLOTS: usize, const LOCAL_SLOTS: usize, const CLOCK_SLOTS: usize, const CLOCK_HEIGHT: usize, MessageType: Clone> Planet<INTER_SLOTS, LOCAL_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType> {
    pub fn create() -> Self {
        todo!()
    }
}