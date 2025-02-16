use std::{
    alloc::{alloc, dealloc, Layout},
    any::{Any, TypeId},
    ffi::c_void,
    mem,
    ptr::{null_mut, NonNull},
};

use smallvec::{Array, SmallVec};

use crate::worlds::Event;

/// A logger for recording snapshots of the world.
pub struct Logger {
    pub astates: Vec<History>,
    pub gstates: History,
    events: Vec<Event>,
}

pub struct History(pub Vec<(*mut c_void, u64)>);

pub struct States<T>(pub Vec<T>);

pub fn update<T>(
    history: &mut History,
    statelogs: &mut States<T>,
    old: *mut c_void,
    new: T,
    step: &u64,
) {
    let mut old = unsafe { std::mem::replace(&mut *(old as *mut T), new) };
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

// New attempt: As little runtime allocations as possible.
use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::ptr::drop_in_place;

#[repr(align(16))]
pub struct AlignedBuffer<const N: usize>(pub [u8; N]);

enum Storage<const N: usize> {
    Stack(AlignedBuffer<N>),
    Heap {
        ptr: NonNull<u8>,
        layout: Layout,
        used: usize,
    },
}

pub struct Snowflake<const N: usize> {
    type_id: TypeId,
    storage: UnsafeCell<Storage<N>>,
}

impl<const N: usize> Snowflake<N> {
    pub fn new() -> Self {
        Self {
            type_id: TypeId::of::<()>(),
            storage: UnsafeCell::new(Storage::Stack(AlignedBuffer([0; N]))),
        }
    }

    pub fn store<T: 'static>(&mut self, value: T) {
        let size = mem::size_of::<T>();
        let align = mem::align_of::<T>();
        unsafe {
            let storage = &mut *self.storage.get();
            let can_reuse = match storage {
                Storage::Stack(buf) => size <= N && align <= mem::align_of_val(buf),
                Storage::Heap { layout, used, .. } => {
                    size <= layout.size() && align <= layout.align() && size <= *used
                }
            };
            if !can_reuse {
                self.clear();
                if size <= N && align <= mem::align_of::<AlignedBuffer<N>>() {
                    *storage = Storage::Stack(AlignedBuffer([0; N]));
                } else {
                    let layout = Layout::from_size_align(size, align).expect("Invalid layout");
                    let ptr = NonNull::new(alloc(layout)).expect("Allocation failed");
                    *storage = Storage::Heap {
                        ptr,
                        layout,
                        used: size,
                    };
                }
            }
            let dest_ptr = match storage {
                Storage::Stack(buf) => buf.0.as_mut_ptr() as *mut T,
                Storage::Heap { ptr, .. } => ptr.as_ptr() as *mut T,
            };
            dest_ptr.write(value);
            self.type_id = TypeId::of::<T>();
        }
    }

    pub unsafe fn clear(&mut self) {
        let storage = &mut *self.storage.get();
        match storage {
            Storage::Stack(_) => {} // No cleanup needed for stack
            Storage::Heap { ptr, layout, .. } => {
                drop_in_place(ptr.as_ptr() as *mut MaybeUninit<u8>);
                dealloc(ptr.as_ptr(), *layout);
            }
        }
    }
}

pub struct Snowball<const N: usize, const M: usize> {
    buffers: [UnsafeCell<Snowflake<N>>; M],
    index: usize,
}

impl<const N: usize, const M: usize> Snowball<N, M> {
    pub fn new() -> Self {
        Self {
            buffers: std::array::from_fn(|_| UnsafeCell::new(Snowflake::new())),
            index: 0,
        }
    }

    pub fn log<T: 'static>(&mut self, value: T) {
        let idx = self.index;
        self.index = (idx + 1) % M;
        unsafe {
            let flake = &mut *self.buffers[idx].get();
            flake.store(value);
        }
    }
}
