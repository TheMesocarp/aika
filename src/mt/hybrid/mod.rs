//! `aika::mt::hybrid` contains the infrastructure for running hybrid synchronization

use bytemuck::{Pod, Zeroable};

use crate::{
    agents::ThreadedAgent,
    mt::hybrid::{config::HybridConfig, galaxy::Galaxy, planet::Planet},
    SimError,
};

pub mod config;
pub mod galaxy;
pub mod planet;

pub struct HybridEngine<
    const INTER_SLOTS: usize,
    const CLOCK_SLOTS: usize,
    const CLOCK_HEIGHT: usize,
    MessageType: Pod + Zeroable + Clone,
> {
    pub galaxy: Galaxy<INTER_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>,
    pub planets: Vec<Planet<INTER_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>>,
    pub config: HybridConfig,
}

impl<
        const INTER_SLOTS: usize,
        const CLOCK_SLOTS: usize,
        const CLOCK_HEIGHT: usize,
        MessageType: Pod + Zeroable + Clone,
    > HybridEngine<INTER_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
    pub fn create(config: HybridConfig) -> Result<Self, SimError> {
        let mut galaxy = Galaxy::new(
            config.number_of_worlds,
            config.throttle_horizon,
            config.checkpoint_frequency,
            config.terminal,
            config.timestep,
        )?;
        let mut planets = Vec::new();
        for i in 0..config.number_of_worlds {
            let registry = galaxy.spawn_world()?;
            let planet = Planet::from_config(
                config.world_config(i)?,
                config.terminal,
                config.timestep,
                config.throttle_horizon,
                registry,
            )?;
            planets.push(planet);
        }
        Ok(Self {
            galaxy,
            planets,
            config,
        })
    }

    pub fn spawn_agent(
        &mut self,
        planet_id: usize,
        agent: Box<dyn ThreadedAgent<INTER_SLOTS, MessageType>>,
    ) -> Result<(), SimError> {
        if planet_id >= self.planets.len() {
            return Err(SimError::InvalidWorldId(planet_id));
        }
        self.planets[planet_id].spawn_agent_preconfigured(agent);
        Ok(())
    }

    pub fn spawn_agent_autobalance(
        &mut self,
        agent: Box<dyn ThreadedAgent<INTER_SLOTS, MessageType>>,
    ) -> Result<(), SimError> {
        let mut lowest = (usize::MAX, usize::MAX);
        for (i, planet) in self.planets.iter().enumerate() {
            let count = planet.agents.len();
            if count < lowest.1 {
                lowest = (i, count)
            }
        }
        self.planets[lowest.0].spawn_agent_preconfigured(agent);
        Ok(())
    }

    pub fn schedule(
        &mut self,
        planet_id: usize,
        agent_id: usize,
        time: u64,
    ) -> Result<(), SimError> {
        if planet_id >= self.planets.len() {
            return Err(SimError::InvalidWorldId(planet_id));
        }
        self.planets[planet_id].schedule(time, agent_id)
    }

    pub fn run(self) -> Result<Self, SimError> {
        let HybridEngine {
            galaxy,
            planets,
            config,
        } = self;
        let galaxy_handle = std::thread::spawn(move || {
            let mut galaxy = galaxy;
            galaxy.gvt_daemon().map(|_| galaxy)
        });

        let mut planet_handles = Vec::new();
        for planet in planets {
            let handle = std::thread::spawn(move || {
                let mut planet = planet;
                planet.run().map(|_| planet)
            });
            planet_handles.push(handle);
        }
        let mut final_planets = Vec::new();
        for handle in planet_handles {
            let planet = handle.join().map_err(|_| SimError::ThreadPanic)??;
            final_planets.push(planet);
        }
        let final_galaxy = galaxy_handle.join().map_err(|_| SimError::ThreadPanic)??;
        Ok(Self {
            galaxy: final_galaxy,
            planets: final_planets,
            config,
        })
    }
}

#[cfg(test)]
mod hybrid_engine_tests {
    use crate::{
        agents::{PlanetContext, ThreadedAgent},
        mt::hybrid::{config::HybridConfig, HybridEngine},
        objects::{Action, Event, Msg},
    };
    use bytemuck::{Pod, Zeroable};

