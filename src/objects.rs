//! Core data structures for simulation including messages, events, and scheduling primitives.
//! Contains `Msg` for inter-agent communication, `Event` for agent scheduling, `AntiMsg` for
//! optimistic rollback, and local event/mail systems for efficient time-based scheduling.
use std::{
    cmp::{Ordering, Reverse},
    collections::BinaryHeap,
};

use bytemuck::{Pod, Zeroable};
use mesocarp::{
    comms::mailbox::Message,
    scheduling::{htw::Clock, Scheduleable},
};

use crate::AikaError;

/// A `Msg` is a direct message between two entities that shares a piece of data of type T
#[derive(Copy, Clone, Debug)]
pub struct Msg<T: Clone> {
    pub from: usize,
    pub to: Option<usize>,
    pub sent: u64,
    pub recv: u64,
    pub data: T,
}

impl<T: Clone> Msg<T> {
    /// Create a new `Msg`. If `to: Option<usize>` is set to None, the `Msg` will be broadcasted to all entities.
    pub fn new(data: T, sent: u64, recv: u64, from: usize, to: Option<usize>) -> Self {
        Self {
            from,
            to,
            sent,
            recv,
            data,
        }
    }
}

impl<T: Clone> Message for Msg<T> {
    fn to(&self) -> Option<usize> {
        self.to
    }

    fn from(&self) -> usize {
        self.from
    }
}

impl<T: Clone> Scheduleable for Msg<T> {
    fn time(&self) -> u64 {
        self.recv
    }

    fn commit_time(&self) -> u64 {
        self.sent
    }
}

impl<T: Clone> PartialOrd for Msg<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: Clone> PartialEq for Msg<T> {
    fn eq(&self, other: &Self) -> bool {
        self.from == other.from
            && self.to == other.to
            && self.sent == other.sent
            && self.recv == other.recv
    }
}

impl<T: Clone> Eq for Msg<T> {}

impl<T: Clone> Ord for Msg<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.recv
            .cmp(&other.recv)
            .then_with(|| self.sent.cmp(&other.sent))
            .then_with(|| self.from.cmp(&other.from))
            .then_with(|| self.to.cmp(&other.to))
    }
}

#[derive(Debug, Copy, Clone)]
/// An `AntiMsg` allows you to directly cancel messages with the same metadata in an optimistic execution environment
pub struct AntiMsg {
    pub sent: u64,
    pub received: u64,
    pub from: usize,
    pub to: Option<usize>,
}

impl AntiMsg {
    /// Create a new `AntiMsg`. Note that you won't need to manual call this to maintain synchronization, this is just for flexibility.
    pub fn new(sent: u64, received: u64, from: usize, to: Option<usize>) -> Self {
        AntiMsg {
            sent,
            received,
            from,
            to,
        }
    }

    /// Annihilate a `Msg<T>` and `AntiMsg` pair.
    pub fn annihilate<T: Clone>(&self, other: &Msg<T>) -> bool {
        self.sent == other.sent
            && self.received == other.recv
            && self.from == other.from
            && self.to == other.to
    }
}

impl PartialEq for AntiMsg {
    fn eq(&self, other: &Self) -> bool {
        self.sent == other.sent && self.received == other.received
    }
}

impl Eq for AntiMsg {}

impl PartialOrd for AntiMsg {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for AntiMsg {
    fn cmp(&self, other: &Self) -> Ordering {
        self.received.partial_cmp(&other.received).unwrap()
    }
}

impl Scheduleable for AntiMsg {
    fn time(&self) -> u64 {
        self.received
    }
    fn commit_time(&self) -> u64 {
        self.sent
    }
}

impl Message for AntiMsg {
    fn to(&self) -> Option<usize> {
        self.to
    }

    fn from(&self) -> usize {
        self.from
    }
}

unsafe impl Pod for AntiMsg {}
unsafe impl Zeroable for AntiMsg {}

/// A `Message` and `AntiMessage` aannihilate each other if they encounter again after creation.
pub struct Annihilator<T: Clone>(pub Msg<T>, pub AntiMsg);

impl<T: Clone> Annihilator<T> {
    /// conjure an annihilator pair
    pub fn conjure(
        creation_time: u64,
        from_id: usize,
        to_id: Option<usize>,
        process_time: u64,
        data: T,
    ) -> Self {
        let msg = Msg::new(data, creation_time, process_time, from_id, to_id);
        let anti = AntiMsg::new(creation_time, process_time, from_id, to_id);
        Self(msg, anti)
    }
}

/// An object that can be transfered between `Planet` threads during optimistic execution
#[derive(Debug, Clone, Copy)]
pub enum Transfer<T: Pod + Zeroable + Clone> {
    Msg(Msg<T>),
    AntiMsg(AntiMsg),
}

impl<T: Pod + Zeroable + Clone> Message for Transfer<T> {
    fn to(&self) -> Option<usize> {
        match self {
            Transfer::Msg(msg) => msg.to(),
            Transfer::AntiMsg(anti_msg) => anti_msg.to(),
        }
    }

