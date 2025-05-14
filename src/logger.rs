use crate::worlds::{Event, SimError};
use std::{
    alloc::{alloc, dealloc, Layout},
    any::TypeId,
    mem,
    ptr::{self, drop_in_place},
};

#[derive(Copy, Clone)]
/// Metadata for writing and reconstructing an arbitrary type to/from raw bytes
pub struct MetaData {
    pub type_id: TypeId,
    size: usize,
    align: usize,
    layout: Layout,
    dropfn: fn(*mut u8),
}

// safe wrapper for the `drop_in_place` function for clearing a value.
fn drop_value<T>(ptr: *mut u8) {
    unsafe { drop_in_place(ptr as *mut T) }
}

/// `Lumi` is a type erased, zero-copy, arena-based batch logger.
// Each instance of the struct is meant only for 1 type.
// This can be agent states, actions or global states.
// The intention of this design is to minimize runtime allocations unless entirely necessary, while also allowing type flexibility without the need to propagate up any generics
pub struct Lumi {
    arena: Vec<(*mut u8, u64)>, // preallocated arena using a vector of fixed size
    slots: usize,               // number of total slots
    current: usize,             // current slot in the arena
    time: u64,                  // current simulation time
    pub state: *mut u8,         // The current state of the logged variable.
    pub metadata: MetaData,     // Type metadata.
    pub history: Vec<(*mut u8, u64)>, // Logs
}

impl Lumi {
    /// Allocate memory arena, and initialize the logger
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
    /// Write directly to logger arena without updating current state.
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
    /// flush arena slots into a heap allocation via a push to vec
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
    /// Rollback the logger by finding the log of a past timestep.
    // !!need to fix this! the case of infrequent updates means this search will fail if any rollback time falls between logs. need to take the floor!!
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

    /// Fetch current state
    pub fn fetch_state<T: 'static>(&self) -> T {
        assert_eq!(self.metadata.type_id, TypeId::of::<T>());
        unsafe { (self.state as *mut T).read() }
    }
    /// safe update function for mutating the agent or global state and zero copy logging.
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
    /// deallocation the arena and push the rest of to historical logs.
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

unsafe impl Send for Lumi {}
unsafe impl Sync for Lumi {}

/// `World`-specific container for all the necessary loggers (global state, events, and agent states).
pub struct Katko {
    pub agents: Vec<Lumi>,
    pub global: Option<Lumi>,
    pub events: Lumi,
}

impl Katko {
    /// initialize state container for `World`
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
    /// Add an `Agent` with a given state type `T` to the container
    pub fn add_agent<T: 'static>(&mut self, slots: usize) {
        self.agents.push(Lumi::initialize::<T>(slots));
    }
    /// Write an `Event` to logs
    pub fn write_event(&mut self, event: Event) {
        let time = event.time;
        self.events.write(event, time);
    }
    /// update the global state
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


#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::Layout;

    // Helper to write an initial T into the raw state pointer
    unsafe fn seed_state<T: 'static + Copy>(lumi: &mut Lumi, val: T) {
        let p = lumi.state as *mut T;
        p.write(val);
        // also update metadata so size_of_val on drop checks out
        assert_eq!(lumi.metadata.type_id, TypeId::of::<T>());
    }

    #[test]
    fn test_update_and_fetch_state() {
        let mut lumi = Lumi::initialize::<u32>(2);
        // Seed an initial state of 7
        unsafe { seed_state(&mut lumi, 7u32) };
        assert_eq!(lumi.fetch_state::<u32>(), 7);

        // Update to 42 at t=1
        lumi.update(42u32, 1);
        assert_eq!(lumi.fetch_state::<u32>(), 42);

        // Update to 100 at t=2
        lumi.update(100u32, 2);
        assert_eq!(lumi.fetch_state::<u32>(), 100);
    }

    #[test]
    fn test_flush_triggers_history_growth() {
        let mut lumi = Lumi::initialize::<u32>(2);
        // Seed initial state so the first update writes something sensible
        unsafe { seed_state(&mut lumi, 10u32) };

        // First update goes into arena, no flush yet
        lumi.update(20u32, 5);
        assert_eq!(lumi.history.len(), 0, "haven't hit a full slot cycle yet");

        // Second update: current==slots-1 (1), so write() will call flush()
        lumi.update(30u32, 6);
        // flush pushes both arena entries into history
        assert_eq!(
            lumi.history.len(),
            2,
            "after two updates on a 2-slot arena, flush should have dumped both entries"
        );
    }

    // Only run this if you built with `--features timewarp`
    #[cfg(feature = "timewarp")]
    #[test]
    fn test_rollback_restores_previous_state() {
        let mut lumi = Lumi::initialize::<u8>(5);
        // seed a String so that rollback has something valid to swap
        unsafe { seed_state(&mut lumi, 0u8) };

        lumi.update(1u8, 1);
        lumi.update(2u8, 2);
        lumi.update(3u8, 3);

        // roll back to time=2, should go back to "second"
        lumi.rollback(2).expect("rollback should succeed");
        assert_eq!(lumi.fetch_state::<u8>(), 2u8);

        // roll back to time=1, now "first"
        lumi.rollback(1).expect("rollback should succeed");
        assert_eq!(lumi.fetch_state::<u8>(), 1u8);
    }
}
