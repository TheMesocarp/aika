use bytemuck::{Pod, Zeroable};
use mesocarp::{comms::mailbox::ThreadedMessengerUser, sync::gvt::aika::BlockSpoke};

use crate::{mt::{agents::{PlanetContext, ThreadedAgent}, chain::Time}, objects::{LocalEventSystem, LocalMailSystem, Mail}, AikaError};


pub struct Planet<const BLOCK_BW: usize, const MSG_BW: usize, const CLOCK_BW: usize, const CLOCK_SCALES: usize, MessageType: Pod + Zeroable + Clone> {
    pub agents: Vec<Box<dyn ThreadedAgent<MSG_BW, MessageType>>>,
    pub context: PlanetContext<MSG_BW, MessageType>,
    event_system: LocalEventSystem<CLOCK_BW, CLOCK_SCALES>,
    local_messages: LocalMailSystem<CLOCK_BW, CLOCK_SCALES, MessageType>,
    message_user: ThreadedMessengerUser<MSG_BW, Mail<MessageType>>,
    blocks: BlockSpoke<BLOCK_BW>,
    pub time: Time
}

unsafe impl<const BLOCK_BW: usize, const MSG_BW: usize, const CLOCK_BW: usize, const CLOCK_SCALES: usize, MessageType: Pod + Zeroable + Clone> Send for Planet<BLOCK_BW, MSG_BW, CLOCK_BW, CLOCK_SCALES, MessageType> {}
unsafe impl<const BLOCK_BW: usize, const MSG_BW: usize, const CLOCK_BW: usize, const CLOCK_SCALES: usize, MessageType: Pod + Zeroable + Clone> Sync for Planet<BLOCK_BW, MSG_BW, CLOCK_BW, CLOCK_SCALES, MessageType> {}

impl<const BLOCK_BW: usize, const MSG_BW: usize, const CLOCK_BW: usize, const CLOCK_SCALES: usize, MessageType: Pod + Zeroable + Clone> Planet<BLOCK_BW, MSG_BW, CLOCK_BW, CLOCK_SCALES, MessageType> {
    pub fn from_galaxy_registration(time: Time, blocks: BlockSpoke<BLOCK_BW>, message_user: ThreadedMessengerUser<MSG_BW, Mail<MessageType>>, id: usize) -> Result<Self, AikaError> {
        Ok(Self {
            agents: Vec::new(),
            context: PlanetContext::new(32 * 1024, 16 * 1024, id),
            event_system: LocalEventSystem::new()?,
            local_messages: LocalMailSystem::new()?,
            message_user,
            blocks,
            time
        })
    }
}