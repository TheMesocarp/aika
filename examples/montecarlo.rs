#![allow(dead_code, unused_variables)]
use aika::{
    worlds::{Action, Agent, Config, Event},
    TestAgent,
};
use rand::rng;
use rand_distr::{Distribution, Normal};

pub fn gbm_next_step(current_value: f64, drift: f64, volatility: f64, dt: f64) -> f64 {
    let normal = Normal::new(0.0, 1.0).unwrap();
    let mut rng = rng();
    let z = normal.sample(&mut rng);
    let exponent = (drift - 0.5 * volatility.powi(2)) * dt + volatility * dt.sqrt() * z;
    current_value * exponent.exp()
}

struct MCAgent {
    id: usize,
    name: String,
    drift: f64,
    volatility: f64,
    dt: f64,
    current_value: f64,
    serialized: [u8; 8],
}

impl Agent for MCAgent {
    fn step<'a>(
        &mut self,
        state: &mut Option<Vec<u8>>,
        time: &f64,
        mailbox: &mut Option<aika::worlds::Mailbox>,
    ) -> Event {
        self.current_value =
            gbm_next_step(self.current_value, self.drift, self.volatility, self.dt);
        self.serialized = self.current_value.to_be_bytes();

        Event::new(*time, self.id, Action::Timeout(1.0))
    }

    fn get_state(&self) -> Option<&[u8]> {
        Some(&self.serialized)
    }
}

impl MCAgent {
    pub fn new(
        id: usize,
        name: String,
        drift: f64,
        volatility: f64,
        dt: f64,
        initial_value: f64,
    ) -> Self {
        let serialized = initial_value.to_be_bytes();
        MCAgent {
            id,
            name,
            drift,
            volatility,
            dt,
            current_value: initial_value,
            serialized,
        }
    }
}

#[tokio::main]
async fn main() {
    let ts = 1.0;
    let config = Config::new(ts, Some(19000000.0), 10, 10, false, true, false);
    let mut world = aika::worlds::World::<128, 1>::create(config);
    let agent = MCAgent::new(0, "Test".to_string(), 0.1, 0.2, ts, 100.0);
    let agent1 = TestAgent::new(1, "Test1".to_string());
    world.spawn(Box::new(agent));
    world.spawn(Box::new(agent1));
    //world.schedule(0.0, 0).unwrap();
    let start = std::time::Instant::now();
    world.run().await.unwrap();
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
    println!("logger size: {}", world.logger.get_snapshots().len());
}
