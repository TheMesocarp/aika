use aika::{prelude::*, TestAgent};
use criterion::{criterion_group, criterion_main, Criterion};
use std::hint::black_box;

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
