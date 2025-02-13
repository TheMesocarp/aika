use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use super::{Action, Agent, Config, Event, Mailbox, Message, SimError};
use crate::logger::Logger;
use crate::clock::Clock;

/// Control commands for the real-time simulation
///
/// TODO: breakout to universe level & let worlds read them on tick
// pub enum ControlCommand {
//     Pause,
//     Resume,
//     SetTimeScale(f64),
//     Quit,
//     Schedule(f64, usize),
// }

/// A world that can contain multiple agents and run a simulation.
pub struct World<const SLOTS: usize, const HEIGHT: usize> {
    pub overflow: BTreeSet<Reverse<Event>>,
    pub clock: Clock<Event, SLOTS, HEIGHT>,
    pub agents: Vec<Box<dyn Agent>>,
    mailbox: Mailbox,
    state: Option<Vec<u8>>,
    pub logger: Option<Logger>,
}

unsafe impl<const SLOTS: usize, const HEIGHT: usize> Send for World<SLOTS, HEIGHT> {}
unsafe impl<const SLOTS: usize, const HEIGHT: usize> Sync for World<SLOTS, HEIGHT> {}

impl<const SLOTS: usize, const HEIGHT: usize> World<SLOTS, HEIGHT> {
    /// Create a new world with the given configuration.
    /// By default, this will include a logger for state logging and a mailbox for message passing between agents.
    pub fn create(config: Config) -> Self {
        World {
            overflow: BTreeSet::new(),
            clock: Clock::<Event, SLOTS, HEIGHT>::new(config.timestep, config.terminal).unwrap(),
            agents: Vec::new(),
            mailbox: Mailbox::new(config.mailbox_size),
            state: None,
            logger: config.logs.then_some(Logger::new()),
        }
    }

    /// Spawn a new agent into the world.
    pub fn spawn(&mut self, agent: Box<dyn Agent>) -> usize {
        self.agents.push(agent);
        self.agents.len() - 1
    }

    fn commit(&mut self, event: Event) {
        let event_maybe = self.clock.insert(event);
        if event_maybe.is_err() {
            self.overflow.insert(Reverse(event_maybe.err().unwrap()));
        }
    }

    /// speed up/slow down the simulation playback.
    pub fn rescale_time(&mut self, timescale: f64) {
        self.clock.time.timescale = timescale;
    }

    /// Get the current time of the simulation.
    #[inline(always)]
    pub fn now(&self) -> u64 {
        self.clock.time.step
    }

    /// Get the current step of the simulation.
    pub fn step_counter(&self) -> u64 {
        self.clock.time.step
    }

    /// Clone the current state of the simulation.
    pub fn state(&self) -> Option<Vec<u8>> {
        self.state.clone()
    }

    /// Block the long term actions of agents !!this is broken since the shift to the timing wheel!!
    // pub fn block_agent(&mut self, idx: usize, until: Option<f64>) -> Result<(), SimError> {
    //     if self.agents.len() <= idx {
    //         return Err(SimError::InvalidIndex);
    //     }
    //     if until.is_none() {
    //         self.overflow.retain(|x| x.0.agent != idx);
    //     }
    //     self.overflow
    //         .retain(|x| x.0.agent != idx && x.0.time < until.unwrap());
    //     Ok(())
    // }
    // /// remove a particular pending event !!this is broken since the shift to the timing wheel!!
    // pub fn remove_event(&mut self, idx: usize, time: f64) -> Result<(), SimError> {
    //     if self.agents.len() <= idx {
    //         return Err(SimError::InvalidIndex);
    //     } else if self.clock.time.time > time {
    //         return Err(SimError::TimeTravel);
    //     }
    //     self.overflow
    //         .retain(|x| x.0.agent != idx && x.0.time != time);
    //     Ok(())
    // }

    /// Schedule an event for an agent at a given time.
    pub fn schedule(&mut self, time: u64, agent: usize) -> Result<(), SimError> {
        if time < self.now() {
            return Err(SimError::TimeTravel);
        } else if time as f64 * self.clock.time.timestep > self.clock.time.terminal.unwrap_or(f64::INFINITY) {
            return Err(SimError::PastTerminal);
        }

        self.commit(Event::new(time, agent, Action::Wait));
        Ok(())
    }

    /// Run the simulation.
    pub fn run(&mut self) -> Result<(), SimError> {
        loop {
            if (self.now() + 1) as f64 * self.clock.time.timestep
                > self.clock.time.terminal.unwrap_or(f64::INFINITY)
            {
                break;
            }

            match self.clock.tick(&mut self.overflow) {
                Ok(events) => {
                    for event in events {
                        if event.time as f64 * self.clock.time.timestep > self.clock.time.terminal.unwrap_or(f64::INFINITY) {
                            break;
                        }

                        let event = &mut self.agents[event.agent].step(
                            &mut self.state,
                            &event.time,
                            &mut self.mailbox,
                        );

                        self.handle_log(event);

                        match event.yield_ {
                            Action::Timeout(time) => {
                                if (self.now() + time) as f64 * self.clock.time.timestep
                                    > self.clock.time.terminal.unwrap_or(f64::INFINITY)
                                {
                                    continue;
                                }

                                self.commit(Event::new(
                                    self.now() + time,
                                    event.agent,
                                    Action::Wait,
                                ));
                            }
                            Action::Schedule(time) => {
                                self.commit(Event::new(time, event.agent, Action::Wait));
                            }
                            Action::Trigger { time, idx } => {
                                self.commit(Event::new(time, idx, Action::Wait));
                            }
                            Action::Wait => {}
                            Action::Break => {
                                break;
                            }
                        }
                    }
                }
                Err(_) => {}
            }
        }
        Ok(())
    }

    /// Handles logging of events, provided the logger is active.
    #[inline(always)]
    fn handle_log(&mut self, event: &Event) {
        if let Some(logger) = &mut self.logger {
            let agent_states: BTreeMap<usize, Vec<u8>> = self
                .agents
                .iter()
                .filter_map(|agent| agent.get_state())
                .map(|state| state.to_vec())
                .enumerate()
                .collect();

            logger.log(
                self.clock.time.time,
                match &self.state {
                    Some(state) => Some(state.clone()),
                    None => None,
                },
                agent_states,
                event.clone(),
            );
        }
    }
}