    fn from(&self) -> usize {
        match self {
            Transfer::Msg(msg) => msg.from(),
            Transfer::AntiMsg(anti_msg) => anti_msg.from(),
        }
    }
}

impl<T: Pod + Zeroable + Clone> Scheduleable for Transfer<T> {
    fn time(&self) -> u64 {
        match self {
            Transfer::Msg(msg) => msg.time(),
            Transfer::AntiMsg(anti_msg) => anti_msg.time(),
        }
    }

    fn commit_time(&self) -> u64 {
        match self {
            Transfer::Msg(msg) => msg.commit_time(),
            Transfer::AntiMsg(anti_msg) => anti_msg.commit_time(),
        }
    }
}

impl<T: Pod + Zeroable + Clone> PartialOrd for Transfer<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: Pod + Zeroable + Clone> PartialEq for Transfer<T> {
    fn eq(&self, other: &Self) -> bool {
        self.from() == other.from()
            && self.to() == other.to()
            && self.commit_time() == other.commit_time()
            && self.time() == other.time()
    }
}

impl<T: Pod + Zeroable + Clone> Eq for Transfer<T> {}

impl<T: Pod + Zeroable + Clone> Ord for Transfer<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.time().cmp(&other.time())
    }
}

unsafe impl<T: Pod + Zeroable + Clone> Send for Transfer<T> {}
unsafe impl<T: Pod + Zeroable + Clone> Sync for Transfer<T> {}

unsafe impl<T: Pod + Zeroable + Clone> Pod for Transfer<T> {}
unsafe impl<T: Pod + Zeroable + Clone> Zeroable for Transfer<T> {}

/// Inter-planetary `Mail` carry data of type `T` for optimistic execution environments
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Mail<T: Pod + Zeroable + Clone> {
    pub transfer: Transfer<T>,
    pub to_world: Option<usize>,
    pub from_world: usize,
}

impl<T: Pod + Zeroable + Clone> Mail<T> {
    /// Create a new peice of `Mail`. if `to_world: Option<usize>` is set to `None`, the `Mail` broadcasts
    pub fn write_letter(transfer: Transfer<T>, from_world: usize, to_world: Option<usize>) -> Self {
        Self {
            transfer,
            to_world,
            from_world,
        }
    }
    /// Consume to receive a `Transfer`
    pub fn open_letter(self) -> Transfer<T> {
        self.transfer
    }
}

impl<T: Pod + Zeroable + Clone> Message for Mail<T> {
    fn to(&self) -> Option<usize> {
        self.to_world
    }

    fn from(&self) -> usize {
        self.from_world
    }
}

unsafe impl<T: Pod + Zeroable + Clone> Pod for Mail<T> {}
unsafe impl<T: Pod + Zeroable + Clone> Zeroable for Mail<T> {}

pub(crate) struct LocalMailSystem<
    const CLOCK_SLOTS: usize,
    const CLOCK_HEIGHT: usize,
    MessageType: Clone,
> {
    pub(crate) overflow: BinaryHeap<Reverse<Msg<MessageType>>>,
    pub(crate) schedule: Clock<Msg<MessageType>, CLOCK_SLOTS, CLOCK_HEIGHT>,
}

impl<const CLOCK_SLOTS: usize, const CLOCK_HEIGHT: usize, MessageType: Clone>
    LocalMailSystem<CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
    pub(crate) fn new() -> Result<Self, AikaError> {
        let overflow = BinaryHeap::new();
        let schedule = Clock::new()?;
        Ok(Self { overflow, schedule })
    }
}

unsafe impl<const CLOCK_SLOTS: usize, const CLOCK_HEIGHT: usize, MessageType: Clone> Send
    for LocalMailSystem<CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
}
unsafe impl<const CLOCK_SLOTS: usize, const CLOCK_HEIGHT: usize, MessageType: Clone> Sync
    for LocalMailSystem<CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
}

/// A scheduling action that an `Agent` or `ThreadedAgent` can take.
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

pub(crate) struct LocalEventSystem<const CLOCK_SLOTS: usize, const CLOCK_HEIGHT: usize> {
    pub(crate) overflow: BinaryHeap<Reverse<Event>>,
    pub(crate) local_clock: Clock<Event, CLOCK_SLOTS, CLOCK_HEIGHT>,
}

impl<const CLOCK_SLOTS: usize, const CLOCK_HEIGHT: usize>
    LocalEventSystem<CLOCK_SLOTS, CLOCK_HEIGHT>
{
    pub(crate) fn new() -> Result<Self, AikaError> {
        let overflow = BinaryHeap::new();
        let local_clock = Clock::new()?;
        Ok(Self {
            overflow,
            local_clock,
        })
    }

    pub(crate) fn insert(&mut self, event: Event) {
        let possible_overflow = self.local_clock.insert(event);
        if possible_overflow.is_err() {
            let event = possible_overflow.err().unwrap();
            self.overflow.push(Reverse(event));
        }
    }
}

unsafe impl<const CLOCK_SLOTS: usize, const CLOCK_HEIGHT: usize> Send
    for LocalEventSystem<CLOCK_SLOTS, CLOCK_HEIGHT>
{
}
unsafe impl<const CLOCK_SLOTS: usize, const CLOCK_HEIGHT: usize> Sync
    for LocalEventSystem<CLOCK_SLOTS, CLOCK_HEIGHT>
{
}
