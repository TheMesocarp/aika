use std::cmp::Ordering;

use mesocarp::{comms::mailbox::Message, scheduling::Scheduleable};

#[derive(Clone, Debug)]
pub struct Msg<T: Clone> {
    pub from: usize,
    pub to: Option<usize>,
    pub sent: u64,
    pub recv: u64,
    pub data: T
}

impl<T: Clone> Msg<T> {
    pub fn new(data: T, sent: u64, recv: u64, from: usize, to: Option<usize>) -> Self {
        Self {
            from,
            to,
            sent,
            recv,
            data
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

    fn broadcast(&self) -> bool {
        if self.to.is_none() {
            true
        } else {
            false
        }
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
        self.recv.partial_cmp(&other.recv)
    }
}

impl<T: Clone> PartialEq for Msg<T> {
    fn eq(&self, other: &Self) -> bool {
        self.from == other.from && self.to == other.to && self.sent == other.sent && self.recv == other.recv
    }
}

impl<T: Clone> Eq for Msg<T> {}

impl<T: Clone> Ord for Msg<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.recv.cmp(&other.recv)
    }
}


#[derive(Debug, Clone)]
/// A message that can be sent between agents.
pub struct AntiMsg {
    pub sent: u64,
    pub received: u64,
    pub from: usize,
    pub to: usize,
}

impl AntiMsg {
    pub fn new(sent: u64, received: u64, from: usize, to: usize) -> Self {
        AntiMsg {
            sent,
            received,
            from,
            to,
        }
    }

    pub fn annihilate<T: Clone>(&self, other: &Msg<T>) -> bool {
        self.sent == other.sent && self.received == other.recv && self.from == other.from
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
        Some(self.to)
    }

    fn from(&self) -> usize {
        self.from
    }

    fn broadcast(&self) -> bool {
        false
    }
}

/// A `Message` and `AntiMessage` aannihilate each other if they encounter again after creation.
pub struct Annihilator<T: Clone>(pub Msg<T>, pub AntiMsg);

impl<T: Clone> Annihilator<T> {
    /// conjure an annihilator pair
    pub fn conjure(
        creation_time: u64,
        from_id: usize,
        to_id: usize,
        process_time: u64,
        data: T,
    ) -> Self {
        let msg = Msg::new(data, creation_time, process_time, from_id, Some(to_id));
        let anti = AntiMsg::new(creation_time, process_time, from_id, to_id);
        Self(msg, anti)
    }
}