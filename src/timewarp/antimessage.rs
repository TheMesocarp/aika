use std::cmp::Ordering;

use crate::{clock::Scheduleable, worlds::Message};

#[derive(Debug, Clone)]
/// A message that can be sent between agents.
pub struct AntiMessage {
    pub sent: u64,
    pub received: u64,
    pub from: usize,
    pub to: usize,
}

impl AntiMessage {
    pub fn new(sent: u64, received: u64, from: usize, to: usize) -> Self {
        AntiMessage {
            sent,
            received,
            from,
            to,
        }
    }

    pub fn annihilate(&self, other: &Message) -> bool {
        self.sent == other.sent && self.received == other.received && self.from == other.from
    }
}

impl PartialEq for AntiMessage {
    fn eq(&self, other: &Self) -> bool {
        self.sent == other.sent && self.received == other.received
    }
}

impl Eq for AntiMessage {}

impl PartialOrd for AntiMessage {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for AntiMessage {
    fn cmp(&self, other: &Self) -> Ordering {
        self.received.partial_cmp(&other.received).unwrap()
    }
}

impl Scheduleable for AntiMessage {
    fn time(&self) -> u64 {
        self.received
    }
    fn commit_time(&self) -> u64 {
        self.sent
    }
}

/// A `Message` and `AntiMessage` aannihilate each other if they encounter again after creation.
pub struct Annihilator(pub Message, pub AntiMessage);

impl Annihilator {
    /// conjure an annihilator pair
    pub fn conjure<T: 'static>(
        creation_time: u64,
        from_id: usize,
        to_id: usize,
        process_time: u64,
        data: &T,
    ) -> Self {
        let ptr = data as *const T as *const u8;
        let msg = Message::new(ptr, creation_time, process_time, from_id, to_id);
        let anti = AntiMessage::new(creation_time, process_time, from_id, to_id);
        Self(msg, anti)
    }
}
