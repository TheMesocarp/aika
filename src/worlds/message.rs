use std::cmp::Ordering;

use crate::clock::Scheduleable;

/// A message that can be sent between agents
#[derive(Debug)]
pub struct Message {
    pub data: *const u8,
    pub sent: u64,
    pub received: u64,
    pub from: usize,
    pub to: usize,
}

unsafe impl Send for Message {}
unsafe impl Sync for Message {}

impl Message {
    pub fn new(data: *const u8, sent: u64, received: u64, from: usize, to: usize) -> Self {
        Message {
            data,
            sent,
            received,
            from,
            to,
        }
    }
}

impl PartialEq for Message {
    fn eq(&self, other: &Self) -> bool {
        self.sent == other.sent && self.received == other.received
    }
}

impl Eq for Message {}

impl PartialOrd for Message {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Message {
    fn cmp(&self, other: &Self) -> Ordering {
        self.received.partial_cmp(&other.received).unwrap()
    }
}

impl Scheduleable for Message {
    fn time(&self) -> u64 {
        self.received
    }
    fn commit_time(&self) -> u64 {
        self.sent
    }
}
