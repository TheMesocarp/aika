use std::cmp::Ordering;

use bytemuck::{Pod, Zeroable};

use crate::clock::Scheduleable;

/// A scheduling action that an agent can take.
#[derive(Copy, Clone, Debug)]
pub enum Action {
    Timeout(u64),
    Schedule(u64),
    Trigger { time: u64, idx: usize },
    Wait,
    Break,
}

/// An event that can be scheduled in a simulation. This is used to trigger an agent, or schedule another event.
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct Event {
    pub time: u64,
    pub commit_time: u64,
    pub agent: usize,
    pub yield_: Action,
}

impl Event {
    pub fn new(commit_time: u64, time: u64, agent: usize, yield_: Action) -> Self {
        Self {
            commit_time,
            time,
            agent,
            yield_,
        }
    }

    pub fn time(&self) -> u64 {
        self.time
    }
}

impl PartialEq for Event {
    fn eq(&self, other: &Self) -> bool {
        self.time == other.time
    }
}
impl Eq for Event {}

impl PartialOrd for Event {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Event {
    fn cmp(&self, other: &Self) -> Ordering {
        self.time.partial_cmp(&other.time).unwrap()
    }
}

impl Scheduleable for Event {
    fn time(&self) -> u64 {
        self.time
    }
    fn commit_time(&self) -> u64 {
        self.commit_time
    }
}

unsafe impl Zeroable for Event {}

unsafe impl Pod for Event {}