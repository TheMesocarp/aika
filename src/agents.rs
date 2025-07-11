use bytemuck::{Pod, Zeroable};
use mesocarp::{
    comms::mailbox::{Message, ThreadedMessengerUser},
    logging::journal::Journal,
};

use crate::{
    objects::{AntiMsg, Event, Mail, Msg, Transfer},
    SimError,
};

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
            time: 0,
        }
    }
}

pub struct PlanetContext<const INTER_SLOTS: usize, MessageType: Pod + Zeroable + Clone> {
    pub agent_states: Vec<Journal>,
    pub world_state: Journal,
    pub time: u64,
    pub world_id: usize,
    pub user: ThreadedMessengerUser<INTER_SLOTS, Mail<MessageType>>,
    pub anti_msgs: Journal,
}

impl<const INTER_SLOTS: usize, MessageType: Pod + Zeroable + Clone>
    PlanetContext<INTER_SLOTS, MessageType>
{
    pub fn new(
        world_arena_size: usize,
        anti_msg_arena_size: usize,
        user: ThreadedMessengerUser<INTER_SLOTS, Mail<MessageType>>,
        world_id: usize,
    ) -> Self {
        Self {
            agent_states: Vec::new(),
            world_state: Journal::init(world_arena_size),
            time: 0,
            user,
            world_id,
            anti_msgs: Journal::init(anti_msg_arena_size),
        }
    }

    pub fn init_agent_contexts(&mut self, state_arena_size: usize) {
        self.agent_states.push(Journal::init(state_arena_size));
    }

    pub fn send_mail(&mut self, msg: Msg<MessageType>, to_world: usize) -> Result<(), SimError> {
        let anti = AntiMsg::new(msg.sent, msg.recv, msg.from, msg.to);
        let outgoing = Mail::write_letter(Transfer::Msg(msg), self.world_id, Some(to_world));
        self.user.send(outgoing)?;

        let stays: Mail<MessageType> =
            Mail::write_letter(Transfer::AntiMsg(anti), self.world_id, Some(to_world));
        self.anti_msgs.write(stays, self.time, None);
        Ok(())
    }
}

pub trait Agent<const SLOTS: usize, T: Message> {
    fn step(&mut self, context: &mut WorldContext<SLOTS, T>, agent_id: usize) -> Event;
}

pub trait ThreadedAgent<const SLOTS: usize, MessageType: Pod + Zeroable + Clone> {
    fn step(&mut self, context: &mut PlanetContext<SLOTS, MessageType>, agent_id: usize) -> Event;
    fn read_message(
        &mut self,
        context: &mut PlanetContext<SLOTS, MessageType>,
        msg: Msg<MessageType>,
        agent_id: usize,
    );
}
