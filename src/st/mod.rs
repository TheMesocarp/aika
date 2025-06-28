use std::{cmp::Reverse, collections::BTreeSet};

use mesocarp::{comms::mailbox::ThreadWorld, scheduling::htw::Clock};

use crate::{
    agents::{Agent, AgentSupport},
    messages::Msg,
    st::event::{Action, Event},
    SimError,
};

pub mod event;

pub struct TimeInfo {
    pub timestep: f64,
    pub terminal: f64,
}

/// A world that can contain multiple agents and run a simulation.
pub struct World<
    const MESSAGE_SLOTS: usize,
    const CLOCK_SLOTS: usize,
    const CLOCK_HEIGHT: usize,
    MessageType: Clone,
> {
    pub overflow: BTreeSet<Reverse<Event>>,
    pub clock: Clock<Event, CLOCK_SLOTS, CLOCK_HEIGHT>,
    pub agents: Vec<Box<dyn Agent<MESSAGE_SLOTS, Msg<MessageType>>>>,
    pub agent_supports: Vec<AgentSupport<MESSAGE_SLOTS, Msg<MessageType>>>,
    mailbox: Option<ThreadWorld<MESSAGE_SLOTS, Msg<MessageType>>>,
    pub time_info: TimeInfo,
}

unsafe impl<
        const MESSAGE_SLOTS: usize,
        const CLOCK_SLOTS: usize,
        const CLOCK_HEIGHT: usize,
        MessageType: Clone,
    > Send for World<MESSAGE_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
}
unsafe impl<
        const MESSAGE_SLOTS: usize,
        const CLOCK_SLOTS: usize,
        const CLOCK_HEIGHT: usize,
        MessageType: Clone,
    > Sync for World<MESSAGE_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
}

impl<
        const MESSAGE_SLOTS: usize,
        const CLOCK_SLOTS: usize,
        const CLOCK_HEIGHT: usize,
        MessageType: Clone,
    > World<MESSAGE_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
    pub fn init(terminal: f64, timestep: f64) -> Result<Self, SimError> {
        Ok(Self {
            overflow: BTreeSet::new(),
            clock: Clock::<Event, CLOCK_SLOTS, CLOCK_HEIGHT>::new().map_err(SimError::MesoError)?,
            agents: Vec::new(),
            agent_supports: Vec::new(),
            mailbox: None,
            time_info: TimeInfo { timestep, terminal },
        })
    }

    pub fn spawn_agent(&mut self, agent: Box<dyn Agent<MESSAGE_SLOTS, Msg<MessageType>>>) -> usize {
        self.agents.push(agent);
        self.agents.len() - 1
    }

    pub fn init_support_layers(&mut self, arena_size: Option<usize>) -> Result<(), SimError> {
        let agent_ids = self
            .agents
            .iter()
            .enumerate()
            .map(|x| x.0)
            .collect::<Vec<_>>();
        let thread_world = ThreadWorld::<MESSAGE_SLOTS, Msg<MessageType>>::new(agent_ids.clone())
            .map_err(SimError::MesoError)?;
        let len = self.agents.len();
        let mut supports: Vec<AgentSupport<MESSAGE_SLOTS, _>> = Vec::with_capacity(len);
        for i in agent_ids {
            let sup = AgentSupport::new(
                Some(thread_world.get_user(i).map_err(SimError::MesoError)?),
                arena_size,
            );
            supports.push(sup);
        }
        self.mailbox = Some(thread_world);
        self.agent_supports = supports;
        Ok(())
    }

    fn commit(&mut self, event: Event) {
        let event_maybe = self.clock.insert(event);
        if event_maybe.is_err() {
            self.overflow.insert(Reverse(event_maybe.err().unwrap()));
        }
    }

    /// Get the current time of the simulation.
    #[inline(always)]
    pub fn now(&self) -> u64 {
        self.clock.time
    }

    /// Schedule an event for an agent at a given time.
    pub fn schedule(&mut self, time: u64, agent: usize) -> Result<(), SimError> {
        if time < self.now() {
            return Err(SimError::TimeTravel);
        } else if time as f64 * self.time_info.timestep > self.time_info.terminal {
            return Err(SimError::PastTerminal);
        }
        let now = self.now();
        self.commit(Event::new(now, time, agent, Action::Wait));
        Ok(())
    }

    /// Run the simulation.
    pub fn run(&mut self) -> Result<(), SimError> {
        println!("running simulator");
        loop {
            if (self.now() + 1) as f64 * self.time_info.timestep > self.time_info.terminal {
                break;
            }

            if let Ok(events) = self.clock.tick() {
                for event in events {
                    if event.time as f64 * self.time_info.timestep > self.time_info.terminal {
                        break;
                    }
                    let supports = &mut self.agent_supports[event.agent];
                    supports.current_time = event.time;
                    let event = self.agents[event.agent].step(supports);
                    match event.yield_ {
                        Action::Timeout(time) => {
                            if (self.now() + time) as f64 * self.time_info.timestep
                                > self.time_info.terminal
                            {
                                continue;
                            }

                            self.commit(Event::new(
                                self.now(),
                                self.now() + time,
                                event.agent,
                                Action::Wait,
                            ));
                        }
                        Action::Schedule(time) => {
                            self.commit(Event::new(self.now(), time, event.agent, Action::Wait));
                        }
                        Action::Trigger { time, idx } => {
                            self.commit(Event::new(self.now(), time, idx, Action::Wait));
                        }
                        Action::Wait => {}
                        Action::Break => {
                            break;
                        }
                    }
                }
            }
            self.clock.increment(&mut self.overflow);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Markovian Agent
    pub struct TestAgent {
        pub id: usize,
    }

    impl TestAgent {
        pub fn new(id: usize) -> Self {
            TestAgent { id }
        }
    }

    impl Agent<8, Msg<u8>> for TestAgent {
        fn step(&mut self, supports: &mut AgentSupport<8, Msg<u8>>) -> Event {
            let time = supports.current_time;
            Event::new(time, time, self.id, Action::Timeout(1))
        }
    }

    #[test]
    fn test_run() {
        let mut world = World::<8, 128, 1, u8>::init(40000000.0, 1.0).unwrap();
        let agent_test = TestAgent::new(0);
        world.spawn_agent(Box::new(agent_test));
        world.init_support_layers(None).unwrap();
        world.schedule(1, 0).unwrap();
        assert!(world.agent_supports.len() == 1);
        world.run().unwrap();
    }
}
