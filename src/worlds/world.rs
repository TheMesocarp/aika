use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use super::{Action, Agent, Clock, Config, Event, Mailbox, Message, SimError};
use crate::logger::Logger;

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
    overflow: BTreeSet<Reverse<Event>>,
    clock: Clock<SLOTS, HEIGHT>,
    _savedmail: BTreeSet<Message>,
    pub agents: Vec<Box<dyn Agent>>,
    mailbox: Mailbox,
    state: Option<Vec<u8>>,
    pub logger: Option<Logger>,
}

unsafe impl<const SLOTS: usize, const HEIGHT: usize> Send for World<SLOTS, HEIGHT> {}
unsafe impl<const SLOTS: usize, const HEIGHT: usize> Sync for World<SLOTS, HEIGHT> {}

impl<const SLOTS: usize, const HEIGHT: usize> World<SLOTS, HEIGHT> {
    /// Create a new world with the given configuration.
    /// By default, this will include a toggleable CLI for real-time simulation control, a logger for state logging, an asynchronous runtime, and a mailbox for message passing between agents.
    pub fn create(config: Config) -> Self {
        World {
            overflow: BTreeSet::new(),
            clock: Clock::<SLOTS, HEIGHT>::new(config.timestep, config.terminal).unwrap(),
            _savedmail: BTreeSet::new(),
            agents: Vec::new(),
            mailbox: Mailbox::new(config.mailbox_size),
            state: None,
            logger: if config.logs {
                Some(Logger::new())
            } else {
                None
            },
        }
    }

    /// Spawn a new agent into the world.
    pub fn spawn(&mut self, agent: Box<dyn Agent>) -> usize {
        self.agents.push(agent);
        self.agents.len() - 1
    }

    fn _log_mail(&mut self, msg: Message) {
        self._savedmail.insert(msg);
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
    pub fn now(&self) -> f64 {
        self.clock.time.time
    }

    /// Get the current step of the simulation.
    pub fn step_counter(&self) -> usize {
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
    pub fn schedule(&mut self, time: f64, agent: usize) -> Result<(), SimError> {
        if time < self.now() {
            return Err(SimError::TimeTravel);
        } else if time > self.clock.time.terminal.unwrap_or(f64::INFINITY) {
            return Err(SimError::PastTerminal);
        }

        self.commit(Event::new(time, agent, Action::Wait));
        Ok(())
    }

    /// Run the simulation.
    pub fn run(&mut self) -> Result<(), SimError> {
        loop {
            if self.now() + self.clock.time.timestep
                > self.clock.time.terminal.unwrap_or(f64::INFINITY)
            {
                break;
            }

            match self.clock.tick(&mut self.overflow) {
                Ok(events) => {
                    for event in events {
                        if event.time > self.clock.time.terminal.unwrap_or(f64::INFINITY) {
                            break;
                        }

                        let event = &mut self.agents[event.agent].step(
                            &mut self.state,
                            &event.time,
                            &mut self.mailbox,
                        );

                        self.handle_log(&event);

                        match event.yield_ {
                            Action::Timeout(time) => {
                                if self.now() + time
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
