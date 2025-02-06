use aika::{
    worlds::{Config, World},
    TestAgent,
};
use criterion::{criterion_group, criterion_main, Criterion};
use std::hint::black_box;

fn run_sim(id: usize, config: Config) {
    let mut world = World::<256, 1>::create(config);
    let agent = TestAgent::new(id, format!("Test{}", id));

    world.spawn(Box::new(agent));
    world.schedule(0.0, id).unwrap();

    world.run().unwrap();
}

fn sim_bench(c: &mut Criterion) {
    let duration_secs = 20000000;
    let timestep = 1.0;
    let terminal = Some(duration_secs as f64);

    // minimal config world, no logs, no mail, no live for base processing speed benchmark
    let config = Config::new(timestep, terminal, 1000, 1000, false);

    c.bench_function("run_sim", |b| {
        b.iter(|| run_sim(black_box(0), black_box(config.clone())));
    });
}

criterion_group!(benches, sim_bench);

criterion_main!(benches);
