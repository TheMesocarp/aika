use aika::{actors::{Actor, ConnectedActor}, env::Stateless, mt::engines::hlocal::Config, prelude::SchedulingTask, stager, AikaError};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::hint::black_box;

#[derive(Debug)]
struct Bencher;

impl Actor<()> for Bencher {
    fn step(&mut self, _env: &mut aika::actors::Context<()>, _actor_id: usize)
        -> Result<SchedulingTask, AikaError> {
        Ok(SchedulingTask::Timeout(1))
    }
}

impl ConnectedActor<()> for Bencher {
    fn read_message(
        &mut self,
        _env: &mut aika::actors::Context<()>,
        _msg: aika::prelude::Msg<()>,
        _actor_id: usize,
    ) -> Result<(), AikaError> {
        Ok(())
    }
}

// Custom benchmark that reports events per second
fn bench_events_per_second(c: &mut Criterion) {
    let mut group = c.benchmark_group("events_per_second");

    // Fixed time window
    let sim_time = 10000000;

    for &num_agents in [1, 10, 100].iter() {
        let total_events = 7 * sim_time * num_agents as u64;

        group.throughput(Throughput::Elements(total_events));

        group.bench_with_input(
            BenchmarkId::new("agents", num_agents),
            &num_agents,
            |b, num_agents| {
                b.iter_with_setup(
                    || {
                        let mut stager = stager!((), BLOCK_BW = 512).unwrap();
                        stager.config(Config::new(7, 12, 64, sim_time, 100)).unwrap();

                        for _ in 0..7 {
                            stager.create_cluster(Stateless).unwrap();
                        }
                        for i in 0..7 {
                            for _ in 0..*num_agents {
                                stager.spawn_actor_on_cluster(i, Bencher).unwrap();
                            }
                        }
                        stager.schedule_all(1).unwrap();
                        stager
                    },
                    |stager| {
                        stager.run(aika::mt::RunMode::Fast).unwrap();
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
    bench_events_per_second
);
criterion_main!(benches);