use core::time;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use crate::clock::Scheduleable;
use crate::worlds::Agent;
use crate::worlds::Event;
use crate::clock::Clock;
use crate::worlds::Message;

use super::antimessage::AntiMessage;

pub enum Object {
    Event(Event),
    Message(Message),
    AntiMessage(AntiMessage),
}

impl Scheduleable for Object {
    fn time(&self) -> u64 {
        match self {
            Object::Event(e) => e.time,
            Object::Message(m) => m.received,
            Object::AntiMessage(am) => am.received,
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

pub struct LP<const SLOTS: usize, const HEIGHT: usize> {
    pub scheduler: Clock<Object, SLOTS, HEIGHT>,
    pub state: Option<Vec<u8>>,
    pub history: Option<Vec<Vec<u8>>>,
    pub antimessages: Vec<AntiMessage>,
    pub agent: Box<dyn Agent>,
    pub step: Arc<AtomicUsize>,
    pub rollbacks: usize,
    pub id: usize,
}

impl<const SLOTS: usize, const HEIGHT: usize> LP<SLOTS, HEIGHT> {
    pub fn new(id: usize, agent: Box<dyn Agent>, timestep: f64, init_state: Option<Vec<u8>>, step: Arc<AtomicUsize>) -> Self {
        let history = if init_state.is_some() { Some(Vec::<Vec<u8>>::new()) } else { None };
        LP {
            scheduler: Clock::<Object, SLOTS, HEIGHT>::new(timestep, None).unwrap(),
            state: init_state,
            history,
            antimessages: Vec::new(),
            agent,
            step,
            rollbacks: 0,
            id,
        }
    }
}