    // Simple test message type
    #[derive(Copy, Clone, Debug, PartialEq)]
    #[repr(C)]
    struct TestData {
        value: u8,
    }

    unsafe impl Pod for TestData {}
    unsafe impl Zeroable for TestData {}

    // Simple agent that just schedules timeouts (similar to st::World TestAgent)
    struct SimpleSchedulingAgent {}

    impl SimpleSchedulingAgent {
        pub fn new() -> Self {
            SimpleSchedulingAgent {}
        }
    }

    impl ThreadedAgent<128, TestData> for SimpleSchedulingAgent {
        fn step(&mut self, context: &mut PlanetContext<128, TestData>, agent_id: usize) -> Event {
            let time = context.time;
            // Just timeout for 1 time unit
            Event::new(time, time, agent_id, Action::Timeout(1))
        }

        fn read_message(
            &mut self,
            _context: &mut PlanetContext<128, TestData>,
            _msg: Msg<TestData>,
            _agent_id: usize,
        ) {
            // Simple agent doesn't process messages
        }
    }

    #[test]
    fn test_hybrid_engine_basic_run() {
        // Configuration
        const NUM_PLANETS: usize = 14;
        const AGENTS_PER_PLANET: usize = 100;
        const TOTAL_AGENTS: usize = NUM_PLANETS * AGENTS_PER_PLANET;
        const EVENTS: u64 = 100000;
        // Create configuration
        let config = HybridConfig::new(NUM_PLANETS, 16) // 512 bytes for anti-message arena
            .with_time_bounds(EVENTS as f64, 1.0) // terminal=1000, timestep=1.0
            .with_optimistic_sync(50, 100) // throttle_horizon=50, checkpoint_frequency=100
            .with_uniform_worlds(
                16, // world state arena size
                AGENTS_PER_PLANET,
                16, // agent state arena size
            );

        // Validate config
        assert!(config.validate().is_ok());
        assert_eq!(config.total_agents(), TOTAL_AGENTS);

        // Create the hybrid engine
        let mut engine = HybridEngine::<128, 128, 1, TestData>::create(config).unwrap();

        // Spawn agents using autobalancing
        for _i in 0..TOTAL_AGENTS {
            let agent = SimpleSchedulingAgent::new();
            engine.spawn_agent_autobalance(Box::new(agent)).unwrap();
        }

        // Schedule initial events for each planet
        // Each planet should have approximately AGENTS_PER_PLANET agents due to autobalancing
        for planet_id in 0..NUM_PLANETS {
            // Schedule first few agents in each planet to start at time 1
            for agent_id in 0..10 {
                // Just schedule first 5 agents per planet
                let _ = engine.schedule(planet_id, agent_id, 1);
            }
        }

        // Run the simulation
        println!(
            "Starting hybrid simulation with {NUM_PLANETS} planets and {TOTAL_AGENTS} total agents"
        );

        let result = engine.run();

        // Verify the simulation ran successfully
        assert!(
            result.is_ok(),
            "Hybrid engine run failed: {:?}",
            result.err()
        );

        let final_engine = result.unwrap();

        // Basic verification that the simulation progressed
        println!("Simulation completed successfully");

        // Verify we still have all our planets
        assert_eq!(final_engine.planets.len(), NUM_PLANETS);

        // Verify agents are distributed (autobalancing should give roughly equal distribution)
        let mut total_agent_count = 0;
        for planet in final_engine.planets.iter() {
            let agent_count = planet.agents.len();
            total_agent_count += agent_count;
            //println!("Planet {} has {} agents", i, agent_count);

            // With perfect autobalancing, each should have AGENTS_PER_PLANET
            // Allow some variance due to integer division
            assert!(agent_count >= AGENTS_PER_PLANET - 1);
            assert!(agent_count <= AGENTS_PER_PLANET + 1);
        }

        assert_eq!(
            total_agent_count, TOTAL_AGENTS,
            "Total agent count mismatch"
        );

        println!(
            "Test passed: {TOTAL_AGENTS} agents distributed across {NUM_PLANETS} planets, with {EVENTS} events per agent"
        );
    }
}

