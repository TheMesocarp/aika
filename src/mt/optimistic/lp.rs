// implement local loop with rollback and throttle window with `Agent`

use std::{
    cmp::Reverse,
    collections::BTreeSet,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
};

use mesocarp::{
    logging::journal::Journal,
    scheduling::{htw::Clock, Scheduleable},
};

use crate::{
    agents::{AgentSupport, ThreadedAgent},
    messages::{AntiMsg, Msg, Transfer},
    mt::optimistic::{config::LPConfig, gvt::RegistryOutput},
    st::{
        event::{Action, Event},
        TimeInfo,
    },
    SimError,
};

pub struct LocalMailSystem<
    const SLOTS: usize,
    const CLOCK_SLOTS: usize,
    const CLOCK_HEIGHT: usize,
    MessageType: Clone,
> {
    overflow: BTreeSet<Reverse<Msg<MessageType>>>,
    schedule: Clock<Msg<MessageType>, CLOCK_SLOTS, CLOCK_HEIGHT>,
    anti_messages: Journal,
}

impl<
        const SLOTS: usize,
        const CLOCK_SLOTS: usize,
        const CLOCK_HEIGHT: usize,
        MessageType: Clone,
    > LocalMailSystem<SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
    pub fn new(arena_size: usize) -> Result<Self, SimError> {
        let overflow = BTreeSet::new();
        let schedule = Clock::new().map_err(SimError::MesoError)?;
        let anti_messages = Journal::init(arena_size);
        Ok(Self {
            overflow,
            schedule,
            anti_messages,
        })
    }
}

pub struct LocalEventSystem<const CLOCK_SLOTS: usize, const CLOCK_HEIGHT: usize> {
    overflow: BTreeSet<Reverse<Event>>,
    local_clock: Clock<Event, CLOCK_SLOTS, CLOCK_HEIGHT>,
}

impl<const CLOCK_SLOTS: usize, const CLOCK_HEIGHT: usize>
    LocalEventSystem<CLOCK_SLOTS, CLOCK_HEIGHT>
{
    pub fn new() -> Result<Self, SimError> {
        let overflow = BTreeSet::new();
        let local_clock = Clock::new().map_err(SimError::MesoError)?;
        Ok(Self {
            overflow,
            local_clock,
        })
    }
}

pub struct LocalTime {
    time: u64,
    horizon: Option<u64>,
    time_info: TimeInfo,
    global_clock: Arc<AtomicU64>,
}

impl LocalTime {
    pub fn init(
        global_clock: Arc<AtomicU64>,
        horizon: Option<u64>,
        timestep: f64,
        terminal: f64,
    ) -> Self {
        Self {
            time: 0,
            horizon,
            time_info: TimeInfo { timestep, terminal },
            global_clock,
        }
    }
}

/// an `LP` (Logical Process) is an agent that owns its own thread
pub struct LP<
    const SLOTS: usize,
    const CLOCK_SLOTS: usize,
    const CLOCK_HEIGHT: usize,
    MessageType: Clone,
> {
    agent: Box<dyn ThreadedAgent<SLOTS, Transfer<MessageType>>>,
    agent_id: usize,
    supports: AgentSupport<SLOTS, Transfer<MessageType>>,
    event_process: LocalEventSystem<CLOCK_SLOTS, CLOCK_HEIGHT>,
    mail_process: LocalMailSystem<SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>,
    time: LocalTime,
    paused: bool,
}

