use std::time::Instant;

use aika::worlds::*;
use aika::TestAgent;

fn main() {
    let duration_secs = 20000000;
    let timestep = 1.0;
    let terminal = Some(duration_secs as f64);

    // minimal config world, no logs, no mail, no live for base processing speed benchmark
    let config = Config::new(timestep, terminal, 10, 10, false);
    let mut world = World::<128, 1>::create(config, None);

    let agent = TestAgent::new(0, format!("Test{}", 0));
    world.spawn(Box::new(agent));
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
    // for testing real-time run command line features like pause, resume, and speed up and slow down
    // just type in the terminal: cargo run --example realtime
    // and then type the commands: pause, resume, speed 2.0, speed 0.5 or whatever floating point speed you want (there are limits to how accurately the simulator can run in real-time depending on the hardware)
}