#[cfg(test)]
mod inter_planetary_message_tests {
    use crate::{
        agents::{PlanetContext, ThreadedAgent},
        mt::hybrid::{config::HybridConfig, HybridEngine},
        objects::{Action, Event, Msg},
    };
    use bytemuck::{Pod, Zeroable};
    use std::sync::{Arc, Mutex};

    // Test message type with more data
    #[derive(Copy, Clone, Debug, PartialEq)]
    #[repr(C)]
    struct InterPlanetaryMessage {
        value: u32,
        sender_planet: u32,
        sender_agent: u32,
        target_planet: u32,
        target_agent: u32,
    }

    unsafe impl Pod for InterPlanetaryMessage {}
    unsafe impl Zeroable for InterPlanetaryMessage {}

    // Shared state for tracking received messages
    type MessageLog = Arc<Mutex<Vec<(usize, usize, InterPlanetaryMessage)>>>; // (planet_id, agent_id, message)

    // Agent that sends messages to agents on other planets
    struct InterPlanetarySender {
        planet_id: usize,
        agent_id: usize,
        target_planet: usize,
        target_agent: usize,
        messages_to_send: usize,
        messages_sent: usize,
        send_interval: u64,
    }

    impl InterPlanetarySender {
        fn new(
            planet_id: usize,
            agent_id: usize,
            target_planet: usize,
            target_agent: usize,
            messages_to_send: usize,
            send_interval: u64,
        ) -> Self {
            Self {
                planet_id,
                agent_id,
                target_planet,
                target_agent,
                messages_to_send,
                messages_sent: 0,
                send_interval,
            }
        }
    }

    impl ThreadedAgent<128, InterPlanetaryMessage> for InterPlanetarySender {
        fn step(
            &mut self,
            context: &mut PlanetContext<128, InterPlanetaryMessage>,
            agent_id: usize,
        ) -> Event {
            let time = context.time;

            // Send message if we haven't sent all yet
            if self.messages_sent < self.messages_to_send {
                let message_data = InterPlanetaryMessage {
                    value: self.messages_sent as u32,
                    sender_planet: self.planet_id as u32,
                    sender_agent: self.agent_id as u32,
                    target_planet: self.target_planet as u32,
                    target_agent: self.target_agent as u32,
                };

                let msg = Msg::new(
                    message_data,
                    time,                    // sent time
                    time + 20,               // receive time (delayed)
                    agent_id,                // from agent
                    Some(self.target_agent), // to specific agent
                );

                // Send to another planet
                let result = context.send_mail(msg, self.target_planet);
                if result.is_ok() {
                    self.messages_sent += 1;
                    println!(
                        "Planet {} Agent {} sent message {} to Planet {} Agent {}",
                        self.planet_id,
                        self.agent_id,
                        self.messages_sent - 1,
                        self.target_planet,
                        self.target_agent
                    );
                }
            }

            // Continue sending at intervals
            if self.messages_sent < self.messages_to_send {
                Event::new(time, time, agent_id, Action::Timeout(self.send_interval))
            } else {
                Event::new(time, time, agent_id, Action::Timeout(100)) // Keep alive
            }
        }

        fn read_message(
            &mut self,
            _context: &mut PlanetContext<128, InterPlanetaryMessage>,
            _msg: Msg<InterPlanetaryMessage>,
            _agent_id: usize,
        ) {
            // Sender doesn't process incoming messages
        }
    }

    // Agent that receives and logs messages
    struct InterPlanetaryReceiver {
        planet_id: usize,
        agent_id: usize,
        message_log: MessageLog,
    }

    impl InterPlanetaryReceiver {
        fn new(planet_id: usize, agent_id: usize, message_log: MessageLog) -> Self {
            Self {
                planet_id,
                agent_id,
                message_log,
            }
        }
    }

    impl ThreadedAgent<128, InterPlanetaryMessage> for InterPlanetaryReceiver {
        fn step(
            &mut self,
            _context: &mut PlanetContext<128, InterPlanetaryMessage>,
            agent_id: usize,
        ) -> Event {
            let time = _context.time;
            // Keep checking for messages
            Event::new(time, time, agent_id, Action::Timeout(1))
        }

