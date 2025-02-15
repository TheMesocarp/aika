use std::{ffi::c_void, ptr::{null, null_mut}};

use crate::worlds::Event;

/// A logger for recording snapshots of the world.
pub struct Logger {
    pub astates: Vec<History>,
    pub gstates: History,
    events: Vec<Event>,
}

pub struct History(pub Vec<(*mut c_void, u64)>);

pub struct ThisSucks<T>(pub Vec<T>);

pub fn update<T>(history: &mut History, statelogs: &mut ThisSucks<T>, old: *mut c_void, new: T, step: &u64) {
    let mut old = unsafe {std::mem::replace(&mut *(old as *mut T), new)};
    let ptr = &mut old as *mut T as *mut _ as *mut c_void;
    history.0.push((ptr, *step));
    statelogs.0.push(old);
}

impl Logger {
    pub fn new() -> Self {
        Logger {
            astates: Vec::new(),
            gstates: History(Vec::new()),
            events: Vec::new(),
        }
    }

    pub fn log_global(&mut self, state: *mut c_void, step: u64) {
        self.gstates.0.push((state, step));
    }
    pub fn log_event(&mut self, event: Event) {
        self.events.push(event);
    }

    pub fn get_events(&self) -> Vec<Event> {
        self.events.clone()
    }

    pub fn latest(&self) -> u64 {
        let mut last = if self.gstates.0.last().is_none() {
            &(null_mut(), 0)
        } else {
            self.gstates.0.last().unwrap()
        };
        for i in 0..self.astates.len() {
            let astates = &self.astates[i].0;

            let last_astate = astates.last();
            if last_astate.is_none() {
                continue;
            }
            if last_astate.unwrap().1 > last.1 {
                last = last_astate.unwrap();
            }
        }
        last.1
    }
}