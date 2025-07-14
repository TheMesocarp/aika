// benches/hybrid_engine_benchmark.rs

use criterion::{criterion_group, criterion_main, Criterion};

// Import necessary modules from your crate
// Ensure these paths are correct relative to your project structure
use aika::{
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

impl ThreadedAgent<16, TestData> for SimpleSchedulingAgent {
    fn step(&mut self, context: &mut PlanetContext<16, TestData>, agent_id: usize) -> Event {
        let time = context.time;
        // Just timeout for 1 time unit
        Event::new(time, time, agent_id, Action::Timeout(1))
    }

    fn read_message(
        &mut self,
        _context: &mut PlanetContext<16, TestData>,
        _msg: Msg<TestData>,
        _agent_id: usize,
    ) {
        // Simple agent doesn't process messages
    }
}

// Define the benchmark function
fn hybrid_engine_benchmark(c: &mut Criterion) {
    // Configuration constants
    const NUM_PLANETS: usize = 7;
    const AGENTS_PER_PLANET: usize = 100;
    const TOTAL_AGENTS: usize = NUM_PLANETS * AGENTS_PER_PLANET;
    const EVENTS: u64 = 1000000; // Total simulation time

    let mut group = c.benchmark_group("HybridEngineRun");

    group.sample_size(10); // Adjust sample size as needed for stable results

    group.bench_function(
        format!("run_simulation_planets_{NUM_PLANETS}_agents_{TOTAL_AGENTS}_events_{EVENTS}"),
        |b| {
            // The setup code that should run once per benchmark iteration,
            // but outside the timed loop.
            // We need to recreate the engine and agents for each iteration
            // to ensure a fresh state for accurate benchmarking.

            // Create configuration
            let config = HybridConfig::new(NUM_PLANETS, 16) // 512 bytes for anti-message arena
                .with_time_bounds(EVENTS as f64, 1.0) // terminal=1000, timestep=1.0
                .with_optimistic_sync(50, 100) // throttle_horizon=50, checkpoint_frequency=100
                .with_uniform_worlds(
                    16, // world state arena size
                    AGENTS_PER_PLANET,
                    16, // agent state arena size
                );

            // Validate config (can be done once outside the bench_function if desired,
            // but doing it here ensures it's part of the setup cost for the benchmark)
            assert!(config.validate().is_ok());
            assert_eq!(config.total_agents(), TOTAL_AGENTS);

            // The `b.iter` closure contains the code to be benchmarked.
            // This code will be run multiple times by Criterion.
            b.iter(|| {
                // Create the hybrid engine
                let mut engine =
                    HybridEngine::<16, 32, 8, 128, 1, TestData>::create(config.clone()).unwrap(); // config.clone() is important here

                // Spawn agents using autobalancing
                for _i in 0..TOTAL_AGENTS {
                    let agent = SimpleSchedulingAgent::new();
                    engine.spawn_agent_autobalance(Box::new(agent)).unwrap();
                }

                // Schedule initial events for each planet
                for planet_id in 0..NUM_PLANETS {
                    for agent_id in 0..100 {
                        let _ = engine.schedule(planet_id, agent_id, 1);
                    }
                }

                // Run the simulation - this is the core part we want to measure
                let result = engine.run();

                // Basic verification that the simulation ran successfully
                // This assertion will run on every iteration, but it's crucial
                // to ensure the benchmark is measuring a successful run.
                assert!(
                    result.is_ok(),
                    "Hybrid engine run failed during benchmark: {:?}",
                    result.err()
                );

                // You might choose to do more extensive verification outside the `b.iter`
                // if it's too expensive to run on every iteration.
                // For now, we'll just ensure the run completed.
            });
        },
    );

    group.finish();
}

// Register the benchmark functions
criterion_group!(benches, hybrid_engine_benchmark);
criterion_main!(benches);
