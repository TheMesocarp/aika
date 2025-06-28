use mesocarp::{
    comms::mailbox::{Message, ThreadWorldUser},
    logging::journal::Journal,
};

use crate::st::event::Event;

pub struct AgentSupport<const SLOTS: usize, T: Message> {
    pub mailbox: Option<ThreadWorldUser<SLOTS, T>>,
    pub logger: Option<Journal>,
    pub current_time: u64,
}

impl<const SLOTS: usize, T: Message> AgentSupport<SLOTS, T> {
    pub fn new(mail: Option<ThreadWorldUser<SLOTS, T>>, size: Option<usize>) -> Self {
        let logger = if size.is_some() {
            let size = size.unwrap();
            Some(Journal::init(size))
        } else {
            None
        };
        Self {
            mailbox: mail,
            logger,
            current_time: 0,
        }
    }
}

pub trait Agent<const SLOTS: usize, T: Message> {
    fn step(&mut self, supports: &mut AgentSupport<SLOTS, T>) -> Event;
}

pub trait ThreadedAgent<const SLOTS: usize, T: Message>: Agent<SLOTS, T> {
    fn read_message(&mut self, supports: &mut AgentSupport<SLOTS, T>, agent_id: usize);
}