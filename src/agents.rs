//! Agent traits and execution contexts for both single-threaded and multi-threaded simulations.
//! Provides `Agent` trait for single-threaded worlds and `ThreadedAgent` for multi-threaded planets,
//! along with their respective context structures that manage state and inter-agent communication.
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use bytemuck::{Pod, Zeroable};
use mesocarp::{
    comms::mailbox::{Message, ThreadedMessengerUser},
    logging::journal::Journal,
};

use crate::{
    objects::{AntiMsg, Event, Mail, Msg, Transfer},
    AikaError,
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

/// Shared context local `ThreadedAgents` mutate within a `Planet` thread
pub struct PlanetContext<const INTER_SLOTS: usize, MessageType: Pod + Zeroable + Clone> {
    /// state of each `ThreadedAgent` on the `Planet`
    pub agent_states: Vec<Journal>,
    /// `Planet` global state
    pub world_state: Journal,
    /// current time
    pub time: u64,
    /// world ID in the interplanetary messaging system
    pub world_id: usize,
    /// Counter for unprocessed messages in the system
    pub counter: Arc<AtomicUsize>,
    /// interplanetary messaging system user interface
    pub user: ThreadedMessengerUser<INTER_SLOTS, Mail<MessageType>>,
    /// all anti messages generated by this `Planet`
    pub anti_msgs: Journal,
}

impl<const INTER_SLOTS: usize, MessageType: Pod + Zeroable + Clone>
    PlanetContext<INTER_SLOTS, MessageType>
{
    /// Spawn a new context environment for a `Planet`.
    pub fn new(
        world_arena_size: usize,
        anti_msg_arena_size: usize,
        user: ThreadedMessengerUser<INTER_SLOTS, Mail<MessageType>>,
        world_id: usize,
        counter: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            agent_states: Vec::new(),
            world_state: Journal::init(world_arena_size),
            time: 0,
            user,
            world_id,
            counter,
            anti_msgs: Journal::init(anti_msg_arena_size),
        }
    }

    /// Initialize a `ThreadedAgent`'s state `Journal`.
    pub fn init_agent_contexts(&mut self, state_arena_size: usize) {
        self.agent_states.push(Journal::init(state_arena_size));
    }
    /// Send a `Msg` to another `Planet`
    pub fn send_mail(&mut self, msg: Msg<MessageType>, to_world: usize) -> Result<(), AikaError> {
        let anti = AntiMsg::new(msg.sent, msg.recv, msg.from, msg.to);
        let outgoing = Mail::write_letter(Transfer::Msg(msg), self.world_id, Some(to_world));
        self.user.send(outgoing)?;
        self.counter.fetch_add(1, Ordering::SeqCst);
        let stays: Mail<MessageType> =
            Mail::write_letter(Transfer::AntiMsg(anti), self.world_id, Some(to_world));
        self.anti_msgs.write(stays, self.time, None);
        Ok(())
    }
}

/// An `Agent` is an independent logical process that can interact with a single threaded `st::World`
pub trait Agent<const SLOTS: usize, T: Message> {
    fn step(&mut self, context: &mut WorldContext<SLOTS, T>, agent_id: usize) -> Event;
}

/// A `ThreadedAgent` is an independent logical process that belongs to a `Planet` and can schedule events,
/// send messages, and interact with that `Planet`'s `PlanetContext`.
pub trait ThreadedAgent<const SLOTS: usize, MessageType: Pod + Zeroable + Clone> {
    fn step(&mut self, context: &mut PlanetContext<SLOTS, MessageType>, agent_id: usize) -> Event;
    fn read_message(
        &mut self,
        context: &mut PlanetContext<SLOTS, MessageType>,
        msg: Msg<MessageType>,
        agent_id: usize,
    );
}
