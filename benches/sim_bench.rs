use aika::{
    worlds::{Config, World},
    TestAgent,
};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use std::hint::black_box;
use tokio::runtime::Runtime;

async fn run_sim(id: usize, config: Config) {
    let mut world = World::<256, 1>::create(config);
    let agent = TestAgent::new(id, format!("Test{}", id));
    world.spawn(Box::new(agent));
    world.schedule(0.0, id).unwrap();

    world.run().await.unwrap();
}

fn sim_bench(c: &mut Criterion) {
    let duration_secs = 20000000;
    let timestep = 1.0;
    let terminal = Some(duration_secs as f64);

    // minimal config world, no logs, no mail, no live for base processing speed benchmark
    let config = Config::new(timestep, terminal, 1000, 1000, false, false, false, false);

    let id: usize = 0;

    c.bench_with_input(BenchmarkId::new("run_sim", id), &id, |b, &i| {
        b.to_async(Runtime::new().unwrap())
            .iter(|| run_sim(black_box(i), black_box(config.clone())));
    });
}

criterion_group!(benches, sim_bench);

criterion_main!(benches);
