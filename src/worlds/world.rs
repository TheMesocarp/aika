use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;
use tokio::io::AsyncBufReadExt;
use tokio::sync::{mpsc::Sender, watch};

use super::{Action, Agent, Clock, Config, Event, Mailbox, Message, SimError};
use crate::logger::Logger;

/// Control commands for the real-time simulation
pub enum ControlCommand {
    Pause,
    Resume,
    SetTimeScale(f64),
    Quit,
    Schedule(f64, usize),
}

/// A world that can contain multiple agents and run a simulation.
pub struct World<'a, const SLOTS: usize, const HEIGHT: usize> {
    overflow: BTreeSet<Reverse<Event>>,
    clock: Clock<SLOTS, HEIGHT>,
    _savedmail: BTreeSet<Message<'a>>,
    pub agents: Vec<Box<dyn Agent>>,
    mailbox: Mailbox<'a>,
    state: Option<&'a [u8]>,
    runtype: (bool, bool, bool),
    pub pause: Option<(watch::Sender<bool>, watch::Receiver<bool>)>,
    pub logger: Logger,
}

unsafe impl<'a, const SLOTS: usize, const HEIGHT: usize> Send for World<'a, SLOTS, HEIGHT> {}
unsafe impl<'a, const SLOTS: usize, const HEIGHT: usize> Sync for World<'a, SLOTS, HEIGHT> {}

impl<'a, const SLOTS: usize, const HEIGHT: usize> World<'a, SLOTS, HEIGHT> {
    /// Create a new world with the given configuration.
    /// By default, this will include a toggleable CLI for real-time simulation control, a logger for state logging, an asynchronous runtime, and a mailbox for message passing between agents.
    pub fn create(config: Config) -> Self {
        let pause = if config.live {
            Some(watch::channel(false))
        } else {
            None
        };
        let mailbox = Mailbox::new(config.mailbox_size);
        World {
            overflow: BTreeSet::new(),
            clock: Clock::<SLOTS, HEIGHT>::new(config.timestep, config.terminal).unwrap(),
            _savedmail: BTreeSet::new(),
            agents: Vec::new(),
            mailbox,
            runtype: (config.live, config.logs, config.mail),
            state: None,
            pause,
            logger: Logger::new(),
        }
    }
    /// Spawn a new agent into the world.
    pub fn spawn(&mut self, agent: Box<dyn Agent>) -> usize {
        self.agents.push(agent);
        self.agents.len() - 1
    }