        fn read_message(
            &mut self,
            _context: &mut PlanetContext<128, InterPlanetaryMessage>,
            msg: Msg<InterPlanetaryMessage>,
            _agent_id: usize,
        ) {
            println!(
                "Planet {} Agent {} received message with value {} from Planet {} Agent {}",
                self.planet_id,
                self.agent_id,
                msg.data.value,
                msg.data.sender_planet,
                msg.data.sender_agent
            );

            // Log the received message
            if let Ok(mut log) = self.message_log.lock() {
                log.push((self.planet_id, self.agent_id, msg.data));
            }
        }
    }

    // Agent that broadcasts to all agents on other planets
    struct InterPlanetaryBroadcaster {
        planet_id: usize,
        agent_id: usize,
        target_planets: Vec<usize>,
        broadcasts_to_send: usize,
        broadcasts_sent: usize,
    }

    impl InterPlanetaryBroadcaster {
        fn new(
            planet_id: usize,
            agent_id: usize,
            target_planets: Vec<usize>,
            broadcasts_to_send: usize,
        ) -> Self {
            Self {
                planet_id,
                agent_id,
                target_planets,
                broadcasts_to_send,
                broadcasts_sent: 0,
            }
        }
    }

    impl ThreadedAgent<128, InterPlanetaryMessage> for InterPlanetaryBroadcaster {
        fn step(
            &mut self,
            context: &mut PlanetContext<128, InterPlanetaryMessage>,
            agent_id: usize,
        ) -> Event {
            let time = context.time;

            if self.broadcasts_sent < self.broadcasts_to_send {
                let message_data = InterPlanetaryMessage {
                    value: (100 + self.broadcasts_sent) as u32,
                    sender_planet: self.planet_id as u32,
                    sender_agent: self.agent_id as u32,
                    target_planet: u32::MAX, // Broadcast indicator
                    target_agent: u32::MAX,  // Broadcast indicator
                };

                // Send broadcast to each target planet
                for &target_planet in &self.target_planets {
                    let msg = Msg::new(
                        message_data,
                        time,
                        time + 15,
                        agent_id,
                        None, // None means broadcast
                    );

                    let _ = context.send_mail(msg, target_planet);
                }

                self.broadcasts_sent += 1;
                println!(
                    "Planet {} Agent {} broadcasted message {} to planets {:?}",
                    self.planet_id,
                    self.agent_id,
                    self.broadcasts_sent - 1,
                    self.target_planets
                );
            }

            if self.broadcasts_sent < self.broadcasts_to_send {
                Event::new(time, time, agent_id, Action::Timeout(30))
            } else {
                Event::new(time, time, agent_id, Action::Timeout(100))
            }
        }

        fn read_message(
            &mut self,
            _context: &mut PlanetContext<128, InterPlanetaryMessage>,
            _msg: Msg<InterPlanetaryMessage>,
            _agent_id: usize,
        ) {
            // Broadcaster doesn't process messages
        }
    }

