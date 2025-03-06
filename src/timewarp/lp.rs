use std::cmp::Reverse;
use std::collections::BTreeSet;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::clock::Clock;
use crate::clock::Scheduleable;
use crate::logger::Lumi;
use crate::worlds::Action;
use crate::worlds::Event;
use crate::worlds::Message;
use crate::worlds::SimError;

use super::antimessage::AntiMessage;
use super::comms::CircularBuffer;
use super::comms::Transferable;
use super::paragent::HandlerOutput;
use super::paragent::LogicalProcess;

// Wrapper for objects in a time warp simulator
pub enum Object {
    Event(Event),
    Message(Message),
}

impl Scheduleable for Object {
    fn time(&self) -> u64 {
        match self {
            Object::Event(e) => e.time,
            Object::Message(m) => m.received,
        }
    }
    fn commit_time(&self) -> u64 {
        match self {
            Object::Event(e) => e.commit_time,
            Object::Message(m) => m.sent,
        }
    }
}

impl PartialEq for Object {
    fn eq(&self, other: &Self) -> bool {
        self.time() == other.time()
    }
}

impl Eq for Object {}

impl PartialOrd for Object {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Object {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.time().partial_cmp(&other.time()).unwrap()
    }
}
/// `LP` provides all the logic for executing local events, processing messages to and from other LPs, and rollbacks when incoming messages are intended to exxecute in the past.
pub struct LP<const SLOTS: usize, const HEIGHT: usize, const SIZE: usize> {
    pub scheduler: Clock<Object, SLOTS, HEIGHT>,
    pub overflow: BTreeSet<Reverse<Object>>,
    state: Lumi,
    out_antimessages: Vec<AntiMessage>,
    in_antimessages: Vec<AntiMessage>,
    in_times: Vec<u64>,
    in_queue: [Transferable; SIZE],
    out_queue: BTreeSet<Reverse<Transferable>>,
    buffers: [CircularBuffer<SIZE>; 2],
    agent: Box<dyn LogicalProcess>,
    pub step: Arc<AtomicUsize>,
    pub rollbacks: usize,
    pub id: usize,
}

