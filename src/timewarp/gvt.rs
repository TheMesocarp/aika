// GVT/Coordinator Thread,
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use std::thread::*;


pub struct GVT {
    global_time: Arc<AtomicUsize>
}