    #[test]
    fn test_basic_inter_planetary_messaging() {
        const NUM_PLANETS: usize = 3;
        const TERMINAL_TIME: f64 = 500.0;

        let message_log = Arc::new(Mutex::new(Vec::new()));

        // Create configuration
        let config = HybridConfig::new(NUM_PLANETS, 512)
            .with_time_bounds(TERMINAL_TIME, 1.0)
            .with_optimistic_sync(100, 200)
            .with_uniform_worlds(1024, 2, 256); // 2 agents per planet

        let mut engine =
            HybridEngine::<128, 128, 2, InterPlanetaryMessage>::create(config).unwrap();

        // Planet 0: Sender agent
        let sender = InterPlanetarySender::new(
            0, 0, // planet 0, agent 0
            1, 0,  // target planet 1, agent 0
            5,  // send 5 messages
            10, // every 10 time units
        );
        engine.spawn_agent(0, Box::new(sender)).unwrap();

        // Planet 0: Receiver agent (for any messages sent to it)
        let receiver0 = InterPlanetaryReceiver::new(0, 1, message_log.clone());
        engine.spawn_agent(0, Box::new(receiver0)).unwrap();

        // Planet 1: Receiver agent
        let receiver1 = InterPlanetaryReceiver::new(1, 0, message_log.clone());
        engine.spawn_agent(1, Box::new(receiver1)).unwrap();

        // Planet 1: Another agent
        let receiver1_2 = InterPlanetaryReceiver::new(1, 1, message_log.clone());
        engine.spawn_agent(1, Box::new(receiver1_2)).unwrap();

        // Planet 2: Just receivers
        let receiver2_1 = InterPlanetaryReceiver::new(2, 0, message_log.clone());
        let receiver2_2 = InterPlanetaryReceiver::new(2, 1, message_log.clone());
        engine.spawn_agent(2, Box::new(receiver2_1)).unwrap();
        engine.spawn_agent(2, Box::new(receiver2_2)).unwrap();

        // Schedule initial events
        engine.schedule(0, 0, 1).unwrap(); // Start sender
        for planet in 0..NUM_PLANETS {
            for agent in 0..2 {
                engine.schedule(planet, agent, 1).unwrap();
            }
        }

        // Run simulation
        let result = engine.run();
        assert!(result.is_ok(), "Engine run failed: {:?}", result.err());

        // Verify messages were received
        let log = message_log.lock().unwrap();
        println!("Total messages received: {}", log.len());

        // Should have received 5 messages on planet 1, agent 0
        let planet1_agent0_messages: Vec<_> = log
            .iter()
            .filter(|(planet, agent, _)| *planet == 1 && *agent == 0)
            .collect();

        assert_eq!(
            planet1_agent0_messages.len(),
            5,
            "Planet 1 Agent 0 should have received 5 messages"
        );

        // Verify message content
        for (i, (_, _, msg)) in planet1_agent0_messages.iter().enumerate() {
            assert_eq!(msg.value, i as u32);
            assert_eq!(msg.sender_planet, 0);
            assert_eq!(msg.sender_agent, 0);
            assert_eq!(msg.target_planet, 1);
            assert_eq!(msg.target_agent, 0);
        }
    }

    #[test]
    fn test_inter_planetary_broadcast() {
        const NUM_PLANETS: usize = 4;
        const AGENTS_PER_PLANET: usize = 3;
        const TERMINAL_TIME: f64 = 300.0;

        let message_log = Arc::new(Mutex::new(Vec::new()));

        // Create configuration
        let config = HybridConfig::new(NUM_PLANETS, 512)
            .with_time_bounds(TERMINAL_TIME, 1.0)
            .with_optimistic_sync(100, 200)
            .with_uniform_worlds(1024, AGENTS_PER_PLANET, 256);

        let mut engine =
            HybridEngine::<128, 128, 2, InterPlanetaryMessage>::create(config).unwrap();

        // Planet 0: Broadcaster
        let broadcaster = InterPlanetaryBroadcaster::new(
            0,
            0,             // planet 0, agent 0
            vec![1, 2, 3], // broadcast to planets 1, 2, 3
            3,             // send 3 broadcasts
        );
        engine.spawn_agent(0, Box::new(broadcaster)).unwrap();

        // Add receivers to planet 0
        for agent_id in 1..AGENTS_PER_PLANET {
            let receiver = InterPlanetaryReceiver::new(0, agent_id, message_log.clone());
            engine.spawn_agent(0, Box::new(receiver)).unwrap();
        }

        // Add receivers to other planets
        for planet in 1..NUM_PLANETS {
            for agent_id in 0..AGENTS_PER_PLANET {
                let receiver = InterPlanetaryReceiver::new(planet, agent_id, message_log.clone());
                engine.spawn_agent(planet, Box::new(receiver)).unwrap();
            }
        }

        // Schedule all agents
        for planet in 0..NUM_PLANETS {
            for agent in 0..AGENTS_PER_PLANET {
                engine.schedule(planet, agent, 1).unwrap();
            }
        }

        // Run simulation
        let result = engine.run();
        assert!(result.is_ok(), "Engine run failed: {:?}", result.err());

        // Verify broadcasts were received
        let log = message_log.lock().unwrap();
        println!("Total broadcast messages received: {}", log.len());

        // Each broadcast should be received by all agents on planets 1, 2, 3
        // That's 3 planets * 3 agents per planet * 3 broadcasts = 27 messages
        let broadcast_messages: Vec<_> = log
            .iter()
            .filter(|(planet, _, msg)| *planet > 0 && msg.value >= 100)
            .collect();

        assert_eq!(
            broadcast_messages.len(),
            27,
            "Should have received 27 broadcast messages (3 broadcasts * 3 planets * 3 agents)"
        );

        // Verify each planet received all broadcasts
        for planet in 1..NUM_PLANETS {
            let planet_broadcasts: Vec<_> = log
                .iter()
                .filter(|(p, _, msg)| *p == planet && msg.value >= 100)
                .collect();

            assert_eq!(
                planet_broadcasts.len(),
                9,
                "Planet {planet} should have received 9 broadcast messages (3 broadcasts * 3 agents)"
            );
        }
    }

