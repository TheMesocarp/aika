use std::{
    cmp::Reverse,
    collections::{BTreeSet, BinaryHeap},
    sync::Arc,
    thread::sleep,
    time::Duration,
};

use bytemuck::{Pod, Zeroable};
use mesocarp::{
    comms::{spmc::Subscriber, spsc::BufferWheel},
    logging::journal::Journal,
    scheduling::Scheduleable,
};

use crate::{
    agents::{PlanetContext, ThreadedAgent},
    mt::hybrid::{blocks::Block, galaxy::PlanetaryRegister},
    objects::{Action, AntiMsg, Event, LocalEventSystem, LocalMailSystem, Mail, Msg, Transfer},
    AikaError,
};

pub enum Noisiness {
    Silent,
    Quiet,
    Average,
    Loud,
    Screaming,
}

pub struct Planet<
    const MSG_SLOTS: usize,
    const BLOCK_SLOTS: usize,
    const GVT_SLOTS: usize,
    const CLOCK_SLOTS: usize,
    const CLOCK_HEIGHT: usize,
    MessageType: Pod + Zeroable + Clone,
> {
    // interactables
    pub agents: Vec<Box<dyn ThreadedAgent<MSG_SLOTS, MessageType>>>,
    pub context: PlanetContext<MSG_SLOTS, MessageType>,
    // local processors
    event_system: LocalEventSystem<CLOCK_SLOTS, CLOCK_HEIGHT>,
    local_messages: LocalMailSystem<CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>,
    // block management
    block_submitter: Arc<BufferWheel<BLOCK_SLOTS, Block<BLOCK_SLOTS>>>,
    block: Block<BLOCK_SLOTS>,
    block_nmb: usize,
    block_size: u64,
    // time
    throttle: u64,
    checkpoint_hz: u64,
    current_gvt: u64,
    timestep: f64,
    terminal: f64,
    gvt: Subscriber<GVT_SLOTS, u64>,
}

unsafe impl<
        const MSG_SLOTS: usize,
        const BLOCK_SLOTS: usize,
        const GVT_SLOTS: usize,
        const CLOCK_SLOTS: usize,
        const CLOCK_HEIGHT: usize,
        MessageType: Pod + Zeroable + Clone,
    > Send for Planet<MSG_SLOTS, BLOCK_SLOTS, GVT_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
}

unsafe impl<
        const MSG_SLOTS: usize,
        const BLOCK_SLOTS: usize,
        const GVT_SLOTS: usize,
        const CLOCK_SLOTS: usize,
        const CLOCK_HEIGHT: usize,
        MessageType: Pod + Zeroable + Clone,
    > Sync for Planet<MSG_SLOTS, BLOCK_SLOTS, GVT_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
}

