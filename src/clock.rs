use crate::worlds::SimError;
use std::{any::TypeId, cmp::Reverse, collections::BTreeSet};

/// Trait for any time-series object for processing.
pub trait Scheduleable {
    fn time(&self) -> u64;
    fn commit_time(&self) -> u64;
}

/// Simulation time, simulation step, timestep size, terminal time, and time scale
pub struct Time {
    pub time: f64,
    pub step: u64,
    pub timestep: f64,
    pub timescale: f64, // 1.0 = real-time, 0.5 = half-time, 2.0 = double-time
    pub terminal: Option<f64>,
}

/// Hierarchical Timing Wheel (HTW) for scheduling events and messages.
pub struct Clock<T: Scheduleable + Ord + 'static, const SLOTS: usize, const HEIGHT: usize> {
    pub wheels: [[Vec<T>; SLOTS]; HEIGHT], //timing wheels, with sizes {SLOTS, SLOTS^2, ..., SLOTS^HEIGHT}
    pub current_idxs: [usize; HEIGHT],     // wheel parsing offset index
    pub time: Time,
}

impl<T: Scheduleable + Ord, const SLOTS: usize, const HEIGHT: usize> Clock<T, SLOTS, HEIGHT> {
    /// New HTW from time-step size and terminal time.
    pub fn new(timestep: f64, terminal: Option<f64>) -> Result<Self, SimError> {
        if HEIGHT < 1 {
            return Err(SimError::NoClock);
        }
        let wheels = std::array::from_fn(|_| std::array::from_fn(|_| Vec::new()));
        let current = [0 as usize; HEIGHT];
        Ok(Clock {
            wheels,
            time: Time {
                time: 0.0,
                step: 0,
                timestep: timestep,
                timescale: 1.0,
                terminal: terminal,
            },
            current_idxs: current,
        })
    }
    /// Find corresponding slot based on `Scheduleable::time()' output, and insert.
    pub fn insert(&mut self, event: T) -> Result<(), T> {
        let time = event.time();
        let deltaidx = (time - self.time.step) as usize;

        for k in 0..HEIGHT {
            let startidx = ((SLOTS).pow(1 + k as u32) - SLOTS) / (SLOTS - 1); // start index for each level
            let endidx = ((SLOTS).pow(2 + k as u32) - SLOTS) / (SLOTS - 1) - 1; // end index for each level
            if deltaidx >= startidx {
                if deltaidx >= (((SLOTS).pow(1 + HEIGHT as u32) - SLOTS) / (SLOTS - 1)) as usize {
                    return Err(event);
                }
                if deltaidx > endidx {
                    continue;
                }
                let offset =
                    ((deltaidx - startidx) / (SLOTS.pow(k as u32)) + self.current_idxs[k]) % SLOTS; // slot based on the current offset index for level k.
                self.wheels[k][offset as usize].push(event);
                return Ok(());
            }
        }
        Err(event)
    }
    /// consume the next step's pending events.
    pub fn tick(&mut self) -> Result<Vec<T>, SimError> {
        let row: &mut [Vec<T>] = &mut self.wheels[0];
        let events = std::mem::replace(&mut row[self.current_idxs[0]], Vec::new());
        if !events.is_empty() && events[0].time() < self.time.step {
            println!("Time travel detected");
            return Err(SimError::TimeTravel);
        }
        if events.is_empty() {
            return Err(SimError::NoEvents);
        }
        Ok(events)
    }
    /// roll clock forward one tick, and rotate higher wheels if necessary.
    pub fn increment(&mut self, overflow: &mut BTreeSet<Reverse<T>>) {
        self.current_idxs[0] = (self.current_idxs[0] + 1) % SLOTS;
        self.time.time += self.time.timestep;
        self.time.step += 1;
        if self.current_idxs[0] as u64 == 0 {
            self.rotate(overflow);
        }
    }
    /// Rotate the timing wheel, moving events from the k-th wheel to fill the (k-1)-th wheel.
    pub fn rotate(&mut self, overflow: &mut BTreeSet<Reverse<T>>) {
        for k in 1..HEIGHT {
            let wheel_period = SLOTS.pow(k as u32);
            if self.time.step % (wheel_period as u64) == 0 {
                if HEIGHT == k {
                    for _ in 0..SLOTS.pow(HEIGHT as u32 - 1) {
                        overflow.pop_first().map(|event| self.insert(event.0));
                    }
                    return;
                }
                let row = &mut self.wheels[k];
                let higher_events = std::mem::replace(&mut row[self.current_idxs[k]], Vec::new());
                self.current_idxs[k] = (self.current_idxs[k] + 1) % SLOTS;
                for event in higher_events {
                    let _ = self.insert(event).map_err(|event| {
                        overflow.insert(Reverse(event));
                    });
                }
            }
        }
    }

    #[cfg(feature = "timewarp")]
    /// Rollback the wheel
    pub fn rollback(
        &mut self,
        time: u64,
        overflow: &mut BTreeSet<Reverse<T>>,
    ) -> Result<(), SimError> {
        self.current_idxs.iter_mut().for_each(|x| *x = 0);
        self.time.step = time;
        self.time.time = time as f64 * self.time.timestep;

        let mut resubmit = Vec::new();
        self.wheels.iter_mut().for_each(|x| {
            x.iter_mut().for_each(|x| {
                if x.len() > 0 {
                    check_process_object_list(time, x);
                    for i in 0..x.len() {
                        let g = x.remove(i);
                        resubmit.push(g);
                    }
                }
            })
        });
        for i in 0..resubmit.len() {
            let result = self.insert(resubmit.remove(i));
            if result.is_err() {
                if overflow.insert(Reverse(result.err().unwrap())) == false {
                    return Err(SimError::ClockSubmissionFailed);
                }
            }
        }
        Ok(())
    }
}

#[cfg(feature = "timewarp")]
use crate::timewarp::lp::Object;
#[cfg(feature = "timewarp")]
/// remove local events scheduled after the rollback time.
fn check_process_object_list<T: Scheduleable + 'static>(time: u64, object_list: &mut Vec<T>) {
    for i in 0..object_list.len() {
        if object_list[i].commit_time() >= time && TypeId::of::<T>() == TypeId::of::<Object>() {
            let obj: &Object = unsafe { &*(&object_list[i] as *const T as *const Object) };
            match obj {
                Object::Event(_) => {
                    object_list.remove(i);
                }
                Object::Message(_) => {}
            }
        }
    }
}
