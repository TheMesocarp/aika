// todo! spsc circular buffer with atomics for notifying thread2thread communications
use std::sync::atomic::AtomicBool;

pub struct Buffer {
    ptr: *mut u8,
    size: usize,
    notify: AtomicBool,
}

unsafe impl Send for Buffer {}
unsafe impl Sync for Buffer {}

pub struct CircularBuffer<const LPS: usize> {
    wheel: [[Buffer; LPS]; 2],

}