impl<
        const MSG_SLOTS: usize,
        const BLOCK_SLOTS: usize,
        const GVT_SLOTS: usize,
        const CLOCK_SLOTS: usize,
        const CLOCK_HEIGHT: usize,
        MessageType: Pod + Zeroable + Clone,
    > Planet<MSG_SLOTS, BLOCK_SLOTS, GVT_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
    pub fn create(
        registration: PlanetaryRegister<MSG_SLOTS, BLOCK_SLOTS, GVT_SLOTS, MessageType>,
        shared_world_size: usize,
        noise_level: Noisiness,
    ) -> Result<Self, AikaError> {
        let size = match noise_level {
            Noisiness::Silent => 0,
            Noisiness::Quiet => 16,
            Noisiness::Average => 64,
            Noisiness::Loud => 256,
            Noisiness::Screaming => 512,
        } * 1024;
        Ok(Self {
            agents: Vec::new(),
            context: PlanetContext::new(
                shared_world_size,
                size,
                registration.messenger_account,
                registration.planet_id,
            ),
            event_system: LocalEventSystem::<CLOCK_SLOTS, CLOCK_HEIGHT>::new()?,
            local_messages: LocalMailSystem::new()?,
            block_submitter: registration.block_channel,
            block: Block::new(1, 1 + registration.block_size, registration.planet_id, 1)?,
            block_nmb: 1,
            block_size: registration.block_size,
            throttle: registration.throttle,
            checkpoint_hz: registration.checkpoint_hz,
            current_gvt: 0,
            timestep: registration.timestep,
            terminal: registration.terminal,
            gvt: registration.gvt_subscriber,
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
        } else if time as f64 * self.timestep > self.terminal {
            return Err(AikaError::PastTerminal);
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

    pub fn spawn_agent(
        &mut self,
        agent: Box<dyn ThreadedAgent<MSG_SLOTS, MessageType>>,
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
        agent: Box<dyn ThreadedAgent<MSG_SLOTS, MessageType>>,
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
            self.context.user.send(anti)?;
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

    fn poll_interplanetary_messenger(&mut self) -> Result<(), AikaError> {
        let maybe = self.context.user.poll();
        if maybe.is_none() {
            return Ok(());
        }
        for msg in maybe.unwrap() {
            if let Some(to) = msg.to_world {
                if to != self.context.world_id {
                    return Err(AikaError::MismatchedDeliveryAddress);
                }
            }
            self.block.recv(msg.transfer.commit_time())?;
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
                Transfer::Msg(msg) => self.commit_mail(msg),
                Transfer::AntiMsg(anti_msg) => self.annihilate(anti_msg),
            }
        }
        Ok(())
    }

    fn step(&mut self) -> Result<(), AikaError> {
        self.check_time_validity()?;

        // process messages at the next time step
        if let Ok(msgs) = self.local_messages.schedule.tick() {
            for msg in msgs {
                self.context.time = msg.time();
                let id = msg.to;
                if id.is_none() {
                    for i in 0..self.agents.len() {
                        self.agents[i].read_message(&mut self.context, msg, i);
                    }
                    continue;
                }
                let id = id.unwrap();
                self.agents[id].read_message(&mut self.context, msg, id);
            }
        }
        // process events at the next time step
        if let Ok(events) = self.event_system.local_clock.tick() {
            for event in events {
                self.context.time = event.time;
                let event = self.agents[event.agent].step(&mut self.context, event.agent);
                match event.yield_ {
                    Action::Timeout(time) => {
                        if (self.now() + time) as f64 * self.timestep > self.terminal {
                            continue;
                        }

                        self.commit(Event::new(
                            self.now(),
                            self.now() + time,
                            event.agent,
                            Action::Wait,
                        ));
                    }
                    Action::Schedule(time) => {
                        self.commit(Event::new(self.now(), time, event.agent, Action::Wait));
                    }
                    Action::Trigger { time, idx } => {
                        self.commit(Event::new(self.now(), time, idx, Action::Wait));
                    }
                    Action::Wait => {}
                    Action::Break => {
                        break;
                    }
                }
            }
        }
        self.block.sends += self.context.sends;
        self.context.sends = 0;
        self.increment()?;
        std::thread::yield_now();
        Ok(())
    }

    fn increment(&mut self) -> Result<(), AikaError> {
        self.event_system
            .local_clock
            .increment(&mut self.event_system.overflow);
        self.local_messages
            .schedule
            .increment(&mut self.local_messages.overflow);
        self.context.time += 1;
        // check-process block now
        if self.context.time > self.block.end {
            self.block_submitter.write(std::mem::take(&mut self.block))?;

            self.block_nmb += 1;

            self.block.block_id = (self.context.world_id, self.block_nmb);
            self.block.start = self.context.time;
            self.block.end = self.context.time + self.block_size;
        }
        Ok(())
    }

    fn check_time_validity(&self) -> Result<(), AikaError> {
        if self.local_messages.schedule.time != self.event_system.local_clock.time
            && self.local_messages.schedule.time != self.context.time
        {
            return Err(AikaError::ClockSyncIssue);
        }
        if self.terminal <= self.timestep * self.context.time as f64 {
            return Err(AikaError::PastTerminal);
        }
        if self.current_gvt as f64 * self.timestep >= self.terminal {
            return Err(AikaError::PastTerminal);
        }
        Ok(())
    }

    pub fn run(&mut self) -> Result<(), AikaError> {
        // let id = self.context.world_id;
        loop {
            // poll-receive messages and GVT updates before proceeding
            let now = self.now();
            for _ in 0..8 {
                self.poll_interplanetary_messenger()?;
            }
            if let Some(gvt) = self.gvt.try_recv() {
                self.current_gvt = gvt;
            }

            // if at a checkpoint or the throttle limit, busy-wait the thread
            if now == (self.checkpoint_hz * self.block_size * self.block_nmb as u64)
                && now != (self.terminal / self.timestep) as u64
                && self.current_gvt != now
            {
                sleep(Duration::from_nanos(100));
                std::thread::yield_now();
                continue;
            }
            if self.current_gvt + (self.throttle * self.block_size) < self.now() {
                sleep(Duration::from_nanos(100));
                std::thread::yield_now();
                continue;
            }
            // step the sim forward one time step
            let step = self.step();
            if let Err(AikaError::PastTerminal) = step {
                break;
            }
            step?;
            std::thread::yield_now();
        }
        Ok(())
    }
}

#[cfg(test)]
mod planet_tests {
    use super::*;
    use crate::{
        agents::{PlanetContext, ThreadedAgent},
        mt::hybrid::planet::Planet,
        objects::{Action, Event, Mail, Msg},
    };
    use bytemuck::{Pod, Zeroable};
    use mesocarp::comms::{mailbox::ThreadedMessenger, spmc::Broadcast};
    use std::sync::Arc;

    // Simple test message type
    #[derive(Copy, Clone, Debug, PartialEq)]
    #[repr(C)]
    struct TestMessage {
        value: u32,
        sender_id: u32,
    }

    unsafe impl Pod for TestMessage {}
    unsafe impl Zeroable for TestMessage {}

    // Basic test agent that just schedules timeouts
    struct BasicTestAgent {
        timeout_count: usize,
        max_timeouts: usize,
    }

    impl ThreadedAgent<16, TestMessage> for BasicTestAgent {
        fn step(
            &mut self,
            _context: &mut PlanetContext<16, TestMessage>,
            agent_id: usize,
        ) -> Event {
            let time = _context.time;
            self.timeout_count += 1;

            if self.timeout_count < self.max_timeouts {
                Event::new(time, time, agent_id, Action::Timeout(10))
            } else {
                Event::new(time, time, agent_id, Action::Wait)
            }
        }

        fn read_message(
            &mut self,
            _context: &mut PlanetContext<16, TestMessage>,
            _msg: Msg<TestMessage>,
            _agent_id: usize,
        ) {
            // Basic agent doesn't process messages
        }
    }

    // Agent that triggers other agents
    struct TriggerAgent {
        target: usize,
        trigger_time: u64,
        triggered: bool,
    }

    impl ThreadedAgent<16, TestMessage> for TriggerAgent {
        fn step(&mut self, context: &mut PlanetContext<16, TestMessage>, agent_id: usize) -> Event {
            let time = context.time;

            if !self.triggered && time >= 10 {
                self.triggered = true;
                Event::new(
                    time,
                    time,
                    agent_id,
                    Action::Trigger {
                        time: self.trigger_time,
                        idx: self.target,
                    },
                )
            } else {
                Event::new(time, time, agent_id, Action::Timeout(5))
            }
        }

        fn read_message(
            &mut self,
            _context: &mut PlanetContext<16, TestMessage>,
            _msg: Msg<TestMessage>,
            _agent_id: usize,
        ) {
            // Doesn't process messages
        }
    }

    // Helper function to create a mock RegistryOutput
    fn create_mock_registry(
        world_id: usize,
    ) -> Result<PlanetaryRegister<16, 32, 8, TestMessage>, AikaError> {
        let block_channel = Arc::new(BufferWheel::new());
        // Create a simple messenger for testing
        let messenger = ThreadedMessenger::<16, Mail<TestMessage>>::new(vec![world_id])?;
        let user = messenger.get_user(world_id)?;

        let gvt = Broadcast::new()?;
        let gvt_subscriber = Arc::new(gvt).register_subscriber();

        Ok(PlanetaryRegister { planet_id: 0, messenger_account: user, block_channel, gvt_subscriber, terminal: 300.0, timestep: 1.0, throttle: 5, checkpoint_hz: 10, block_size: 16 })
    }

    #[test]
    fn test_planet_creation() {
        let registry = create_mock_registry(0).unwrap();

        let planet = Planet::<16, 32, 8, 128, 2, TestMessage>::create(
            registry, // terminal
            1024,     // timestep
            Noisiness::Average,
        );

        assert!(planet.is_ok());
        let planet = planet.unwrap();
        assert_eq!(planet.agents.len(), 0);
        assert_eq!(planet.now(), 0);
    }

    #[test]
    fn test_spawn_agent() {
        let registry = create_mock_registry(0).unwrap();
        let mut planet = Planet::<16, 32, 8, 128, 2, TestMessage>::create(
            registry, // terminal
            1024,     // timestep
            Noisiness::Average,
        )
        .unwrap();

        let agent = BasicTestAgent {
            timeout_count: 0,
            max_timeouts: 5,
        };

        let agent_id = planet.spawn_agent(Box::new(agent), 256);
        assert_eq!(agent_id, 0);
        assert_eq!(planet.agents.len(), 1);
        assert_eq!(planet.context.agent_states.len(), 1);
    }

    #[test]
    fn test_schedule_event() {
        let registry = create_mock_registry(0).unwrap();
        let mut planet = Planet::<16, 32, 8, 128, 2, TestMessage>::create(
            registry, // terminal
            1024,     // timestep
            Noisiness::Average,
        )
        .unwrap();

        let agent = BasicTestAgent {
            timeout_count: 0,
            max_timeouts: 5,
        };

        planet.spawn_agent(Box::new(agent), 256);

        // Schedule event at time 10
        let result = planet.schedule(10, 0);
        assert!(result.is_ok());

        // Try to schedule in the past (should fail)
        planet.event_system.local_clock.time = 20;
        let result = planet.schedule(5, 0);
        assert!(matches!(result, Err(AikaError::TimeTravel)));

        // Try to schedule past terminal (should fail)
        let result = planet.schedule(2000, 0);
        assert!(matches!(result, Err(AikaError::PastTerminal)));
    }

    #[test]
    fn test_time_advancement() {
        let registry = create_mock_registry(0).unwrap();
        let mut planet = Planet::<16, 32, 8, 128, 2, TestMessage>::create(
            registry, // terminal
            1024,     // timestep
            Noisiness::Average,
        )
        .unwrap();

        let agent = BasicTestAgent {
            timeout_count: 0,
            max_timeouts: 1,
        };

        planet.spawn_agent(Box::new(agent), 256);
        planet.schedule(1, 0).unwrap();

        // Step forward
        let initial_time = planet.now();
        let result = planet.step();
        assert!(result.is_ok());
        assert_eq!(planet.now(), initial_time + 1);
    }

    #[test]
    fn test_rollback() {
        let registry = create_mock_registry(0).unwrap();
        let mut planet = Planet::<16, 32, 8, 128, 2, TestMessage>::create(
            registry, // terminal
            1024,     // timestep
            Noisiness::Average,
        )
        .unwrap();

        // Advance time
        planet.event_system.local_clock.time = 50;
        planet.local_messages.schedule.time = 50;
        planet.context.time = 50;

        // Rollback to time 25
        let result = planet.rollback(25);
        assert!(result.is_ok());
        assert_eq!(planet.event_system.local_clock.time, 25);

        // Try to rollback to future (should fail)
        let result = planet.rollback(100);
        assert!(matches!(result, Err(AikaError::TimeTravel)));
    }

    #[test]
    fn test_agent_triggering() {
        let registry = create_mock_registry(0).unwrap();
        let mut planet = Planet::<16, 32, 8, 128, 2, TestMessage>::create(
            registry, // terminal
            1024,     // timestep
            Noisiness::Average,
        )
        .unwrap();

        // Create trigger agent
        let trigger_agent = TriggerAgent {
            target: 1,
            trigger_time: 30,
            triggered: false,
        };

        // Create target agent
        let target_agent = BasicTestAgent {
            timeout_count: 0,
            max_timeouts: 3,
        };

        planet.spawn_agent(Box::new(trigger_agent), 256);
        planet.spawn_agent(Box::new(target_agent), 256);

        // Schedule trigger agent
        planet.schedule(1, 0).unwrap();

        // Run for a few steps
        for _ in 0..15 {
            if planet.step().is_err() {
                break;
            }
        }

        // The trigger should have fired and scheduled the target
        assert!(planet.now() >= 15);
    }
}