impl<
        const SLOTS: usize,
        const CLOCK_SLOTS: usize,
        const CLOCK_HEIGHT: usize,
        MessageType: Clone,
    > LP<SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
    pub fn init(
        agent: Box<dyn ThreadedAgent<SLOTS, Transfer<MessageType>>>,
        registry: RegistryOutput<SLOTS, MessageType>,
        config: LPConfig,
    ) -> Result<Self, SimError> {
        let time = LocalTime::init(registry.0, config.horizon, config.timestep, config.terminal);
        let event_process = LocalEventSystem::new()?;
        let mail_process = LocalMailSystem::new(config.anti_msg_arena_size)?;
        let agent_id = registry.2;
        let supports = AgentSupport::new(Some(registry.1), Some(config.state_arena_size));
        Ok(Self {
            agent,
            agent_id,
            supports,
            event_process,
            mail_process,
            time,
            paused: false,
        })
    }

    fn commit(&mut self, event: Event) {
        let event_maybe = self.event_process.local_clock.insert(event);
        if event_maybe.is_err() {
            self.event_process
                .overflow
                .insert(Reverse(event_maybe.err().unwrap()));
        }
    }

    fn commit_mail(&mut self, msg: Msg<MessageType>) {
        let msg = self.mail_process.schedule.insert(msg);
        if msg.is_err() {
            self.mail_process
                .overflow
                .insert(Reverse(msg.err().unwrap()));
        }
    }

    fn annihilate(&mut self, anti_msg: AntiMsg) {
        let time = anti_msg.time();
        let idxs = self.mail_process.schedule.current_idxs;
        let diff = (time - self.mail_process.schedule.time) as usize;
        for (k, idx) in idxs.iter().enumerate().take(CLOCK_HEIGHT) {
            let startidx = ((CLOCK_SLOTS).pow(1 + k as u32) - CLOCK_SLOTS) / (CLOCK_SLOTS - 1); // start index for each level
            let endidx = ((CLOCK_SLOTS).pow(2 + k as u32) - CLOCK_SLOTS) / (CLOCK_SLOTS - 1) - 1; // end index for each level
            if diff >= startidx {
                if diff
                    >= (((CLOCK_SLOTS).pow(1 + CLOCK_HEIGHT as u32) - CLOCK_SLOTS)
                        / (CLOCK_SLOTS - 1))
                {
                    break;
                }
                if diff > endidx {
                    continue;
                }
                let offset = ((diff - startidx) / (SLOTS.pow(k as u32)) + idx) % SLOTS;
                let msgs = &mut self.mail_process.schedule.wheels[k][offset];
                let mut remaining = Vec::new();
                while let Some(msg) = msgs.pop() {
                    if anti_msg.annihilate(&msg) {
                        continue;
                    }
                    remaining.push(msg);
                }
                *msgs = remaining;
                return;
            }
        }
        // fallback if timestamp beyond clock horizon
        let mut to_be_removed = BTreeSet::new();
        for i in self.mail_process.overflow.iter().enumerate() {
            if anti_msg.annihilate(&i.1 .0) {
                to_be_removed.insert(Reverse(i.0));
            }
        }
        let current = self.mail_process.overflow.clone();
        let mut vec = current.into_iter().collect::<Vec<_>>();
        for i in to_be_removed {
            let idx = i.0;
            vec.remove(idx);
        }
        self.mail_process.overflow = BTreeSet::from_iter(vec);
    }

    pub fn step(&mut self) -> Result<(), SimError> {
        // check messages
        if let Some(transfers) = self.supports.mailbox.as_mut().unwrap().poll() {
            for transfer in transfers {
                let time = transfer.time();
                if time < self.time.time {
                    self.rollback(time)?;
                }
                match transfer {
                    Transfer::Msg(msg) => self.commit_mail(msg),
                    Transfer::AntiMsg(anti_msg) => self.annihilate(anti_msg),
                }
            }
        };
        // step forward
        if let Ok(msgs) = self.mail_process.schedule.tick() {
            for msg in msgs {
                if msg.recv as f64 * self.time.time_info.timestep > self.time.time_info.terminal {
                    break;
                }
                let supports = &mut self.supports;
                supports.current_time = msg.recv;
                self.agent.read_message(supports, self.agent_id);
            }
        }
        self.mail_process
            .schedule
            .increment(&mut self.mail_process.overflow);
        if let Ok(events) = self.event_process.local_clock.tick() {
            for event in events {
                if event.time as f64 * self.time.time_info.timestep > self.time.time_info.terminal {
                    break;
                }
                let supports = &mut self.supports;
                supports.current_time = event.time;
                let event = self.agent.step(supports);
                match event.yield_ {
                    Action::Timeout(time) => {
                        if (self.time.time + time) as f64 * self.time.time_info.timestep
                            > self.time.time_info.terminal
                        {
                            continue;
                        }

                        self.commit(Event::new(
                            self.time.time,
                            self.time.time + time,
                            event.agent,
                            Action::Wait,
                        ));
                    }
                    Action::Schedule(time) => {
                        self.commit(Event::new(self.time.time, time, event.agent, Action::Wait));
                    }
                    Action::Trigger { time, idx } => {
                        self.commit(Event::new(self.time.time, time, idx, Action::Wait));
                    }
                    Action::Wait => {}
                    Action::Break => {
                        break;
                    }
                }
            }
        }
        self.event_process
            .local_clock
            .increment(&mut self.event_process.overflow);
        self.time.time += 1;
        Ok(())
    }

    pub fn rollback(&mut self, time: u64) -> Result<(), SimError> {
        if time > self.time.time {
            return Err(SimError::TimeTravel);
        }
        self.supports.logger.as_mut().unwrap().rollback(time);
        self.mail_process
            .schedule
            .rollback(&mut self.mail_process.overflow, time);
        let out = self
            .mail_process
            .anti_messages
            .rollback_return::<AntiMsg>(time);
        for (anti, _) in out {
            self.supports
                .mailbox
                .as_mut()
                .unwrap()
                .send(Transfer::AntiMsg(anti))
                .map_err(SimError::MesoError)?;
        }

        self.event_process.local_clock = Clock::new().map_err(SimError::MesoError)?;
        self.event_process.local_clock.set_time(time);
        self.time.time = time;
        Ok(())
    }

    pub fn run(&mut self, termination_flag: Arc<AtomicBool>) -> Result<(), SimError> {
        while self.time.time as f64 * self.time.time_info.timestep < self.time.time_info.terminal {
            if termination_flag.load(Ordering::Acquire) {
                break;
            }
            let gvt = self.time.global_clock.load(Ordering::SeqCst);
            let throttled = self.time.horizon.is_some();
            if throttled {
                if self.time.time > gvt + self.time.horizon.unwrap() && !self.paused {
                    self.paused = true;
                    continue;
                }
                if self.paused {
                    if self.time.time == gvt + 1 {
                        self.paused = false;
                    }
                    continue;
                }
            }
            self.step()?;
        }
        Ok(())
    }
}

unsafe impl<
        const SLOTS: usize,
        const CLOCK_SLOTS: usize,
        const CLOCK_HEIGHT: usize,
        MessageType: Clone,
    > Send for LP<SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
}

unsafe impl<
        const SLOTS: usize,
        const CLOCK_SLOTS: usize,
        const CLOCK_HEIGHT: usize,
        MessageType: Clone,
    > Sync for LP<SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
}
