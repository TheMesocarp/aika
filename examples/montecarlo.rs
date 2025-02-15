#![allow(dead_code, unused_variables)]
use aika::prelude::*;
use rand::rng;
use rand_distr::{Distribution, Normal};
use std::ffi::c_void;

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
    real_logs: States<f64>,
}

impl Agent for MCAgent {
    fn step<'a>(
        &mut self,
        state: &mut Option<*mut c_void>,
        step: &u64,
        supports: Supports<'a>,
    ) -> Event {
        let history = match supports {
            Supports::Both(_, logger) => logger,
            _ => panic!("Expected logger"),
        };
        let new = gbm_next_step(self.current_value, self.drift, self.volatility, self.dt);
        let old_ptr = &mut self.current_value as *mut f64 as *mut c_void;
        //update(history,     &mut self.real_logs, old_ptr, new, step);
        Event::new(*step, *step + 1, self.id, Action::Timeout(1))
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
            real_logs: States(Vec::new()),
            current_value: initial_value,
            serialized,
        }
    }
}

fn main() {
    let ts = 1.0;
    let config = Config::new(ts, Some(20000000.0), 10, 10, true);
    let mut world = aika::worlds::World::<128, 1>::create(config, None);
    let agent = MCAgent::new(0, "Test".to_string(), 0.1, 0.2, ts, 100.0);
    world.spawn(Box::new(agent));
    world.schedule(0, 0).unwrap();
    let start = std::time::Instant::now();
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
    println!("logger size: {}", world.logger.unwrap().astates[0].0.len());
}
