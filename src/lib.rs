use worlds::{Action, Agent, Event, Mailbox, Message};

extern crate tokio;

pub mod logger;
pub mod universes;
pub mod worlds;

pub struct TestAgent {
    pub id: usize,
    pub name: String,
}

impl TestAgent {
    pub fn new(id: usize, name: String) -> Self {
        TestAgent { id, name }
    }
}

impl Agent for TestAgent {
    fn step(
        &mut self,
        _state: &mut Option<Vec<u8>>,
        time: &f64,
        _mailbox: &mut Option<Mailbox>,
    ) -> Event {
        Event::new(*time, self.id, Action::Timeout(1.0))
    }

    fn get_state(&self) -> Option<&[u8]> {
        None
    }
}

pub struct SingleStepAgent {
    pub id: usize,
    pub name: String,
}

impl SingleStepAgent {
    pub fn new(id: usize, name: String) -> Self {
        SingleStepAgent { id, name }
    }
}

impl Agent for SingleStepAgent {
    fn step(
        &mut self,
        _state: &mut Option<Vec<u8>>,
        time: &f64,
        _mailbox: &mut Option<Mailbox>,
    ) -> Event {
        Event::new(*time, self.id, Action::Wait)
    }

    fn get_state(&self) -> Option<&[u8]> {
        None
    }
}

pub struct MessengerAgent {
    pub id: usize,
    pub name: String,
}

impl MessengerAgent {
    pub fn new(id: usize, name: String) -> Self {
        MessengerAgent { id, name }
    }
}

impl Agent for MessengerAgent {
    fn step(
        &mut self,
        _state: &mut Option<Vec<u8>>,
        time: &f64,
        mailbox: &mut Option<Mailbox>,
    ) -> Event {
        let _messages = mailbox.as_mut().unwrap().receive(self.id);

        let return_message = Message::new("Hello".into(), *time + 1.0, self.id, 1);

        match mailbox {
            Some(mb) => mb.send(return_message),
            None => (),
        }

        Event::new(*time, self.id, Action::Wait)
    }

    fn get_state(&self) -> Option<&[u8]> {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::worlds::*;
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn test_run() {
        let config = Config::new(1.0, Some(2000000.0), 100, 100, false, false, false);
        let mut world = World::<256, 1>::create(config);
        let agent_test = TestAgent::new(0, "Test".to_string());
        world.spawn(Box::new(agent_test));
        world.schedule(0.0, 0).unwrap();
        assert!(world.run().await.unwrap() == ());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_baseline_processing_bench() {
        let duration_secs = 20000000;
        let timestep = 1.0;
        let terminal = Some(duration_secs as f64);

        // minimal config world, no logs, no mail, no live for base processing speed benchmark
        let config = Config::new(timestep, terminal, 10, 10, false, false, false);
        let mut world = World::<128, 1>::create(config);

        let agent = TestAgent::new(0, format!("Test{}", 0));
        world.spawn(Box::new(agent));
        world.schedule(0.0, 0).unwrap();

        let start = Instant::now();
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
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_periphery() {
        let config = Config::new(1.0, Some(1000.0), 100, 100, false, true, false);
        let mut world = World::<256, 1>::create(config);
        let agent_test = SingleStepAgent::new(0, "Test".to_string());
        world.spawn(Box::new(agent_test));
        world.schedule(0.0, 0).unwrap();

        assert!(world.step_counter() == 0);
        assert!(world.now() == 0.0);
        assert!(world.state().is_none());

        world.run().await.unwrap();

        assert!(world
            .logger
            .get_snapshots()
            .pop()
            .unwrap()
            .shared_state
            .is_none());
        assert!(
            world
                .logger
                .get_snapshots()
                .pop()
                .unwrap()
                .agent_states
                .len()
                == 0
        );
        assert!(world.logger.get_snapshots().pop().unwrap().timestamp == 1.0);

        assert!(world.now() == 1000.0);
        assert!(world.step_counter() == 1000);
    }

    // need to fix and test the mailbox, and write some universe tests
}
