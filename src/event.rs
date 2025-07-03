use std::{cmp::{Ordering, Reverse}, collections::BinaryHeap};

use bytemuck::{Pod, Zeroable};

use mesocarp::scheduling::{htw::Clock, Scheduleable};

use crate::SimError;

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

unsafe impl Send for Event {}
unsafe impl Sync for Event {}


pub struct LocalEventSystem<const CLOCK_SLOTS: usize, const CLOCK_HEIGHT: usize> {
    pub overflow: BinaryHeap<Reverse<Event>>,
    pub local_clock: Clock<Event, CLOCK_SLOTS, CLOCK_HEIGHT>,
}

impl<const CLOCK_SLOTS: usize, const CLOCK_HEIGHT: usize>
    LocalEventSystem<CLOCK_SLOTS, CLOCK_HEIGHT>
{
    pub fn new() -> Result<Self, SimError> {
        let overflow = BinaryHeap::new();
        let local_clock = Clock::new().map_err(SimError::MesoError)?;
        Ok(Self {
            overflow,
            local_clock,
        })
    }

    pub fn insert(&mut self, event: Event) {
        let possible_overflow = self.local_clock.insert(event);
        if possible_overflow.is_err() {
            let event = possible_overflow.err().unwrap();
            self.overflow.push(Reverse(event));
        }
    }
}

unsafe impl<const CLOCK_SLOTS: usize, const CLOCK_HEIGHT: usize> Send for LocalEventSystem<CLOCK_SLOTS, CLOCK_HEIGHT> {}
unsafe impl<const CLOCK_SLOTS: usize, const CLOCK_HEIGHT: usize> Sync for LocalEventSystem<CLOCK_SLOTS, CLOCK_HEIGHT> {}