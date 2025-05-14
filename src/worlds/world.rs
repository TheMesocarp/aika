use std::cmp::Reverse;
use std::collections::BTreeSet;
use std::ffi::c_void;

use bytemuck::Pod;

use super::agent::Supports;
use super::{Action, Agent, Config, Event, Mailbox, SimError};
use crate::clock::Clock;
use crate::logger::Katko;

/// A world that can contain multiple agents and run a simulation.
pub struct World<const LOGS: usize, const SLOTS: usize, const HEIGHT: usize> {
    pub overflow: BTreeSet<Reverse<Event>>,
    pub clock: Clock<Event, SLOTS, HEIGHT>,
    pub agents: Vec<Box<dyn Agent>>,
    mailbox: Mailbox,
    state: Option<*mut c_void>,
    pub logger: Option<Katko>,
}

unsafe impl<const LOGS: usize, const SLOTS: usize, const HEIGHT: usize> Send
    for World<LOGS, SLOTS, HEIGHT>
{
}
unsafe impl<const LOGS: usize, const SLOTS: usize, const HEIGHT: usize> Sync
    for World<LOGS, SLOTS, HEIGHT>
{
}

impl<const LOGS: usize, const SLOTS: usize, const HEIGHT: usize> World<LOGS, SLOTS, HEIGHT> {
    /// Create a new world with the given configuration.
    /// By default, this will include a logger for state logging and a mailbox for message passing between agents.
    pub fn create<T: Pod + 'static>(config: Config, init_state: Option<*mut c_void>) -> Self {
        World {
            overflow: BTreeSet::new(),
            clock: Clock::<Event, SLOTS, HEIGHT>::new(config.timestep, config.terminal).unwrap(),
            agents: Vec::new(),
            mailbox: Mailbox::new(config.mailbox_size),
            state: init_state,
            logger: config
                .logs
                .then_some(Katko::init::<T>(config.shared_state, LOGS)),
        }
    }

    /// Spawn a new agent into the world.
    pub fn spawn<T: Pod + 'static>(&mut self, agent: Box<dyn Agent>) -> usize {
        self.agents.push(agent);
        if self.logger.is_some() {
            self.logger.as_mut().unwrap().add_agent::<T>(LOGS);
        }
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

    /// Clone the current state pointer of the simulation.
    pub fn state(&self) -> Option<*mut c_void> {
        self.state
    }

    /// Schedule an event for an agent at a given time.
    pub fn schedule(&mut self, time: u64, agent: usize) -> Result<(), SimError> {
        if time < self.now() {
            return Err(SimError::TimeTravel);
        } else if time as f64 * self.clock.time.timestep
            > self.clock.time.terminal.unwrap_or(f64::INFINITY)
        {
            return Err(SimError::PastTerminal);
        }
        let now = self.now();
        self.commit(Event::new(now, time, agent, Action::Wait));
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

            if let Ok(events) = self.clock.tick() {
                for event in events {
                    if event.time as f64 * self.clock.time.timestep
                        > self.clock.time.terminal.unwrap_or(f64::INFINITY)
                    {
                        break;
                    }
                    let supports = if self.logger.is_none() {
                        Supports::Mailbox(&mut self.mailbox)
                    } else {
                        Supports::Both(
                            &mut self.mailbox,
                            &mut self.logger.as_mut().unwrap().agents[event.agent],
                        )
                    };
                    let event = self.agents[event.agent].step(&event.time, supports);

                    match event.yield_ {
                        Action::Timeout(time) => {
                            if (self.now() + time) as f64 * self.clock.time.timestep
                                > self.clock.time.terminal.unwrap_or(f64::INFINITY)
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
                    if self.logger.is_some() {
                        self.logger.as_mut().unwrap().write_event(event);
                    }
                }
            }
            self.clock.increment(&mut self.overflow);
        }
        Ok(())
    }
}
