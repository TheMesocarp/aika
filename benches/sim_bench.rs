use aika::prelude::*;
use criterion::{criterion_group, criterion_main, Criterion};
use std::hint::black_box;

pub struct TestAgent {
    pub id: usize,
}

impl TestAgent {
    pub fn new(id: usize) -> Self {
        TestAgent { id }
    }
}

impl Agent for TestAgent {
    fn step(&mut self, time: &u64, _supports: Supports) -> Event {
        Event::new(*time, *time, self.id, Action::Timeout(1))
    }
}

impl LogicalProcess for TestAgent {
    fn step(&mut self, time: &u64, _state: &mut Lumi) -> Event {
        Event::new(*time, *time, self.id, Action::Timeout(1))
    }
    fn process_message(&mut self, msg: Message, time: u64, _state: &mut Lumi) -> HandlerOutput {
        HandlerOutput::Messages(Annihilator(
            Message {
                data: msg.data,
                sent: time,
                received: time + 19,
                from: msg.to,
                to: msg.from,
            },
            AntiMessage {
                sent: time,
                received: time + 19,
                from: msg.to,
                to: msg.from,
            },
        ))
    }
}

fn run_sim(id: usize, config: Config) {
    let agent = TestAgent::new(id);
    let mut world = World::<256, 256, 1>::create::<()>(config, None);

    world.spawn::<()>(Box::new(agent));
    world.schedule(0, id).unwrap();

    world.run().unwrap();
}

fn sim_bench(c: &mut Criterion) {
    let duration_secs = 40000000;
    let timestep = 1.0;
    let terminal = Some(duration_secs as f64);

    // minimal config world, no logs, no mail, no live for base processing speed benchmark
    let config = Config::new(timestep, terminal, 1000, 1000, true, false);

    c.bench_function("run_sim", |b| {
        b.iter(|| run_sim(black_box(0), black_box(config.clone())));
    });
}

criterion_group!(benches, sim_bench);

criterion_main!(benches);
