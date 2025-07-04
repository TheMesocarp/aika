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
        event::{Action, Event},
        messages::Msg,
        mt::hybrid::{config::HybridConfig, HybridEngine},
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
