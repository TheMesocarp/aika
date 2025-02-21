use std::{
    alloc::{alloc, dealloc, Layout},
    any::TypeId,
    cmp::Ordering,
    mem,
    ptr::{self, drop_in_place},
};

use crate::worlds::{Event, SimError};

pub struct Log<T: 'static>(T, u64);

impl<T: 'static> PartialEq for Log<T> {
    fn eq(&self, other: &Self) -> bool {
        self.1 == other.1
    }
}

impl<T: 'static> Eq for Log<T> {}

impl<T: 'static> PartialOrd for Log<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: 'static> Ord for Log<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.1.partial_cmp(&other.1).unwrap()
    }
}

#[derive(Copy, Clone)]
pub struct MetaData {
    pub type_id: TypeId,
    size: usize,
    align: usize,
    layout: Layout,
    dropfn: unsafe fn(*mut u8),
}

unsafe fn drop_value<T>(ptr: *mut u8) {
    drop_in_place(ptr as *mut T)
}

pub struct Lumi {
    arena: Vec<(*mut u8, u64)>,
    slots: usize,
    current: usize,
    time: u64,
    pub state: *mut u8,
    pub metadata: MetaData,
    pub history: Vec<(*mut u8, u64)>,
}

impl Lumi {
    pub fn initialize<T: 'static>(slots: usize) -> Self {
        let current = 0;
        let size = size_of::<T>();
        let align = align_of::<T>();
        let layout = Layout::from_size_align(size, align).unwrap();
        let type_id = TypeId::of::<T>();
        let arena = vec![(unsafe { alloc(layout) }, 0); slots];

        let metadata = MetaData {
            type_id,
            size,
            align,
            layout,
            dropfn: drop_value::<T>,
        };

        let state = unsafe { alloc(layout) };
        let history = Vec::new();
        Lumi {
            arena,
            state,
            slots,
            time: 0,
            current,
            metadata,
            history,
        }
    }

    pub fn write<T: 'static>(&mut self, state: T, time: u64) {
        let size = size_of_val(&state);
        let align = align_of_val(&state);
        let slot = self.current;
        let aligned = (self.arena[slot].0 as usize + align - 1) & !(align - 1);
        let is = aligned.checked_add(size).map_or(false, |end| {
            end <= (self.arena[slot].0 as usize + self.metadata.size)
        });
        unsafe {
            let ptr = if is == false {
                alloc(self.metadata.layout) as *mut T
            } else {
                aligned as *mut T
            };
            ptr.write(state);
            self.arena[slot].0 = ptr as *mut u8;
        }
        self.arena[slot].1 = time;
        if self.current == self.slots - 1 {
            self.flush()
        };
        self.current = (self.current + 1) % self.metadata.size;
    }

    fn flush(&mut self) {
        for i in &mut self.arena {
            let newalloc = unsafe { alloc(self.metadata.layout) };
            unsafe {
                ptr::swap(newalloc, i.0);
            }
            self.history.push((newalloc, i.1));
            i.1 = 0;
        }
    }
  
    #[cfg(feature = "timewarp")]
    pub fn rollback(&mut self, time: u64) -> Result<(), SimError> {
        if time >= self.time {
            return Err(SimError::RollbackTimeMismatch);
        }
        let arena_maybe = self.arena.iter().rposition(|&(_, x)| x == time);
        if arena_maybe.is_some() {
            let idx = arena_maybe.unwrap();
            unsafe { ptr::swap(self.state, self.arena[idx].0) };
            for i in idx..self.current {
                let ptr = self.arena[i].0;
                unsafe {
                    ((self.metadata.dropfn)(ptr));
                }
            }
            return Ok(());
        }
        let last_idx = self.history.iter().rposition(|&(_, t)| t == time).unwrap();
        for i in (last_idx + 1)..self.history.len() {
            let (ptr, _) = self.history[i];
            unsafe {
                (self.metadata.dropfn)(ptr);
                dealloc(ptr, self.metadata.layout);
            };
        }
        Ok(())
    }

    pub fn fetch_state<T: 'static>(&self) -> T {
        assert_eq!(self.metadata.type_id, TypeId::of::<T>());
        unsafe { (self.state as *mut T).read() }
    }

    pub fn update<T: 'static>(&mut self, new: T, time: u64) {
        assert_eq!(self.metadata.type_id, TypeId::of::<T>());
        unsafe {
            let current = ptr::replace(self.state as *mut T, new);
            if mem::size_of_val::<T>(&current) == 0 {
                return;
            }

            self.write::<T>(current, self.time);
            self.time = time
        }
    }

    pub fn wrap_up<T: 'static>(&mut self) {
        if self.current != 0 {
            for i in 0..self.current {
                let newalloc = unsafe { alloc(self.metadata.layout) };
                unsafe {
                    ptr::swap(newalloc, self.arena[i].0);
                }
                self.history.push((newalloc, self.arena[i].1));
                self.arena[i].1 = 0;
            }
        }
        let newalloc = unsafe { alloc(self.metadata.layout) };
        unsafe {
            ptr::swap(newalloc, self.state);
            dealloc(self.state, self.metadata.layout);
            for i in &mut self.arena {
                dealloc(
                    i.0,
                    Layout::from_size_align(self.metadata.size, self.metadata.align).unwrap(),
                );
            }
        }
        self.history.push((newalloc, self.time));
    }
}

pub struct Katko {
    pub agents: Vec<Lumi>,
    pub global: Option<Lumi>,
    pub events: Lumi,
}

impl Katko {
    pub fn init<T: 'static>(global: bool, slots: usize) -> Self {
        let global = if global == true {
            Some(Lumi::initialize::<T>(slots))
        } else {
            None
        };
        Katko {
            agents: Vec::new(),
            global,
            events: Lumi::initialize::<Event>(slots),
        }
    }

    pub fn add_agent<T: 'static>(&mut self, slots: usize) {
        self.agents.push(Lumi::initialize::<T>(slots));
    }

    pub fn write_event(&mut self, event: Event) {
        let time = event.time;
        self.events.write(event, time);
    }

    pub fn write_global<T: 'static>(&mut self, state: T, time: u64) {
        if self.global.is_some() {
            assert_eq!(
                self.global.as_ref().unwrap().metadata.type_id,
                TypeId::of::<T>()
            );
            self.global.as_mut().unwrap().update(state, time);
        }
    }
}