impl<const SLOTS: usize, const HEIGHT: usize, const SIZE: usize> LP<SLOTS, HEIGHT, SIZE> {
    /// Spawn new logical process
    pub fn new<T: 'static>(
        id: usize,
        agent: Box<dyn LogicalProcess>,
        timestep: f64,
        step: Arc<AtomicUsize>,
        buffers: [CircularBuffer<SIZE>; 2],
        log_slots: usize,
    ) -> Self {
        LP {
            scheduler: Clock::<Object, SLOTS, HEIGHT>::new(timestep, None).unwrap(),
            overflow: BTreeSet::new(),
            state: Lumi::initialize::<T>(log_slots),
            out_antimessages: Vec::new(),
            in_antimessages: Vec::new(),
            in_times: Vec::new(),
            in_queue: [const { Transferable::Nan }; SIZE],
            out_queue: BTreeSet::new(),
            buffers,
            agent,
            step,
            rollbacks: 0,
            id,
        }
    }
    /// Set terminal time
    pub fn set_terminal(&mut self, terminal: f64) {
        self.scheduler.time.terminal = Some(terminal);
    }
    /// Read incoming messages from Comms
    fn read_incoming(&mut self) {
        let circular = &self.buffers[0];
        let mut r = circular.read_idx.load(Ordering::Acquire);
        let w = circular.write_idx.load(Ordering::Acquire);
        let mut count = 0;
        loop {
            if r == w {
                return;
            }
            if count == SIZE {
                return;
            }
            let msg = unsafe { (*circular.ptr)[r].take().unwrap() };
            self.in_queue[count] = msg;
            r = (r + 1) % SIZE;
            count += 1;
        }
    }
    /// Write outgoing messages to Comms
    fn write_outgoing(&mut self, msg: Transferable) -> Result<(), Transferable> {
        let circular = &self.buffers[1];
        let w = circular.write_idx.load(Ordering::Acquire);
        let r = circular.read_idx.load(Ordering::Acquire);
        let next = (w + 1) % SIZE;
        if next == r {
            return Err(msg);
        }
        unsafe {
            (*circular.ptr)[w] = Some(msg);
        }
        circular.write_idx.store(next, Ordering::Release);
        Ok(())
    }
    /// rollback state and clock, and send required anti messages
    fn rollback(&mut self, time: u64) -> Result<(), SimError> {
        self.scheduler.rollback(time, &mut self.overflow)?;
        self.state.rollback(time)?;
        for i in 0..self.out_antimessages.len() {
            if self.out_antimessages[i].sent > time {
                let anti = self.out_antimessages.remove(i);
                let msg = self.write_outgoing(Transferable::AntiMessage(anti));
                if msg.is_err() {
                    self.out_queue.insert(Reverse(msg.err().unwrap()));
                };
            }
        }
        Ok(())
    }
    /// commit object to scheduler
    pub fn commit(&mut self, event: Object) {
        let result = self.scheduler.insert(event);
        if result.is_err() {
            self.overflow.insert(Reverse(result.err().unwrap()));
        }
    }
    /// one local time step in an LP
    fn step(&mut self) -> Result<(), SimError> {
        self.read_incoming();
        // process messages with insertation and time checks.
        let mut rollback = u64::MAX;
        for i in self.in_queue.as_mut() {
            if *i != Transferable::Nan {
                let msg = std::mem::replace(i, Transferable::Nan);
                match msg {
                    Transferable::AntiMessage(anti) => {
                        if anti.received < self.scheduler.time.step {
                            if anti.received < rollback {
                                rollback = anti.received;
                            }
                        }
                        self.in_antimessages.push(anti);
                    }
                    Transferable::Message(msg) => {
                        if msg.received < self.scheduler.time.step {
                            if msg.received < rollback {
                                rollback = msg.received;
                            }
                            continue;
                        }
                        let result = self.scheduler.insert(Object::Message(msg));
                        if result.is_err() {
                            self.overflow.insert(Reverse(result.err().unwrap()));
                        }
                    }
                    _ => {}
                }
            }
        }
        if rollback != u64::MAX {
            self.rollback(rollback)?;
            for i in self.in_queue.as_mut() {
                if *i != Transferable::Nan {
                    let msg = std::mem::replace(i, Transferable::Nan);
                    match msg {
                        Transferable::Message(msg) => {
                            let result = self.scheduler.insert(Object::Message(msg));
                            if result.is_err() {
                                self.overflow.insert(Reverse(result.err().unwrap()));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        // increment
        match self.scheduler.tick() {
            Ok(events) => {
                let mut result = self.check_annihilation();
                for event in events {
                    match event {
                        Object::Event(event) => {
                            if event.time as f64 * self.scheduler.time.timestep
                                > self.scheduler.time.terminal.unwrap_or(f64::INFINITY)
                            {
                                break;
                            }
                            let event = self.agent.step(&event.time, &mut self.state);

                            match event.yield_ {
                                Action::Timeout(time) => {
                                    if (self.scheduler.time.step + time) as f64
                                        * self.scheduler.time.timestep
                                        > self.scheduler.time.terminal.unwrap_or(f64::INFINITY)
                                    {
                                        continue;
                                    }

                                    self.commit(Object::Event(Event::new(
                                        self.scheduler.time.step,
                                        self.scheduler.time.step + time,
                                        event.agent,
                                        Action::Wait,
                                    )));
                                }
                                Action::Schedule(time) => {
                                    self.commit(Object::Event(Event::new(
                                        self.scheduler.time.step,
                                        time,
                                        event.agent,
                                        Action::Wait,
                                    )));
                                }
                                Action::Trigger { time, idx } => {
                                    self.commit(Object::Event(Event::new(
                                        self.scheduler.time.step,
                                        time,
                                        idx,
                                        Action::Wait,
                                    )));
                                }
                                Action::Wait => {}
                                Action::Break => {
                                    break;
                                }
                            }
                        }
                        Object::Message(msg) => {
                            let mut brk = false;
                            if result.as_ref().is_err() {
                                let antis = result.as_mut().err().unwrap();
                                let lena = antis.len();
                                for i in 0..lena {
                                    if antis[i].annihilate(&msg) {
                                        antis.remove(i);
                                        brk = true;
                                        break;
                                    }
                                }
                            }
                            if brk {
                                continue;
                            }
                            let response = self.agent.process_message(
                                msg,
                                self.scheduler.time.step,
                                &mut self.state,
                            );
                            match response {
                                HandlerOutput::Event(event) => {
                                    self.commit(Object::Event(event));
                                }
                                HandlerOutput::Messages(anni) => {
                                    self.out_antimessages.push(anni.1);
                                    if anni.0.to == anni.0.from {
                                        self.commit(Object::Message(anni.0));
                                    } else {
                                        let wresult =
                                            self.write_outgoing(Transferable::Message(anni.0));
                                        if wresult.is_err() {
                                            self.out_queue.insert(Reverse(wresult.err().unwrap()));
                                        }
                                    }
                                }
                                HandlerOutput::Nan => {}
                            }
                        }
                    }
                }
            }
            Err(err) => match err {
                SimError::TimeTravel => return Err(err),
                _ => {}
            },
        };
        self.scheduler.increment(&mut self.overflow);
        Ok(())
    }
    /// check if a message needs annihilating.
    fn check_annihilation(&mut self) -> Result<(), Vec<AntiMessage>> {
        if self.in_times.contains(&self.scheduler.time.step) {
            let mut vec = Vec::new();
            let size = self.in_times.len();
            for _i in 0..size {
                let anti = self
                    .in_times
                    .iter()
                    .rposition(|&x| x == self.scheduler.time.step)
                    .unwrap();
                vec.push(self.in_antimessages.remove(anti))
            }
            return Err(vec);
        }
        Ok(())
    }
    /// Run the logical process
    pub fn run(&mut self) -> Result<(), SimError> {
        loop {
            if self.scheduler.time.step as f64 * self.scheduler.time.timestep
                >= self.scheduler.time.terminal.unwrap_or(f64::INFINITY)
            {
                break;
            }
            self.step()?;
            self.step
                .store(self.scheduler.time.step as usize, Ordering::Release);
        }
        Ok(())
    }
}