    fn spawn_cli(&self, cmd_tx: Sender<ControlCommand>) {
        tokio::spawn(async move {
            let mut reader = tokio::io::BufReader::new(tokio::io::stdin());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).await.is_ok() {
                    let cmd = match line.trim() {
                        "pause" => ControlCommand::Pause,
                        "resume" => ControlCommand::Resume,
                        "quit" => ControlCommand::Quit,
                        cmd if cmd.starts_with("speed ") => {
                            if let Some(scale) = cmd
                                .split_whitespace()
                                .nth(1)
                                .and_then(|s| s.parse::<f64>().ok())
                            {
                                ControlCommand::SetTimeScale(scale)
                            } else {
                                continue;
                            }
                        }
                        cmd if cmd.starts_with("schedule ") => {
                            let parts: Vec<_> = cmd.split_whitespace().collect();
                            if parts.len() >= 3 {
                                if let (Some(time), Some(idx)) =
                                    (parts[1].parse::<f64>().ok(), parts[2].parse::<usize>().ok())
                                {
                                    ControlCommand::Schedule(time, idx)
                                } else {
                                    continue;
                                }
                            } else {
                                continue;
                            }
                        }
                        _ => continue,
                    };
                    if cmd_tx.send(cmd).await.is_err() {
                        break;
                    }
                } else {
                    break;
                }
            }
        });
    }

    fn _log_mail(&mut self, msg: Message<'a>) {
        self._savedmail.insert(msg);
    }

    fn commit(&mut self, event: Event) {
        let event_maybe = self.clock.insert(event);
        if event_maybe.is_err() {
            self.overflow.insert(Reverse(event_maybe.err().unwrap()));
        }
    }
    /// Pause the real-time simulation.
    pub fn pause(&mut self) -> Result<(), SimError> {
        if self.pause.is_none() {
            return Err(SimError::NotRealtime);
        }
        let pause = self.pause.as_mut().unwrap().0.send(true);
        if pause.is_err() {
            return Err(SimError::PlaybackFroze);
        }
        Ok(())
    }
    /// Resume the real-time simulation.
    pub fn resume(&mut self) -> Result<(), SimError> {
        if self.pause.is_none() {
            return Err(SimError::NotRealtime);
        }
        let resume = self.pause.as_mut().unwrap().0.send(false);
        if resume.is_err() {
            return Err(SimError::PlaybackFroze);
        }
        Ok(())
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
    pub fn state(&self) -> Option<&'a [u8]> {
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
        if time < self.clock.time.time {
            return Err(SimError::TimeTravel);
        } else if time > self.clock.time.terminal.unwrap_or(f64::INFINITY) {
            return Err(SimError::PastTerminal);
        }
        self.commit(Event::new(time, agent, Action::Wait));
        Ok(())
    }

    /// Run the simulation.
    pub async fn run(&mut self) -> Result<(), SimError> {
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel(100);
        if self.runtype.0 {
            self.spawn_cli(cmd_tx);
        }
        loop {
            if self.clock.time.time + self.clock.time.timestep
                > self.clock.time.terminal.unwrap_or(f64::INFINITY)
            {
                break;
            }
            if self.runtype.0 {
                while let Ok(cmd) = cmd_rx.try_recv() {
                    match cmd {
                        ControlCommand::Pause => self.pause()?,
                        ControlCommand::Resume => self.resume()?,
                        ControlCommand::SetTimeScale(scale) => self.rescale_time(scale),
                        ControlCommand::Quit => break,
                        ControlCommand::Schedule(time, idx) => {
                            self.commit(Event::new(time, idx, Action::Wait));
                        }
                    }
                }
                if *self.pause.as_mut().unwrap().1.borrow() {
                    tokio::select! {
                        _ = self.pause.as_mut().unwrap().1.changed() => {},
                        cmd = cmd_rx.recv() => {
                            if let Some(cmd) = cmd {
                                match cmd {
                                    ControlCommand::Pause => self.pause()?,
                                    ControlCommand::Resume => self.resume()?,
                                    ControlCommand::SetTimeScale(scale) => self.rescale_time(scale),
                                    ControlCommand::Quit => break,
                                    ControlCommand::Schedule(time, idx) => {
                                        self.commit(Event::new(time, idx, Action::Wait));
                                    }
                                }
                            }
                        }
                    }
                    continue;
                }
            }
            match self.clock.tick(&mut self.overflow) {
                Ok(events) => {
                    // if self.clock.time.step % 10000 == 0 {
                    //     println!("Processing events {:?}", self.clock.time.time);
                    // }
                    for event in events {
                        if self.runtype.0 {
                            tokio::time::sleep(Duration::from_millis(
                                (self.clock.time.timestep * 1000.0 / self.clock.time.timescale)
                                    as u64,
                            ))
                            .await;
                        }
                        if event.time > self.clock.time.terminal.unwrap_or(f64::INFINITY) {
                            break;
                        }
                        if self.runtype.2 {
                            self.mailbox.collect_messages().await;
                        }
                        let agent = &mut self.agents[event.agent];
                        let event = agent
                            .step(&mut self.state, &event.time, &mut self.mailbox)
                            .await;
                        if self.runtype.1 {
                            let agent_states: BTreeMap<usize, Vec<u8>> = self
                                .agents
                                .iter()
                                .enumerate()
                                .filter_map(|(i, agt)| {
                                    if agt.get_state().is_some() {
                                        Some((i, agt.get_state().unwrap().to_vec()))
                                    } else {
                                        None
                                    }
                                })
                                .collect();
                            self.logger.log(
                                self.now(),
                                if self.state.is_some() {
                                    Some(self.state.unwrap().to_vec())
                                } else {
                                    None
                                },
                                agent_states,
                                event.clone(),
                            );
                        }
                        match event.yield_ {
                            Action::Timeout(time) => {
                                if self.clock.time.time + time
                                    > self.clock.time.terminal.unwrap_or(f64::INFINITY)
                                {
                                    continue;
                                }
                                let new = Event::new(
                                    self.clock.time.time + time,
                                    event.agent,
                                    Action::Wait,
                                );
                                self.commit(new);
                            }
                            Action::Schedule(time) => {
                                let new = Event::new(time, event.agent, Action::Wait);
                                self.commit(new);
                            }
                            Action::Trigger { time, idx } => {
                                let new = Event::new(time, idx, Action::Wait);
                                self.commit(new);
                            }
                            Action::Wait => {}
                            Action::Break => {
                                break;
                            }
                        }
                        if self.runtype.0 {
                            while let Ok(cmd) = cmd_rx.try_recv() {
                                match cmd {
                                    ControlCommand::Pause => self.pause()?,
                                    ControlCommand::Resume => self.resume()?,
                                    ControlCommand::SetTimeScale(scale) => self.rescale_time(scale),
                                    ControlCommand::Quit => break,
                                    ControlCommand::Schedule(time, idx) => {
                                        self.commit(Event::new(time, idx, Action::Wait));
                                    }
                                }
                            }
                        }
                    }
                }
                Err(SimError::NoEvents) => {
                    continue;
                }
                Err(_) => {}
            }
        }
        Ok(())
    }
}
