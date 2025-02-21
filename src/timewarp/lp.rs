use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::clock::Clock;
use crate::clock::Scheduleable;
use crate::logger::Lumi;
use crate::worlds::Agent;
use crate::worlds::Event;
use crate::worlds::Message;
use crate::worlds::SimError;

use super::antimessage::AntiMessage;
use super::comms::CircularBuffer;
use super::comms::Transferable;

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

pub struct LP<const SLOTS: usize, const HEIGHT: usize, const SIZE: usize> {
    pub scheduler: Clock<Object, SLOTS, HEIGHT>,
    pub state: Lumi,
    pub antimessages: Vec<AntiMessage>,
    pub buffers: [CircularBuffer<SIZE>; 2],
    pub agent: Box<dyn Agent>,
    pub step: Arc<AtomicUsize>,
    pub rollbacks: usize,
    pub id: usize,
}

impl<const SLOTS: usize, const HEIGHT: usize, const SIZE: usize> LP<SLOTS, HEIGHT, SIZE> {
    pub fn new<T: 'static>(
        id: usize,
        agent: Box<dyn Agent>,
        timestep: f64,
        step: Arc<AtomicUsize>,
        buffers: [CircularBuffer<SIZE>; 2],
        log_slots: usize
    ) -> Self {
        LP {
            scheduler: Clock::<Object, SLOTS, HEIGHT>::new(timestep, None).unwrap(),
            state: Lumi::initialize::<T>(log_slots),
            antimessages: Vec::new(),
            buffers,
            agent,
            step,
            rollbacks: 0,
            id,
        }
    }

    pub fn read_incoming(&mut self) -> Vec<Transferable> {
        let mut all_msgs = Vec::<Transferable>::new();
        let circular = &self.buffers[0];
        let mut r = circular.read_idx.load(Ordering::Acquire);
        let w = circular.write_idx.load(Ordering::Acquire);
        loop {
            if r == w {
                return all_msgs;
            }
            let msg = unsafe { (*circular.ptr)[r].take().unwrap() };
            all_msgs.push(msg);
            r = (r + 1) % SIZE;
        }
    }

    pub fn write_outgoing(&mut self, msg: Transferable) -> Result<(), SimError> {
        let circular = &self.buffers[1];
        let w = circular.write_idx.load(Ordering::Acquire);
        let r = circular.read_idx.load(Ordering::Acquire);
        let next = (w + 1) % SIZE;
        if next == r {
            return Err(SimError::MailboxFull);
        }
        unsafe {
            (*circular.ptr)[w] = Some(msg);
        }
        circular.write_idx.store(next, Ordering::Release);
        Ok(())
    }


}