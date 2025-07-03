use mesocarp::{
    comms::mailbox::{Message, ThreadedMessengerUser},
    logging::journal::Journal,
};

use crate::event::Event;

pub struct AgentSupport<const SLOTS: usize, T: Message> {
    pub mailbox: Option<ThreadedMessengerUser<SLOTS, T>>,
    pub state: Option<Journal>,
}

impl<const SLOTS: usize, T: Message> AgentSupport<SLOTS, T> {
    pub fn new(mail: Option<ThreadedMessengerUser<SLOTS, T>>, arena_size: Option<usize>) -> Self {
        let state = if arena_size.is_some() {
            let size = arena_size.unwrap();
            Some(Journal::init(size))
        } else {
            None
        };
        Self {
            mailbox: mail,
            state,
        }
    }
}

pub struct WorldContext<const SLOTS: usize, T: Message> {
    pub agent_states: Vec<AgentSupport<SLOTS, T>>,
    pub world_state: Journal,
    pub time: u64,
}

impl<const SLOTS: usize, T: Message> WorldContext<SLOTS, T> {
    pub fn new(world_arena_size: usize) -> Self {
        Self {
            agent_states: Vec::new(),
            world_state: Journal::init(world_arena_size),
            time: 0
        }
    }
}

pub trait Agent<const SLOTS: usize, T: Message> {
    fn step(&mut self, context: &mut WorldContext<SLOTS, T>, agent_id: usize) -> Event;
}

pub trait ThreadedAgent<const SLOTS: usize, T: Message>: Agent<SLOTS, T> {
    fn read_message(&mut self, context: &mut WorldContext<SLOTS, T>, agent_id: usize);
}