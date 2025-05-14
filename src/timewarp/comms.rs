// spsc circular buffer with atomics for notifying thread2thread communications
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use mesocarp::concurrency::spsc::BufferWheel;

use crate::worlds::{Message, SimError};

use super::antimessage::AntiMessage;

#[derive(Debug, Clone)]
/// `Message` and `AntiMessage` wrapper
pub enum Transferable {
    Message(Message),
    AntiMessage(AntiMessage),
    Nan,
}

unsafe impl Send for Transferable {}
unsafe impl Sync for Transferable {}

impl Transferable {
    pub fn to(&self) -> usize {
        match self {
            Transferable::Message(m) => m.to,
            Transferable::AntiMessage(am) => am.to,
            Transferable::Nan => 0,
        }
    }
    pub fn received(&self) -> u64 {
        match self {
            Transferable::Message(m) => m.received,
            Transferable::AntiMessage(am) => am.received,
            Transferable::Nan => u64::MAX,
        }
    }
    pub fn commit_time(&self) -> u64 {
        match self {
            Transferable::Message(m) => m.sent,
            Transferable::AntiMessage(am) => am.sent,
            Transferable::Nan => u64::MAX,
        }
    }
}

impl PartialEq for Transferable {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Transferable::Message(m1), Transferable::Message(m2)) => {
                if m1.received == m2.received {
                    true
                } else {
                    false
                }
            }
            (Transferable::AntiMessage(m1), Transferable::AntiMessage(m2)) => {
                if m1.received == m2.received {
                    true
                } else {
                    false
                }
            }
            (Transferable::Nan, Transferable::Nan) => true,
            _ => false,
        }
    }
}

impl Eq for Transferable {}

impl PartialOrd for Transferable {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Transferable {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.received().partial_cmp(&other.received()).unwrap()
    }
}

/// Full communication hub using 2 circular buffers per LP to avoid contention for incoming and outgoing messages.
/// Meant to be housed by the GVT
pub struct Comms<const LPS: usize, const SIZE: usize> {
    // layer 0 of the wheel is for reading inmsg -> GVT, layer 1 is for writing GVT -> outmsg
    wheel: [[Arc<BufferWheel<SIZE, Transferable>>; LPS]; 2],
}

impl<const LPS: usize, const SIZE: usize> Comms<LPS, SIZE> {
    /// new Comms hub for the GVT
    pub fn new(wheel: [[Arc<BufferWheel<SIZE, Transferable>>; LPS]; 2]) -> Self {
        Comms { wheel }
    }
    /// Write a message to the respective buffer
    pub fn write(&mut self, msg: Transferable) -> Result<(), Transferable> {
        let target = msg.to();
        let cbuff = &mut self.wheel[1][target];
        cbuff.write(msg.clone()).map_err(|_| msg)
    }
    /// read a particular LP's mailbox for outgoing messages or antimessages.
    pub fn read(&mut self, target: usize) -> Result<Transferable, SimError> {
        let cbuff = &mut self.wheel[0][target];
        cbuff.read().map_err(|err| SimError::Mesocarp(format!("{err:?}")))

    }
    /// poll atomics for any outgoing messages that need processing
    pub fn poll(&mut self) -> Result<[Option<Transferable>; LPS], SimError> {
        let mut ready = [const { None }; LPS];
        for i in 0..LPS {
            let msg = self.read(i);
            if msg.is_ok() {
                ready[i] = Some(msg.unwrap())
            }
        }
        Ok(ready)
    }
    /// reset the comms wheel indexes.
    pub fn flush(&mut self) {
        for i in 0..LPS {
            self.wheel[0][i] =  Arc::new(BufferWheel::new());
            self.wheel[1][i] =  Arc::new(BufferWheel::new());
        }
    }
}
