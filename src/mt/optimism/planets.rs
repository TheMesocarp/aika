use std::{cmp::Reverse, collections::{BTreeSet, BinaryHeap}};

use bytemuck::{Pod, Zeroable};
use mesocarp::{comms::mailbox::{Message, ThreadedMessengerUser}, logging::journal::Journal, scheduling::Scheduleable, sync::gvt::aika::BlockSpoke};

use crate::{mt::{agents::{PlanetContext, ThreadedAgent}, optimism::Time}, objects::{AntiMsg, Event, LocalEventSystem, LocalMailSystem, Mail, Msg, SchedulingTask, Transfer}, AikaError};


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

    fn commit(&mut self, event: Event) {
        self.event_system.insert(event)
    }

    fn commit_mail(&mut self, msg: Msg<MessageType>) {
        let msg = self.local_messages.schedule.insert(msg);
        if msg.is_err() {
            self.local_messages
                .overflow
                .push(Reverse(msg.err().unwrap()));
        }
    }

    /// Schedule an event for an agent at a given time.
    pub fn schedule(&mut self, time: u64, agent: usize) -> Result<(), AikaError> {
        if time < self.now() {
            return Err(AikaError::TimeTravel);
        } else if time as f64 * self.time.timestep > self.time.terminal {
            return Err(AikaError::PastTerminal);
        }
        let now = self.now();
        self.commit(Event::new(now, time, agent, SchedulingTask::Wait));
        Ok(())
    }

    /// Get the current time of the simulation.
    #[inline(always)]
    pub fn now(&self) -> u64 {
        self.event_system.local_clock.time
    }

    pub fn spawn_agent(
        &mut self,
        agent: Box<dyn ThreadedAgent<MSG_BW, MessageType>>,
        state_arena_size: usize,
    ) -> usize {
        self.agents.push(agent);
        self.context
            .agent_states
            .push(Journal::init(state_arena_size));
        self.agents.len() - 1
    }

    /// Spawn a preconfigured `ThreadedAgent`.
    pub fn spawn_agent_preconfigured(
        &mut self,
        agent: Box<dyn ThreadedAgent<MSG_BW, MessageType>>,
    ) -> usize {
        self.agents.push(agent);
        self.agents.len() - 1
    }

    // NEED TO REVIEW
    fn rollback(&mut self, time: u64) -> Result<(), AikaError> {
        let now = self.event_system.local_clock.time;
        if time > now {
            return Err(AikaError::TimeTravel);
        }
        // rollback world and agent states
        self.context.world_state.rollback(time);
        for i in &mut self.context.agent_states {
            i.rollback(time);
        }
        // rollback local message scheduler
        self.local_messages
            .schedule
            .rollback(&mut self.local_messages.overflow, time);
        // rollback and claim all the anti messages produced after the rollback time
        let anti_msgs: Vec<(Mail<MessageType>, u64)> = self.context.anti_msgs.rollback_return(time);

        // send out anti messages generated post rollback.
        for (anti, _) in anti_msgs {
            if let Some(to) = anti.to_world {
                if to == self.context.world_id {
                    let anti = anti.open_letter();
                    if let Transfer::AntiMsg(anti) = anti {
                        self.annihilate(anti);
                    }
                    continue;
                }
            }
            self.message_user.send(anti)?;
        }

        // rollback local event scheduling system.
        self.event_system
            .local_clock
            .rollback(&mut self.event_system.overflow, time);
        // reset context time
        self.context.time = time;

        println!(
            "Planet {:?}, Time {now}: ROLLBACK!!!!! rolling back to {time}",
            self.context.world_id
        );
        Ok(())
    }

    // NEED TO REVIEW
    fn annihilate(&mut self, anti_msg: AntiMsg) {
        let time = anti_msg.time();
        let idxs = self.local_messages.schedule.current_idxs;
        let diff = (time - self.local_messages.schedule.time) as usize;
        for (k, idx) in idxs.iter().enumerate().take(CLOCK_SCALES) {
            let startidx = ((CLOCK_BW).pow(1 + k as u32) - CLOCK_BW) / (CLOCK_BW - 1); // start index for each level
            let endidx = ((CLOCK_BW).pow(2 + k as u32) - CLOCK_BW) / (CLOCK_BW - 1) - 1; // end index for each level
            if diff >= startidx {
                if diff
                    >= (((CLOCK_BW).pow(1 + CLOCK_SCALES as u32) - CLOCK_BW)
                        / (CLOCK_BW - 1))
                {
                    break;
                }
                if diff > endidx {
                    continue;
                }
                let offset = ((diff - startidx) / (CLOCK_BW.pow(k as u32)) + idx) % CLOCK_BW;
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

    fn poll_interplanetary_messenger(&mut self) -> Result<(), AikaError> {
        let maybe = self.message_user.poll();
        if maybe.is_none() {
            return Ok(());
        }
        for msg in maybe.unwrap() {
            if let Some(to) = msg.to_world {
                if to != self.context.world_id {
                    println!("!!! PANIC !!! mail planet to ID: {to}, context ID {:?}. source agent {:?} on planet {:?}", self.context.world_id, msg.transfer.from(), msg.from_world);
                    return Err(AikaError::MismatchedDeliveryAddress);
                }
            }
            let time = msg.transfer.time();
            println!(
                "Planet {:?}: opening mail with recieve time {time}",
                self.context.world_id
            );
            if time < self.now() {
                println!(
                    "Planet {:?}, Time {:?}: found old message in poll with recieve time {time}",
                    self.context.world_id,
                    self.now()
                );
                self.rollback(time)?;
            }

            match msg.open_letter() {
                Transfer::Msg(msg) => {
                    self.blocks.block.recv(msg.commit_time(), msg.commit_time() < self.blocks.block.start)?;
                    self.commit_mail(msg)
                }
                Transfer::AntiMsg(anti_msg) => self.annihilate(anti_msg),
            }
        }
        Ok(())
    }
}