    #[test]
    fn test_bidirectional_inter_planetary_communication() {
        const NUM_PLANETS: usize = 2;
        const TERMINAL_TIME: f64 = 400.0;

        let message_log = Arc::new(Mutex::new(Vec::new()));

        // Create configuration
        let config = HybridConfig::new(NUM_PLANETS, 512)
            .with_time_bounds(TERMINAL_TIME, 1.0)
            .with_optimistic_sync(100, 200)
            .with_uniform_worlds(1024, 2, 256);

        let mut engine =
            HybridEngine::<128, 128, 2, InterPlanetaryMessage>::create(config).unwrap();

        // Create an agent that can both send and receive
        struct BidirectionalAgent {
            planet_id: usize,
            agent_id: usize,
            target_planet: usize,
            target_agent: usize,
            messages_to_send: usize,
            messages_sent: usize,
            send_interval: u64,
            message_log: MessageLog,
        }

        impl BidirectionalAgent {
            fn new(
                planet_id: usize,
                agent_id: usize,
                target_planet: usize,
                target_agent: usize,
                messages_to_send: usize,
                send_interval: u64,
                message_log: MessageLog,
            ) -> Self {
                Self {
                    planet_id,
                    agent_id,
                    target_planet,
                    target_agent,
                    messages_to_send,
                    messages_sent: 0,
                    send_interval,
                    message_log,
                }
            }
        }

        impl ThreadedAgent<128, InterPlanetaryMessage> for BidirectionalAgent {
            fn step(
                &mut self,
                context: &mut PlanetContext<128, InterPlanetaryMessage>,
                agent_id: usize,
            ) -> Event {
                let time = context.time;

                // Send message if we haven't sent all yet
                if self.messages_sent < self.messages_to_send {
                    let message_data = InterPlanetaryMessage {
                        value: self.messages_sent as u32,
                        sender_planet: self.planet_id as u32,
                        sender_agent: self.agent_id as u32,
                        target_planet: self.target_planet as u32,
                        target_agent: self.target_agent as u32,
                    };

                    let msg = Msg::new(
                        message_data,
                        time,
                        time + 20,
                        agent_id,
                        Some(self.target_agent),
                    );

                    let result = context.send_mail(msg, self.target_planet);
                    if result.is_ok() {
                        self.messages_sent += 1;
                        println!(
                            "Planet {} Agent {} sent message {} to Planet {} Agent {}",
                            self.planet_id,
                            self.agent_id,
                            self.messages_sent - 1,
                            self.target_planet,
                            self.target_agent
                        );
                    }
                }

                if self.messages_sent < self.messages_to_send {
                    Event::new(time, time, agent_id, Action::Timeout(self.send_interval))
                } else {
                    Event::new(time, time, agent_id, Action::Timeout(100))
                }
            }

            fn read_message(
                &mut self,
                _context: &mut PlanetContext<128, InterPlanetaryMessage>,
                msg: Msg<InterPlanetaryMessage>,
                _agent_id: usize,
            ) {
                println!(
                    "Planet {} Agent {} received message with value {} from Planet {} Agent {}",
                    self.planet_id,
                    self.agent_id,
                    msg.data.value,
                    msg.data.sender_planet,
                    msg.data.sender_agent
                );

                // Log the received message
                if let Ok(mut log) = self.message_log.lock() {
                    log.push((self.planet_id, self.agent_id, msg.data));
                }
            }
        }

        // Planet 0 Agent 0: Sends to Planet 1 Agent 0 AND receives
        let agent0_0 = BidirectionalAgent::new(0, 0, 1, 0, 4, 20, message_log.clone());
        engine.spawn_agent(0, Box::new(agent0_0)).unwrap();

        // Planet 0 Agent 1: Just receives
        let receiver0 = InterPlanetaryReceiver::new(0, 1, message_log.clone());
        engine.spawn_agent(0, Box::new(receiver0)).unwrap();

        // Planet 1 Agent 0: Sends to Planet 0 Agent 0 AND receives
        let agent1_0 = BidirectionalAgent::new(1, 0, 0, 0, 4, 25, message_log.clone());
        engine.spawn_agent(1, Box::new(agent1_0)).unwrap();

        // Planet 1 Agent 1: Just receives
        let receiver1 = InterPlanetaryReceiver::new(1, 1, message_log.clone());
        engine.spawn_agent(1, Box::new(receiver1)).unwrap();

        // Schedule all agents
        for planet in 0..NUM_PLANETS {
            for agent in 0..2 {
                engine.schedule(planet, agent, 1).unwrap();
            }
        }

        // Run simulation
        let result = engine.run();
        assert!(result.is_ok(), "Engine run failed: {:?}", result.err());

        // Verify bidirectional communication
        let log = message_log.lock().unwrap();

        // Messages from Planet 0 to Planet 1
        let p0_to_p1: Vec<_> = log
            .iter()
            .filter(|(planet, _, msg)| *planet == 1 && msg.sender_planet == 0)
            .collect();

        // Messages from Planet 1 to Planet 0
        let p1_to_p0: Vec<_> = log
            .iter()
            .filter(|(planet, _, msg)| *planet == 0 && msg.sender_planet == 1)
            .collect();

        println!("Planet 0 -> Planet 1: {} messages", p0_to_p1.len());
        println!("Planet 1 -> Planet 0: {} messages", p1_to_p0.len());

        assert_eq!(
            p0_to_p1.len(),
            4,
            "Should have 4 messages from Planet 0 to Planet 1"
        );
        assert_eq!(
            p1_to_p0.len(),
            4,
            "Should have 4 messages from Planet 1 to Planet 0"
        );
    }

