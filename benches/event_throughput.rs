use aika::{
    agents::{Agent, WorldContext},
    objects::{Action, Event, Msg},
    st::World,
};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

// Minimal agent that just schedules timeout events
struct ThroughputAgent {
    _id: usize,
    remaining_steps: usize,
}

impl ThroughputAgent {
    fn new(_id: usize, total_steps: usize) -> Self {
        ThroughputAgent {
            _id,
            remaining_steps: total_steps,
        }
    }
}

impl Agent<8, Msg<()>> for ThroughputAgent {
    fn step(&mut self, context: &mut WorldContext<8, Msg<()>>, id: usize) -> Event {
        let time = context.time;

        if self.remaining_steps > 0 {
            self.remaining_steps -= 1;
            // Just timeout for 1 step - minimal work
            Event::new(time, time, id, Action::Timeout(1))
        } else {
            // Stop scheduling once we've done enough steps
            Event::new(time, time, id, Action::Wait)
        }
    }
}

fn bench_event_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("event_throughput");

    // Test different numbers of agents
    for num_agents in [1, 10, 100, 1000].iter() {
        group.bench_with_input(
            BenchmarkId::new("agents", num_agents),
            num_agents,
            |b, &num_agents| {
                b.iter_with_setup(
                    || {
                        // Setup: Create world and agents
                        let mut world = World::<8, 128, 1, ()>::init(1000.0, 1.0, 0).unwrap();

                        // Spawn agents
                        for i in 0..num_agents {
                            let agent = ThroughputAgent::new(i, 1000);
                            world.spawn_agent(Box::new(agent));
                        }

                        // Initialize support layers (mailbox, etc)
                        world.init_support_layers(None).unwrap();

                        // Schedule initial events for all agents
                        for i in 0..num_agents {
                            world.schedule(1, i).unwrap();
                        }

                        world
                    },
                    |mut world| {
                        // Benchmark: Run the simulation
                        world.run().unwrap();
                        black_box(());
                    },
                );
            },
        );
    }

    group.finish();
}

fn bench_event_throughput_fixed_time(c: &mut Criterion) {
    let mut group = c.benchmark_group("event_throughput_fixed");

    // Fix simulation time, vary number of agents
    let sim_time = 10000.0;

    for num_agents in [1, 10, 100].iter() {
        group.bench_with_input(
            BenchmarkId::new("agents", num_agents),
            num_agents,
            |b, &num_agents| {
                b.iter_with_setup(
                    || {
                        let mut world = World::<8, 128, 1, ()>::init(sim_time, 1.0, 0).unwrap();

                        for i in 0..num_agents {
                            let agent = ThroughputAgent::new(i, sim_time as usize);
                            world.spawn_agent(Box::new(agent));
                        }

                        world.init_support_layers(None).unwrap();

                        for i in 0..num_agents {
                            world.schedule(1, i).unwrap();
                        }

                        world
                    },
                    |mut world| {
                        world.run().unwrap();
                        black_box(());
                    },
                );
            },
        );
    }

    group.finish();
}

fn bench_single_agent_long_run(c: &mut Criterion) {
    let mut group = c.benchmark_group("single_agent_throughput");
    group.sample_size(10); // Reduce sample size for long runs

    // Test how many events a single agent can process
    for sim_time in [10000.0, 100000.0, 1000000.0].iter() {
        group.bench_with_input(
            BenchmarkId::new("sim_time", sim_time),
            sim_time,
            |b, &sim_time| {
                b.iter_with_setup(
                    || {
                        let mut world = World::<8, 128, 1, ()>::init(sim_time, 1.0, 0).unwrap();
                        let agent = ThroughputAgent::new(0, sim_time as usize);
                        world.spawn_agent(Box::new(agent));
                        world.init_support_layers(None).unwrap();
                        world.schedule(1, 0).unwrap();
                        world
                    },
                    |mut world| {
                        world.run().unwrap();
                        black_box(());
                    },
                );
            },
        );
    }

    group.finish();
}

// Custom benchmark that reports events per second
fn bench_events_per_second(c: &mut Criterion) {
    let mut group = c.benchmark_group("events_per_second");

    // Fixed time window
    let sim_time = 1000000.0;

    for &num_agents in [1, 10, 100].iter() {
        let total_events = sim_time as u64 * num_agents as u64; // Each agent generates 1 event per time step

        // Inform Criterion of the number of events to be processed.
        group.throughput(Throughput::Elements(total_events));

        group.bench_with_input(
            BenchmarkId::new("agents", num_agents),
            &num_agents,
            |b, &num_agents| {
                b.iter_with_setup(
                    || {
                        // The setup remains the same
                        let mut world = World::<8, 128, 1, ()>::init(sim_time, 1.0, 0).unwrap();
                        for i in 0..num_agents {
                            let agent = ThroughputAgent::new(i, sim_time as usize);
                            world.spawn_agent(Box::new(agent));
                        }
                        world.init_support_layers(None).unwrap();
                        for i in 0..num_agents {
                            world.schedule(1, i).unwrap();
                        }
                        world
                    },
                    |mut world| {
                        // The core benchmarking logic is simplified
                        world.run().unwrap();
                        // We still use black_box to prevent the compiler from optimizing
                        // away the world.run() call if it has no observable side effects.
                        black_box(());
                    },
                );
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_event_throughput,
    bench_event_throughput_fixed_time,
    bench_single_agent_long_run,
    bench_events_per_second
);
criterion_main!(benches);
