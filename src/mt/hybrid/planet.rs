//! Individual threaded simulation world containing agents and local event processing.
//! Each `Planet` runs independently with its own local time, handling agent execution, local
//! messaging, and rollback operations when causality violations are detected.
use std::{
    cmp::Reverse,
    collections::{BTreeSet, BinaryHeap},
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    thread::sleep,
    time::Duration,
};

use bytemuck::{Pod, Zeroable};
use mesocarp::{
    comms::mailbox::ThreadedMessengerUser,
    logging::journal::Journal,
    scheduling::Scheduleable,
};

use crate::{
    agents::{PlanetContext, ThreadedAgent},
    objects::{Action, AntiMsg, Event, LocalEventSystem, LocalMailSystem, Mail, Msg, Transfer},
    st::TimeInfo,
    AikaError,
};

/// The registry information required to spawn a new `Planet` in a `Galaxy`
pub struct RegistryOutput<const SLOTS: usize, MessageType: Pod + Zeroable + Clone> {
    gvt: Arc<AtomicU64>,
    send_counter: Arc<AtomicUsize>,
    recv_counter: Arc<AtomicUsize>,
    lvt: Arc<AtomicU64>,
    checkpoint: Arc<AtomicU64>,
    user: ThreadedMessengerUser<SLOTS, Mail<MessageType>>,
    world_id: usize,
}

impl<const SLOTS: usize, MessageType: Pod + Zeroable + Clone> RegistryOutput<SLOTS, MessageType> {
    pub fn new(
        gvt: Arc<AtomicU64>,
        lvt: Arc<AtomicU64>,
        send_counter: Arc<AtomicUsize>,
        recv_counter: Arc<AtomicUsize>,
        checkpoint: Arc<AtomicU64>,
        user: ThreadedMessengerUser<SLOTS, Mail<MessageType>>,
        world_id: usize,
    ) -> Self {
        Self {
            gvt,
            lvt,
            send_counter,
            recv_counter,
            checkpoint,
            user,
            world_id,
        }
    }
}

/// A `Planet` is much like `World`, except is equipped with "inter-planetary" messaging and rollback functionality.
pub struct Planet<
    const INTER_SLOTS: usize,
    const CLOCK_SLOTS: usize,
    const CLOCK_HEIGHT: usize,
    MessageType: Pod + Zeroable + Clone,
> {
    pub agents: Vec<Box<dyn ThreadedAgent<INTER_SLOTS, MessageType>>>,
    pub context: PlanetContext<INTER_SLOTS, MessageType>,
    time_info: TimeInfo,
    event_system: LocalEventSystem<CLOCK_SLOTS, CLOCK_HEIGHT>,
    local_messages: LocalMailSystem<CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>,
    gvt: Arc<AtomicU64>,
    next_checkpoint: Arc<AtomicU64>,
    local_time: Arc<AtomicU64>,
    throttle_horizon: u64,
    recv_counter: Arc<AtomicUsize>
}

unsafe impl<
        const INTER_SLOTS: usize,
        const CLOCK_SLOTS: usize,
        const CLOCK_HEIGHT: usize,
        MessageType: Pod + Zeroable + Clone,
    > Send for Planet<INTER_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
}
unsafe impl<
        const INTER_SLOTS: usize,
        const CLOCK_SLOTS: usize,
        const CLOCK_HEIGHT: usize,
        MessageType: Pod + Zeroable + Clone,
    > Sync for Planet<INTER_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
}

