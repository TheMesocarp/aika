use std::time::Instant;

use aika::prelude::*;

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

fn main() {
    let duration_secs = 20000000;
    let timestep = 1.0;
    let terminal = Some(duration_secs as f64);

    // minimal config world, no logs, no mail, no live for base processing speed benchmark
    let config = Config::new(timestep, terminal, 10, 10, true, false);
    let mut world = World::<2048, 128, 1>::create::<()>(config, None);

    let agent = TestAgent::new(0);
    world.spawn::<()>(Box::new(agent));
    world.schedule(0, 0).unwrap();

    let start = Instant::now();
    world.run().unwrap();
    let elapsed = start.elapsed();

    let total_steps = world.step_counter();

    println!("Benchmark Results:");
    println!("Total time: {:.2?}", elapsed);
    println!("Total events processed: {}", total_steps);
    println!(
        "Events per second: {:.2}",
        total_steps as f64 / elapsed.as_secs_f64()
    );
    println!(
        "Average event processing time: {:.3?} per event",
        elapsed / total_steps as u32
    );
}
