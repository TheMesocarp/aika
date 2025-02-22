// spsc circular buffer with atomics for notifying thread2thread communications
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use crate::worlds::{Message, SimError};

use super::antimessage::AntiMessage;

#[derive(Debug)]
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

#[derive(Debug)]
pub struct CircularBuffer<const SIZE: usize> {
    pub ptr: *mut [Option<Transferable>; SIZE],
    pub write_idx: Arc<AtomicUsize>,
    pub read_idx: Arc<AtomicUsize>,
}

unsafe impl<const SIZE: usize> Send for CircularBuffer<SIZE> {}
unsafe impl<const SIZE: usize> Sync for CircularBuffer<SIZE> {}

pub struct Comms<const LPS: usize, const SIZE: usize> {
    // layer 0 of the wheel is for reading inmsg -> GVT, layer 1 is for writing GVT -> outmsg
    wheel: [[CircularBuffer<SIZE>; LPS]; 2],
}

impl<const LPS: usize, const SIZE: usize> Comms<LPS, SIZE> {
    pub fn new(wheel: [[CircularBuffer<SIZE>; LPS]; 2]) -> Self {
        Comms { wheel }
    }

    pub fn write(&mut self, msg: Transferable) -> Result<(), Transferable> {
        let target = msg.to();
        let cbuff = &mut self.wheel[1][target];
        let w = cbuff.write_idx.load(Ordering::Acquire);
        let r = cbuff.read_idx.load(Ordering::Acquire);
        let next = (w + 1) % SIZE;
        if next == r {
            return Err(msg);
        }
        unsafe {
            (*cbuff.ptr)[w] = Some(msg);
        }
        // publish by storing next
        cbuff.write_idx.store(next, Ordering::Release);
        Ok(())
    }

    pub fn read(&mut self, target: usize) -> Result<Transferable, SimError> {
        let cbuff = &mut self.wheel[0][target];
        let w = cbuff.write_idx.load(Ordering::Acquire);
        let r = cbuff.read_idx.load(Ordering::Acquire);
        if w == r {
            return Err(SimError::MailboxEmpty);
        }
        let msg = unsafe { (*cbuff.ptr)[r].take().unwrap() };
        cbuff.read_idx.store((r + 1) % SIZE, Ordering::Release);
        Ok(msg)
    }

    pub fn poll(&self) -> Result<[bool; LPS], SimError> {
        let mut ready = [false; LPS];
        for i in 0..LPS {
            let read = self.wheel[0][i].read_idx.load(Ordering::Acquire);
            let write = self.wheel[0][i].write_idx.load(Ordering::Acquire);
            if read != write {
                ready[i] = true;
            }
        }
        Ok(ready)
    }

    pub fn flush(&mut self) {
        for i in 0..LPS {
            self.wheel[0][i].read_idx.store(
                self.wheel[0][i].write_idx.load(Ordering::Acquire),
                Ordering::Release,
            );
        }
    }
}