    #[test]
    fn test_inter_planetary_message_ordering() {
        const NUM_PLANETS: usize = 2;
        const TERMINAL_TIME: f64 = 200.0;

        let message_log = Arc::new(Mutex::new(Vec::new()));

        // Create configuration with smaller values for faster test
        let config = HybridConfig::new(NUM_PLANETS, 256)
            .with_time_bounds(TERMINAL_TIME, 1.0)
            .with_optimistic_sync(50, 100)
            .with_uniform_worlds(512, 1, 128);

        let mut engine = HybridEngine::<128, 64, 2, InterPlanetaryMessage>::create(config).unwrap();

        // Rapid-fire sender
        struct RapidSender {
            messages_sent: usize,
        }

        impl ThreadedAgent<128, InterPlanetaryMessage> for RapidSender {
            fn step(
                &mut self,
                context: &mut PlanetContext<128, InterPlanetaryMessage>,
                agent_id: usize,
            ) -> Event {
                let time = context.time;

                // Send multiple messages at once
                for i in 0..3 {
                    let msg_data = InterPlanetaryMessage {
                        value: (self.messages_sent * 3 + i) as u32,
                        sender_planet: 0,
                        sender_agent: 0,
                        target_planet: 1,
                        target_agent: 0,
                    };

                    let msg = Msg::new(
                        msg_data,
                        time,
                        time + 10 + i as u64 * 5, // Staggered receive times
                        agent_id,
                        Some(0),
                    );

                    let _ = context.send_mail(msg, 1);
                }

                self.messages_sent += 1;

                if self.messages_sent < 5 {
                    Event::new(time, time, agent_id, Action::Timeout(15))
                } else {
                    Event::new(time, time, agent_id, Action::Wait)
                }
            }

            fn read_message(
                &mut self,
                _context: &mut PlanetContext<128, InterPlanetaryMessage>,
                _msg: Msg<InterPlanetaryMessage>,
                _agent_id: usize,
            ) {
            }
        }

        let sender = RapidSender { messages_sent: 0 };
        engine.spawn_agent(0, Box::new(sender)).unwrap();

        let receiver = InterPlanetaryReceiver::new(1, 0, message_log.clone());
        engine.spawn_agent(1, Box::new(receiver)).unwrap();

        // Schedule agents
        engine.schedule(0, 0, 1).unwrap();
        engine.schedule(1, 0, 1).unwrap();

        // Run simulation
        let result = engine.run();
        assert!(result.is_ok());

        // Verify message ordering
        let log = message_log.lock().unwrap();
        let received_values: Vec<u32> = log.iter().map(|(_, _, msg)| msg.value).collect();

        println!("Received values: {received_values:?}");

        // Messages should be received in order despite being sent in batches
        for i in 1..received_values.len() {
            assert!(
                received_values[i] >= received_values[i - 1],
                "Messages received out of order: {} came after {}",
                received_values[i - 1],
                received_values[i]
            );
        }
    }