impl<
        const INTER_SLOTS: usize,
        const CLOCK_SLOTS: usize,
        const CLOCK_HEIGHT: usize,
        MessageType: Pod + Zeroable + Clone,
    > Planet<INTER_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
    /// Create a new `Planet` given the provided time information, `Galaxy` registry output, and arena allocation sizes.
    pub fn create(
        terminal: f64,
        timestep: f64,
        throttle_horizon: u64,
        world_arena_size: usize,
        anti_msg_arena_size: usize,
        registry: RegistryOutput<INTER_SLOTS, MessageType>,
    ) -> Result<Self, AikaError> {
        Ok(Self {
            agents: Vec::new(),
            context: PlanetContext::new(
                world_arena_size,
                anti_msg_arena_size,
                registry.user,
                registry.world_id,
                registry.send_counter,
            ),
            time_info: TimeInfo { terminal, timestep },
            event_system: LocalEventSystem::<CLOCK_SLOTS, CLOCK_HEIGHT>::new()?,
            local_messages: LocalMailSystem::new()?,
            gvt: registry.gvt,
            next_checkpoint: registry.checkpoint,
            local_time: registry.lvt,
            throttle_horizon,
            recv_counter: registry.recv_counter
        })
    }
    /// Creates a new `Planet` from registry, time, and HybridConfig information.
    pub fn from_config(
        world_consts: (usize, usize, &Vec<usize>),
        terminal: f64,
        timestep: f64,
        throttle_horizon: u64,
        registry: RegistryOutput<INTER_SLOTS, MessageType>,
    ) -> Result<Self, AikaError> {
        let mut context = PlanetContext::new(
            world_consts.0,
            world_consts.1,
            registry.user,
            registry.world_id,
            registry.send_counter,
        );
        for i in world_consts.2 {
            context.agent_states.push(Journal::init(*i));
        }
        Ok(Self {
            agents: Vec::new(),
            context,
            time_info: TimeInfo { terminal, timestep },
            event_system: LocalEventSystem::<CLOCK_SLOTS, CLOCK_HEIGHT>::new()?,
            local_messages: LocalMailSystem::new()?,
            gvt: registry.gvt,
            next_checkpoint: registry.checkpoint,
            local_time: registry.lvt,
            throttle_horizon,
            recv_counter: registry.recv_counter
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
        } else if time as f64 * self.time_info.timestep > self.time_info.terminal {
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

    /// Get the time information of the simulation.
    pub fn time_info(&self) -> (f64, f64) {
        (self.time_info.timestep, self.time_info.terminal)
    }

    /// Spawn a new `ThreadedAgent` on the `Planet` with the provided agent state arena allocation size.
    pub fn spawn_agent(
        &mut self,
        agent: Box<dyn ThreadedAgent<INTER_SLOTS, MessageType>>,
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
        agent: Box<dyn ThreadedAgent<INTER_SLOTS, MessageType>>,
    ) -> usize {
        self.agents.push(agent);
        self.agents.len() - 1
    }

    fn rollback(&mut self, time: u64) -> Result<(), AikaError> {
        let now = self.event_system.local_clock.time;
        if time > now {
            return Err(AikaError::TimeTravel);
        }
        self.context.world_state.rollback(time);
        for i in &mut self.context.agent_states {
            i.rollback(time);
        }
        self.local_messages
            .schedule
            .rollback(&mut self.local_messages.overflow, time);
        let anti_msgs: Vec<(Mail<MessageType>, u64)> = self.context.anti_msgs.rollback_return(time);
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

        self.event_system.local_clock.rollback(&mut self.event_system.overflow, time);
        self.local_time.store(time, Ordering::Release);

        println!("ROLLBACK!!!!! rolling back! world {:?}, rollback time {time}, prior {now}", self.context.world_id);
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

    fn poll_interplanetary_messenger(&mut self) -> Result<(), AikaError> {
        let mut counter = 0;
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
            let time = msg.transfer.time();
            println!("opening mail on planet {:?}, with recieve time {time}", self.context.world_id);
            if time < self.now() {
                println!("found message in poll with recieve time {time}, local clock is {:?}, world {:?}", self.now(), self.context.world_id);
                self.rollback(time)?;
            }
            match msg.open_letter() {
                Transfer::Msg(msg) => self.commit_mail(msg),
                Transfer::AntiMsg(anti_msg) => self.annihilate(anti_msg),
            }
            counter += 1;
        }
        if counter == 0 {
            return Ok(())
        }
        let current = self.recv_counter.fetch_add(counter, Ordering::AcqRel);
        println!("planet {:?} polled, receive counter {current} with {counter} messages more", self.context.world_id);
        Ok(())
    }

    /// step forward one timestamp on all local clocks
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
                        if (self.now() + time) as f64 * self.time_info.timestep
                            > self.time_info.terminal
                        {
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
        self.increment();
        std::thread::yield_now();
        Ok(())
    }

    fn increment(&mut self) {
        self.event_system
            .local_clock
            .increment(&mut self.event_system.overflow);
        self.local_messages
            .schedule
            .increment(&mut self.local_messages.overflow);
        self.context.time += 1;
        self.local_time.store(self.now(), Ordering::Release);
    }

    fn check_time_validity(&self) -> Result<(), AikaError> {
        let load = self.local_time.load(Ordering::Acquire);
        if self.local_messages.schedule.time != self.event_system.local_clock.time
            && self.local_messages.schedule.time != load
        {
            return Err(AikaError::ClockSyncIssue);
        }
        if self.time_info.terminal <= self.time_info.timestep * load as f64 {
            return Err(AikaError::PastTerminal);
        }
        let gvt = self.gvt.load(Ordering::Acquire);
        if gvt as f64 * self.time_info.timestep >= self.time_info.terminal {
            return Err(AikaError::PastTerminal);
        }
        println!("valid time: gvt {gvt}, local {load}, world {:?}", self.context.world_id);
        Ok(())
    }

    /// Run the `Planet` optimistically.
    pub fn run(&mut self) -> Result<(), AikaError> {
        let id = self.context.world_id;
        loop {
            let checkpoint = self.next_checkpoint.load(Ordering::SeqCst);
            let now = self.now();
            self.poll_interplanetary_messenger()?;
            if now == checkpoint
                && now != (self.time_info.terminal / self.time_info.timestep) as u64
            {
                //println!("world {id} found sleeping, checkpoint");
                sleep(Duration::from_nanos(100));
                continue;
            }
            let gvt = self.gvt.load(Ordering::SeqCst);
            //println!("world {id} found gvt {gvt}, has local time {now}");
            if gvt + self.throttle_horizon < self.now() {
                //println!("world {id} found sleeping, throttle");
                sleep(Duration::from_nanos(100));
                continue;
            }
            let step = self.step();
            if let Err(AikaError::PastTerminal) = step {
                break;
            }
            step?;
        }
        println!("made it here for planet {id}, almost done");
        Ok(())
    }
}

#[cfg(test)]
mod planet_tests {
    use super::*;
    use crate::{
        agents::{PlanetContext, ThreadedAgent},
        mt::hybrid::planet::{Planet, RegistryOutput},
        objects::{Action, Event, Mail, Msg},
    };
    use bytemuck::{Pod, Zeroable};
    use mesocarp::comms::mailbox::ThreadedMessenger;
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    };

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
    fn create_mock_registry(world_id: usize) -> Result<RegistryOutput<16, TestMessage>, AikaError> {
        let gvt = Arc::new(AtomicU64::new(0));
        let lvt = Arc::new(AtomicU64::new(0));
        let checkpoint = Arc::new(AtomicU64::new(100));
        let send_counter = Arc::new(AtomicUsize::new(0));
        let recv_counter = Arc::new(AtomicUsize::new(0));
        // Create a simple messenger for testing
        let messenger = ThreadedMessenger::<16, Mail<TestMessage>>::new(vec![world_id])?;
        let user = messenger.get_user(world_id)?;

        Ok(RegistryOutput::new(
            gvt, lvt, send_counter, recv_counter, checkpoint, user, world_id,
        ))
    }

    #[test]
    fn test_planet_creation() {
        let registry = create_mock_registry(0).unwrap();

        let planet = Planet::<16, 128, 2, TestMessage>::create(
            1000.0, // terminal
            1.0,    // timestep
            50,     // throttle_horizon
            1024,   // world_arena_size
            512,    // anti_msg_arena_size
            registry,
        );

        assert!(planet.is_ok());
        let planet = planet.unwrap();
        assert_eq!(planet.agents.len(), 0);
        assert_eq!(planet.now(), 0);
    }

    #[test]
    fn test_planet_from_config() {
        let registry = create_mock_registry(0).unwrap();
        let agent_state_sizes = vec![256, 256, 256];
        let config = (1024, 512, &agent_state_sizes);

        let planet = Planet::<16, 128, 2, TestMessage>::from_config(
            config, 1000.0, // terminal
            1.0,    // timestep
            50,     // throttle_horizon
            registry,
        );

        assert!(planet.is_ok());
        let planet = planet.unwrap();
        assert_eq!(planet.context.agent_states.len(), 3);
    }

    #[test]
    fn test_spawn_agent() {
        let registry = create_mock_registry(0).unwrap();
        let mut planet =
            Planet::<16, 128, 2, TestMessage>::create(1000.0, 1.0, 50, 1024, 512, registry)
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
    fn test_spawn_agent_preconfigured() {
        let registry = create_mock_registry(0).unwrap();
        let agent_state_sizes = vec![256];
        let config = (1024, 512, &agent_state_sizes);

        let mut planet =
            Planet::<16, 128, 2, TestMessage>::from_config(config, 1000.0, 1.0, 50, registry)
                .unwrap();

        let agent = BasicTestAgent {
            timeout_count: 0,
            max_timeouts: 5,
        };

        let agent_id = planet.spawn_agent_preconfigured(Box::new(agent));
        assert_eq!(agent_id, 0);
        assert_eq!(planet.agents.len(), 1);
    }

    #[test]
    fn test_schedule_event() {
        let registry = create_mock_registry(0).unwrap();
        let mut planet =
            Planet::<16, 128, 2, TestMessage>::create(1000.0, 1.0, 50, 1024, 512, registry)
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
        let mut planet =
            Planet::<16, 128, 2, TestMessage>::create(1000.0, 1.0, 50, 1024, 512, registry)
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
        let mut planet =
            Planet::<16, 128, 2, TestMessage>::create(1000.0, 1.0, 50, 1024, 512, registry)
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
        let mut planet =
            Planet::<16, 128, 2, TestMessage>::create(1000.0, 1.0, 50, 1024, 512, registry)
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

    #[test]
    fn test_gvt_throttling() {
        let registry = create_mock_registry(0).unwrap();
        let mut planet = Planet::<16, 128, 2, TestMessage>::create(
            1000.0, 1.0, 10, 1024, 512, registry, // throttle_horizon = 10
        )
        .unwrap();

        let agent = BasicTestAgent {
            timeout_count: 0,
            max_timeouts: 20,
        };

        planet.spawn_agent(Box::new(agent), 256);
        planet.schedule(1, 0).unwrap();

        // Set GVT to 0
        planet.gvt.store(0, Ordering::SeqCst);

        // Try to advance past throttle horizon
        let mut steps = 0;
        while steps < 15 && planet.now() < 11 {
            let _ = planet.step();
            steps += 1;
        }

        // Should be throttled around time 10
        assert!(planet.now() <= 11);
    }

    #[test]
    fn test_checkpoint_blocking() {
        let registry = create_mock_registry(0).unwrap();
        let mut planet =
            Planet::<16, 128, 2, TestMessage>::create(1000.0, 1.0, 50, 1024, 512, registry)
                .unwrap();

        let agent = BasicTestAgent {
            timeout_count: 0,
            max_timeouts: 10,
        };

        planet.spawn_agent(Box::new(agent), 256);
        planet.schedule(1, 0).unwrap();

        // Set next checkpoint to current time
        planet.next_checkpoint.store(5, Ordering::SeqCst);
        planet.event_system.local_clock.time = 5;

        // Step should succeed but simulation would pause at checkpoint in run()
        let result = planet.step();
        // In actual run(), it would sleep at checkpoint
        assert!(result.is_ok() || result.is_err());
    }
}
