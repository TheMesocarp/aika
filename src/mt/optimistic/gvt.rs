// Implement message coordinator and GVT time update here

use std::sync::{atomic::AtomicU64, Arc};

use mesocarp::{comms::mailbox::{ThreadWorld, ThreadWorldUser}, scheduling::Scheduleable};

use crate::{messages::Transfer, SimError};

pub struct GVT<const SLOTS: usize, MessageType: Clone> {
    global_clock: Arc<AtomicU64>,
    thread_world: ThreadWorld<SLOTS, Transfer<MessageType>>,
    registered: usize
}

impl<const SLOTS: usize, MessageType: Clone> GVT<SLOTS, MessageType> {
    pub fn new(num_agents: usize) -> Result<Self, SimError> {
        let global_clock = Arc::new(AtomicU64::new(0));
        let mut agent_ids = Vec::new();
        for i in 0..num_agents {
            agent_ids.push(i);
        }
        let thread_world = ThreadWorld::new(agent_ids).map_err(SimError::MesoError)?;
        Ok(Self {
            global_clock,
            thread_world,
            registered: 0
        })
    }

    pub fn register_agent(&mut self) -> Result<(Arc<AtomicU64>, ThreadWorldUser<SLOTS, Transfer<MessageType>>, usize), SimError> {
        let arc = Arc::clone(&self.global_clock);
        let user = self.thread_world.get_user(self.registered).map_err(SimError::MesoError)?;
        let id = self.registered;
        self.registered += 1;
        Ok((arc, user, id))
    }

    pub fn poll(&mut self) -> Result<(), SimError> {
        let poll_results = self.thread_world.poll().map_err(SimError::MesoError)?;
        let mut lowest = u64::MAX;
        for (_, transfer) in &poll_results {
            let time = transfer.time();
            if time < lowest {
                lowest = time
            }
        }
        self.thread_world.deliver(poll_results).map_err(SimError::MesoError)?;
        let current = self.global_clock.load(std::sync::atomic::Ordering::Acquire);
        if current > lowest {
            return Err(SimError::TimeTravel)
        }
        self.global_clock.store(lowest, std::sync::atomic::Ordering::Release);
        Ok(())
    }
}