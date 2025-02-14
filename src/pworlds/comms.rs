// todo! spsc circular buffer with atomics for notifying thread2thread communications
use std::sync::{atomic::{AtomicBool, AtomicUsize}, Arc};

use crate::worlds::{Message, SimError};

pub struct CircularBuffer<const SIZE: usize> {
    ptr: *mut [Option<Message>; SIZE],
    write_idx: Arc<AtomicUsize>,
    read_idx: Arc<AtomicUsize>,
    ready: Vec<Arc<AtomicBool>>,
}


unsafe impl<const SIZE: usize> Send for CircularBuffer<SIZE> {}
unsafe impl<const SIZE: usize> Sync for CircularBuffer<SIZE> {}

pub struct Comms<const LPS: usize, const SIZE: usize> {
    // layer 0 of the wheel is for reading inmsg -> GVT, layer 1 is for writing GVT -> outmsg
    wheel: [[CircularBuffer<SIZE>; LPS]; 2],
}

impl<const LPS: usize, const SIZE: usize> Comms<LPS, SIZE> {
    pub fn new(wheel: [[CircularBuffer<SIZE>; LPS]; 2]) -> Self {
        Comms {
            wheel,
        }
    }

    pub fn write(&mut self, msg: Message) -> Result<(), SimError> {
        let target = msg.to;
        let cbuff = &mut self.wheel[1][target];
        let idx = cbuff.write_idx.load(std::sync::atomic::Ordering::Acquire);
        if cbuff.ready[idx].load(std::sync::atomic::Ordering::Acquire) {
            return Err(SimError::CircularBufferFull);
        } else if cbuff.read_idx.load(std::sync::atomic::Ordering::Acquire) == idx {
            return Err(SimError::CircularBufferFull);
        }
        unsafe {
            let _ = std::mem::replace(&mut (*cbuff.ptr)[idx], Some(msg));
            cbuff.write_idx.store((idx + 1) % SIZE, std::sync::atomic::Ordering::Release);
            cbuff.ready[idx].store(true, std::sync::atomic::Ordering::Release);
            Ok(())
        }
    } 

    pub fn read(&mut self, target: usize) -> Result<Message, SimError> {
        let cbuff = &mut self.wheel[0][target];
        let idx = cbuff.read_idx.load(std::sync::atomic::Ordering::Acquire);
        if !cbuff.ready[idx].load(std::sync::atomic::Ordering::Acquire) {
            return Err(SimError::CircularBufferEmpty);
        } else if cbuff.write_idx.load(std::sync::atomic::Ordering::Acquire) == idx {
            return Err(SimError::CircularBufferEmpty);
        }
        unsafe { 
            let msg = std::mem::replace(&mut (*cbuff.ptr)[idx], None).unwrap();
            cbuff.read_idx.store((idx + 1) % SIZE, std::sync::atomic::Ordering::Release);
            cbuff.ready[idx].store(false, std::sync::atomic::Ordering::Release);
            Ok(msg)
        }
    }

    pub fn poll(&self) -> Result<[bool; LPS], SimError> {
        let mut ready = [false; LPS];
        for i in 0..LPS {
            for j in 0..SIZE {
                if self.wheel[0][i].ready[j].load(std::sync::atomic::Ordering::Acquire) {
                    ready[i] = true;
                    break;
                }
            }
        }
        Ok(ready)
    }

    pub fn flush(&mut self) {
        for i in 0..LPS {
            for j in 0..SIZE {
                self.wheel[0][i].ready[j].store(false, std::sync::atomic::Ordering::Release);
            }
        }
    }
}