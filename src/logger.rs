use std::{
    alloc::{alloc, dealloc, Layout}, any::TypeId, cmp::Ordering, collections::BTreeSet, mem, ptr::{self, drop_in_place}
};

use crate::worlds::Event;

pub struct Log<T: 'static>(T, usize);

impl<T: 'static> PartialEq for Log<T>{
    fn eq(&self, other: &Self) -> bool {
        self.1 == other.1
    }
}

impl<T: 'static> Eq for Log<T>{}

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

unsafe fn drop_value<T>(_ptr: *mut u8) {
    drop_in_place(_ptr as *mut T)
}

pub struct Lumi {
    arena: Vec<(*mut u8, usize)>,
    slots: usize,
    current: usize,
    pub state: *mut u8,
    pub metadata: MetaData,
    pub history: Vec<(*mut u8, usize)>,
}

impl Lumi {
    pub fn initialize<T: 'static>(slots: usize) -> Self {
        let current = 0;
        let size = size_of::<T>();
        let align = align_of::<T>();
        let layout = Layout::from_size_align(size, align).unwrap();
        let type_id = TypeId::of::<T>();
        let arena = vec![
            (
                unsafe { alloc(layout) },
                0
            );
            slots
        ];

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
            current,
            metadata,
            history,
        }
    }

    pub fn write<T: 'static>(&mut self, state: T, time: usize) {
        let size = size_of_val(&state);
        let align = align_of_val(&state);
        let slot = self.current;
        let aligned = (self.arena[slot].0 as usize + align - 1) & !(align - 1);
        let is = aligned.checked_add(size).map_or(false, |end| {
            end <= (self.arena[slot].0 as usize + self.metadata.size)
        });

        if is == false {
            unsafe {
                let ptr_heap = alloc(self.metadata.layout) as *mut T;
                ptr_heap.write(state);
                self.history.push((ptr_heap as *mut u8, time));
            }
            return;
        }
        unsafe {
            let dest = aligned as *mut T;
            dest.write(state);
            self.arena[slot].0 = dest as *mut u8;
            self.arena[slot].1 = time;
        }
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

    pub fn reconstruct<T: 'static>(&mut self) -> BTreeSet<Log<T>> {
        self.history.iter().map(|(x, y)| {
            assert_eq!(self.metadata.type_id, TypeId::of::<T>());
            let read = unsafe { ptr::read(*x as *mut T) };
            Log(read, *y)
        }).collect::<BTreeSet<Log<T>>>()
    }

    pub fn fetch_state<T: 'static>(&self) -> T {
        assert_eq!(self.metadata.type_id, TypeId::of::<T>());
        unsafe { (self.state as *mut T).read() }
    }

    pub fn update<T: 'static>(&mut self, new: T, time: usize) {
        assert_eq!(self.metadata.type_id, TypeId::of::<T>());
        unsafe {
            let current = ptr::replace(self.state as *mut T, new);
            if mem::size_of_val::<T>(&current) == 0 {
                return;
            }
            self.write::<T>(current, time);
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

        unsafe {
            for i in &mut self.arena {
                dealloc(i.0, Layout::from_size_align(self.metadata.size, self.metadata.align).unwrap());
            }
        }
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
        let time = event.time as usize;
        self.events.write(event, time);
    }

    pub fn write_global<T: 'static>(&mut self, state: T, time: usize) {
        if self.global.is_some() {
            assert_eq!(self.global.as_ref().unwrap().metadata.type_id, TypeId::of::<T>());
            self.global.as_mut().unwrap().update(state, time);
        }
    }
}
