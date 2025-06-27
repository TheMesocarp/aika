use mesocarp::{comms::mailbox::ThreadWorldUser, logging::journal::Journal};

use crate::worlds::{event::Event, message::Msg};

pub struct AgentSupport<const SLOTS: usize, T: Clone> {
    mailbox: Option<ThreadWorldUser<SLOTS, Msg<T>>>,
    logger: Option<Journal>,
    pub current_time: u64,
}

impl<const SLOTS: usize, T: Clone> AgentSupport<SLOTS, T>{
    pub fn new(mail: Option<ThreadWorldUser<SLOTS, Msg<T>>>, size: Option<usize>) -> Self {
        let logger = if mail.is_some() {
            let size = size.unwrap();
            Some(Journal::init(size))
        } else {
            None
        };
        Self {
            mailbox: mail,
            logger,
            current_time: 0
        }
    }
}

pub trait Agent<const SLOTS: usize, MessageType: Clone> {
    fn step(&self, supports: &mut Option<AgentSupport<SLOTS, MessageType>>) -> Event;
}