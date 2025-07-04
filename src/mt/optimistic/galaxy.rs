use std::sync::{atomic::AtomicU64, Arc};

use bytemuck::{Pod, Zeroable};
use mesocarp::comms::mailbox::ThreadedMessenger;

use crate::{messages::Mail, mt::optimistic::planet::RegistryOutput, SimError};

pub struct Galaxy<
    const INTER_SLOTS: usize,
    const CLOCK_SLOTS: usize,
    const CLOCK_HEIGHT: usize,
    MessageType: Pod + Zeroable + Clone,
> {
    pub messenger: ThreadedMessenger<INTER_SLOTS, Mail<MessageType>>,
    pub lvts: Vec<Arc<AtomicU64>>,
    pub gvt: Arc<AtomicU64>,
    pub next_checkpoint: Arc<AtomicU64>,
    pub throttle_horizon: u64,
    pub checkpoint_frequency: u64,
    pub registered: usize

}

impl<const INTER_SLOTS: usize, const CLOCK_SLOTS: usize, const CLOCK_HEIGHT: usize, MessageType: Pod + Zeroable + Clone> Galaxy<INTER_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType> {
    pub fn new(num_world: usize, throttle_horizon: u64, checkpoint_frequency: u64) -> Result<Self, SimError> {
        let gvt = Arc::new(AtomicU64::new(0));
        let mut world_ids = Vec::new();
        for i in 0..num_world {
            world_ids.push(i);
        }
        let messenger = ThreadedMessenger::new(world_ids)?;
        Ok(Self {
            messenger,
            lvts: Vec::new(),
            gvt,
            next_checkpoint: Arc::new(AtomicU64::new(checkpoint_frequency)),
            throttle_horizon,
            checkpoint_frequency,
            registered: 0,
        })
    }

    pub fn spawn_world(&mut self) -> Result<RegistryOutput<INTER_SLOTS, MessageType>, SimError> {
        let arc = Arc::clone(&self.gvt);
        
        let lvt = Arc::new(AtomicU64::new(0));
        let out = Arc::clone(&lvt);

        self.lvts.push(lvt);

        let user = self
            .messenger
            .get_user(self.registered)?;
        let world_id = self.registered;
        self.registered += 1;
        Ok((arc, out, user, world_id))
    }

    pub fn deliver_the_mail(&mut self) -> Result<(), SimError> {
        let maybe = self.messenger.poll()?;
        self.messenger.deliver(maybe)?;
        Ok(())
    }

    pub fn gvt_calculation(&mut self) -> Result<(), SimError> {
        // Samadi's is nice but i need something compatible with checkpointing
        Ok(())
    }
}