    #[test]
    fn test_inter_planetary_messaging_with_failures() {
        const NUM_PLANETS: usize = 3;
        const TERMINAL_TIME: f64 = 150.0;

        // Agent that tries to send to non-existent planets
        struct FaultySender {
            attempts: usize,
        }

        impl ThreadedAgent<128, InterPlanetaryMessage> for FaultySender {
            fn step(
                &mut self,
                context: &mut PlanetContext<128, InterPlanetaryMessage>,
                agent_id: usize,
            ) -> Event {
                let time = context.time;

                if self.attempts < 3 {
                    let msg_data = InterPlanetaryMessage {
                        value: self.attempts as u32,
                        sender_planet: 0,
                        sender_agent: 0,
                        target_planet: 99, // Non-existent planet
                        target_agent: 0,
                    };

                    let msg = Msg::new(msg_data, time, time + 10, agent_id, Some(0));

                    // This should fail gracefully
                    let result = context.send_mail(msg, 99);
                    if result.is_err() {
                        println!(
                            "Expected error when sending to non-existent planet: {result:?}"
                        );
                    }

                    self.attempts += 1;
                }

                Event::new(time, time, agent_id, Action::Timeout(10))
            }

            fn read_message(
                &mut self,
                _context: &mut PlanetContext<128, InterPlanetaryMessage>,
                _msg: Msg<InterPlanetaryMessage>,
                _agent_id: usize,
            ) {
            }
        }

        let config = HybridConfig::new(NUM_PLANETS, 256)
            .with_time_bounds(TERMINAL_TIME, 1.0)
            .with_optimistic_sync(50, 100)
            .with_uniform_worlds(512, 1, 128);

        let mut engine = HybridEngine::<128, 64, 2, InterPlanetaryMessage>::create(config).unwrap();

        let sender = FaultySender { attempts: 0 };
        engine.spawn_agent(0, Box::new(sender)).unwrap();

        // Add a dummy agent to planet 1
        let message_log = Arc::new(Mutex::new(Vec::new()));
        let receiver = InterPlanetaryReceiver::new(1, 0, message_log);
        engine.spawn_agent(1, Box::new(receiver)).unwrap();

        // Add a dummy agent to planet 2
        let message_log2 = Arc::new(Mutex::new(Vec::new()));
        let receiver2 = InterPlanetaryReceiver::new(2, 0, message_log2);
        engine.spawn_agent(2, Box::new(receiver2)).unwrap();

        engine.schedule(0, 0, 1).unwrap();
        engine.schedule(1, 0, 1).unwrap();
        engine.schedule(2, 0, 1).unwrap();

        // Should run without panicking despite send failures
        let result = engine.run();
        assert!(
            result.is_ok(),
            "Engine should handle send failures gracefully"
        );
    }
}
