use std::{cmp::Reverse, collections::{BTreeSet, BinaryHeap}, sync::{atomic::AtomicU64, Arc}};

use bytemuck::{Pod, Zeroable};
use mesocarp::{comms::mailbox::ThreadedMessengerUser, logging::journal::Journal, scheduling::{htw::Clock, Scheduleable}};

use crate::{
    agents::{PlanetContext, ThreadedAgent}, event::{Action, Event, LocalEventSystem}, messages::{AntiMsg, LocalMailSystem, Mail, Msg, Transfer}, st::TimeInfo, SimError
};

pub type RegistryOutput<const SLOTS: usize, MessageType> = (
    Arc<AtomicU64>,
    ThreadedMessengerUser<SLOTS, Mail<MessageType>>,
    usize,
);

pub struct Planet<
    const INTER_SLOTS: usize,
    const LOCAL_SLOTS: usize,
    const CLOCK_SLOTS: usize,
    const CLOCK_HEIGHT: usize,
    MessageType: Pod + Zeroable + Clone,
> {
    pub agents: Vec<Box<dyn ThreadedAgent<LOCAL_SLOTS, Msg<MessageType>>>>,
    pub context: PlanetContext,
    pub time_info: TimeInfo,
    event_system: LocalEventSystem<CLOCK_SLOTS, CLOCK_HEIGHT>,
    local_messages: LocalMailSystem<LOCAL_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>,
    interworld_messages: ThreadedMessengerUser<INTER_SLOTS, Mail<MessageType>>,
    gvt: Arc<AtomicU64>,
    world_id: usize,
}

unsafe impl<
        const INTER_SLOTS: usize,
        const LOCAL_SLOTS: usize,
        const CLOCK_SLOTS: usize,
        const CLOCK_HEIGHT: usize,
        MessageType: Pod + Zeroable + Clone,
    > Send for Planet<INTER_SLOTS, LOCAL_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
}
unsafe impl<
        const INTER_SLOTS: usize,
        const LOCAL_SLOTS: usize,
        const CLOCK_SLOTS: usize,
        const CLOCK_HEIGHT: usize,
        MessageType: Pod + Zeroable + Clone,
    > Sync for Planet<INTER_SLOTS, LOCAL_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
}

impl<
        const INTER_SLOTS: usize,
        const LOCAL_SLOTS: usize,
        const CLOCK_SLOTS: usize,
        const CLOCK_HEIGHT: usize,
        MessageType: Pod + Zeroable + Clone,
    > Planet<INTER_SLOTS, LOCAL_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
    pub fn create(terminal: f64, timestep: f64, world_arena_size: usize, registry: RegistryOutput<INTER_SLOTS, MessageType>) -> Result<Self, SimError> {
        Ok(Self {
            agents: Vec::new(),
            context: PlanetContext::new(world_arena_size),
            time_info: TimeInfo { terminal, timestep },
            event_system: LocalEventSystem::<CLOCK_SLOTS, CLOCK_HEIGHT>::new()?,
            local_messages: LocalMailSystem::new()?,
            interworld_messages: registry.1,
            gvt: registry.0,
            world_id: registry.2
        })
    }

    fn commit(&mut self, event: Event) {
        self.event_system.insert(event)
    }

    fn commit_mail(&mut self, msg: Msg<MessageType>) {
        let msg = self.local_messages.schedule.insert(msg);
        if msg.is_err() {
            self.local_messages.overflow.push(Reverse(msg.err().unwrap()));
        }
    }

    /// Schedule an event for an agent at a given time.
    pub fn schedule(&mut self, time: u64, agent: usize) -> Result<(), SimError> {
        if time < self.now() {
            return Err(SimError::TimeTravel);
        } else if time as f64 * self.time_info.timestep > self.time_info.terminal {
            return Err(SimError::PastTerminal);
        }
        let now = self.now();
        self.commit(Event::new(now, time, agent, Action::Wait));
        Ok(())
    }

    /// Get the current time of the simulation.
    #[inline(always)]
    pub fn now(&self) -> u64 {
        self.event_system.local_clock.time
    }

    pub fn spawn_agent(&mut self, agent: Box<dyn ThreadedAgent<LOCAL_SLOTS, Msg<MessageType>>>, state_arena_size: usize) -> usize {
        self.agents.push(agent);
        self.context.agent_states.push(Journal::init(state_arena_size));
        self.agents.len() - 1
    }

    fn rollback(&mut self, time: u64) -> Result<(), SimError> {
        if time > self.event_system.local_clock.time {
            return Err(SimError::TimeTravel);
        }
        self.context.world_state.rollback(time);
        for i in &mut self.context.agent_states {
            i.rollback(time);
        }
        self.local_messages
            .schedule
            .rollback(&mut self.local_messages.overflow, time);
        let mut anti_msgs = Vec::new();
        for i in &mut self.local_messages.anti_messages {
            let out: Vec<(Mail<MessageType>, u64)> = i.rollback_return(time);
            anti_msgs.extend(out);
        }
        for (anti, _) in anti_msgs {
            if let Some(to) = anti.to_world {
                if to == self.world_id {
                    let anti = anti.open_letter();
                    if let Transfer::AntiMsg(anti) = anti {
                        self.annihilate(anti);
                    }
                    continue;
                }
            }
            self.interworld_messages
                .send(anti)
                .map_err(SimError::MesoError)?;
        }

        self.event_system.local_clock = Clock::new().map_err(SimError::MesoError)?;
        self.event_system.local_clock.set_time(time);
        Ok(())
    }

    fn annihilate(&mut self, anti_msg: AntiMsg) {
        let time = anti_msg.time();
        let idxs = self.local_messages.schedule.current_idxs;
        let diff = (time - self.local_messages.schedule.time) as usize;
        for (k, idx) in idxs.iter().enumerate().take(CLOCK_HEIGHT) {
            let startidx = ((CLOCK_SLOTS).pow(1 + k as u32) - CLOCK_SLOTS) / (CLOCK_SLOTS - 1); // start index for each level
            let endidx = ((CLOCK_SLOTS).pow(2 + k as u32) - CLOCK_SLOTS) / (CLOCK_SLOTS - 1) - 1; // end index for each level
            if diff >= startidx {
                if diff
                    >= (((CLOCK_SLOTS).pow(1 + CLOCK_HEIGHT as u32) - CLOCK_SLOTS)
                        / (CLOCK_SLOTS - 1))
                {
                    break;
                }
                if diff > endidx {
                    continue;
                }
                let offset = ((diff - startidx) / (CLOCK_SLOTS.pow(k as u32)) + idx) % CLOCK_SLOTS;
                let msgs = &mut self.local_messages.schedule.wheels[k][offset];
                let mut remaining = Vec::new();
                while let Some(msg) = msgs.pop() {
                    if anti_msg.annihilate(&msg) {
                        continue;
                    }
                    remaining.push(msg);
                }
                *msgs = remaining;
                return;
            }
        }
        // fallback if timestamp beyond clock horizon
        let mut to_be_removed = BTreeSet::new();
        for i in self.local_messages.overflow.iter().enumerate() {
            if anti_msg.annihilate(&i.1 .0) {
                to_be_removed.insert(Reverse(i.0));
            }
        }
        let current = self.local_messages.overflow.clone();
        let mut vec = current.into_iter().collect::<Vec<_>>();
        for i in to_be_removed {
            let idx = i.0;
            vec.remove(idx);
        }
        self.local_messages.overflow = BinaryHeap::from_iter(vec);
    }

    fn step(&mut self) -> Result<(), SimError> {
        Ok(())
    }
}
