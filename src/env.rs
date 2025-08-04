use downcast_rs::{impl_downcast, Downcast};
use mesocarp::logging::journal::Journal;

pub trait Environment: Downcast + std::fmt::Debug {
    fn rollback(&mut self, time: u64);
}
impl_downcast!(Environment);

#[derive(Debug)]
pub struct Stateless;

impl Environment for Stateless {
    fn rollback(&mut self, _time: u64) {}
}

#[derive(Debug)]
pub struct SimpleUnified {
    pub inner: Journal
}

impl Environment for SimpleUnified {
    fn rollback(&mut self, time: u64) {
        self.inner.rollback(time);
    }
}
