use aika::worlds::*;
use aika::TestAgent;

#[tokio::main]
async fn main() {
    let config = Config::new(1.0, Some(2000000.0), 100, 100, false, false, false, false);
    let mut world = World::<256, 1>::create(config);
    let agent_test = TestAgent::new(0, "Test".to_string());
    world.spawn(Box::new(agent_test));
    world.schedule(0.0, 0).unwrap();
    world.run().await.unwrap();
    // for testing real-time run command line features like pause, resume, and speed up and slow down
    // just type in the terminal: cargo run --example realtime
    // and then type the commands: pause, resume, speed 2.0, speed 0.5 or whatever floating point speed you want (there are limits to how accurately the simulator can run in real-time depending on the hardware)
}
