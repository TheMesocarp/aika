use mesocarp::{comms::mailbox::ThreadWorldUser, logging::journal::Journal};

use crate::{messages::Msg, st::event::Event};

pub struct AgentSupport<const SLOTS: usize, T: Clone> {
    mailbox: Option<ThreadWorldUser<SLOTS, Msg<T>>>,
    logger: Option<Journal>,
    pub current_time: u64,
}

impl<const SLOTS: usize, T: Clone> AgentSupport<SLOTS, T>{
    pub fn new(mail: Option<ThreadWorldUser<SLOTS, Msg<T>>>, size: Option<usize>) -> Self {
        let logger = if size.is_some() {
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
    fn step(&self, supports: &mut AgentSupport<SLOTS, MessageType>) -> Event;
}