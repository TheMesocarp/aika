use worlds::{Action, Agent, Event, Mailbox, Message};

extern crate tokio;

pub mod logger;
pub mod universes;
pub mod worlds;
pub mod pworlds;
pub mod clock; 

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
    fn step(&mut self, _state: &mut Option<Vec<u8>>, time: &u64, _mailbox: &mut Mailbox) -> Event {
        Event::new(*time, self.id, Action::Timeout(1))
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
    fn step(&mut self, _state: &mut Option<Vec<u8>>, time: &u64, _mailbox: &mut Mailbox) -> Event {
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
    fn step(&mut self, _state: &mut Option<Vec<u8>>, time: &u64, mailbox: &mut Mailbox) -> Event {
        let _messages = mailbox.receive(self.id);

        let return_message = Message::new("Hello".into(), *time, *time + 1, self.id, 1);

        mailbox.send(return_message);

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

    #[test]
    fn test_run() {
        let config = Config::new(1.0, Some(2000000.0), 100, 100, false);
        let mut world = World::<256, 1>::create(config);
        let agent_test = TestAgent::new(0, "Test".to_string());
        world.spawn(Box::new(agent_test));
        world.schedule(0, 0).unwrap();
        assert!(world.run().unwrap() == ());
    }

    #[test]
    fn test_clock() {
        let timestep = 1.0;
        let terminal = None;

        // minimal config world, no logs, no mail, no live for base processing speed benchmark
        let config = Config::new(timestep, terminal, 10, 10, false);
        let mut world = World::<128, 4>::create(config);

        let agent = TestAgent::new(0, format!("Test{}", 0));
        world.spawn(Box::new(agent));
        world.schedule(128, 0).unwrap();
        world.schedule(256, 0).unwrap();
        world.schedule(128 * 129, 0).unwrap();
        world.schedule(128 * 129 * 129, 0).unwrap();
        println!("scheduled");

        assert!(world.clock.wheels[1][0].len() == 1);
        assert!(world.clock.wheels[1][1].len() == 1);
        assert!(world.clock.wheels[1][2].len() == 0);
        assert!(world.clock.wheels[2][0].len() == 1);
        assert!(world.clock.wheels[3][0].len() == 1);
        assert!(world.clock.wheels[3][1].len() == 0);
    }

    #[test]
    fn test_periphery() {
        let config = Config::new(1.0, Some(1000.0), 100, 100, true);
        let mut world = World::<256, 1>::create(config);
        let agent_test = SingleStepAgent::new(0, "Test".to_string());
        world.spawn(Box::new(agent_test));
        world.schedule(0, 0).unwrap();

        assert!(world.step_counter() == 0);
        assert!(world.now() == 0);
        assert!(world.state().is_none());

        world.run().unwrap();

        assert!(world
            .logger
            .as_ref()
            .unwrap()
            .get_snapshots()
            .pop()
            .unwrap()
            .shared_state
            .is_none());
        assert!(
            world
                .logger
                .as_ref()
                .unwrap()
                .get_snapshots()
                .pop()
                .unwrap()
                .agent_states
                .len()
                == 0
        );
        assert!(
            world
                .logger
                .as_ref()
                .unwrap()
                .get_snapshots()
                .pop()
                .unwrap()
                .timestamp
                == 1.0
        );

        assert!(world.now() == 1000);
        assert!(world.step_counter() == 1000);
    }

    // need to fix and test the mailbox, and write some universe tests
}
