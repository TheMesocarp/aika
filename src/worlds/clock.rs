use anyhow::Result;
use std::cmp::Reverse;
use std::collections::BTreeSet;

use super::{Event, SimError};

/// The relevant time information for the simulation.
pub struct Time {
    pub time: f64,
    pub step: usize,
    pub timestep: f64,
    pub timescale: f64, // 1.0 = real-time, 0.5 = half-time, 2.0 = double-time
    pub terminal: Option<f64>,
}

/// A hierarchical timing wheel for scheduling events in a simulation.
pub struct Clock<const SLOTS: usize, const HEIGHT: usize> {
    wheels: [[Vec<Event>; SLOTS]; HEIGHT],
    current_idxs: [usize; HEIGHT],
    pub time: Time,
}

impl<const SLOTS: usize, const HEIGHT: usize> Clock<SLOTS, HEIGHT> {
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

    /// Insert an event into the timing wheel.
    /// checks how many timesteps into the future the event is and selects the appropriate wheel before finding a slot. If too far into the future, the event is returned.
    pub fn insert(&mut self, event: Event) -> Result<(), Event> {
        let time = event.time();
        let delta = time - self.time.time - self.time.timestep;

        for k in 0..HEIGHT {
            let startidx = ((SLOTS).pow(1 + k as u32) - SLOTS) / (SLOTS - 1);
            let futurestep = (delta / self.time.timestep) as usize;
            if futurestep >= startidx {
                if futurestep >= ((SLOTS).pow(1 + HEIGHT as u32) - SLOTS) / (SLOTS - 1) {
                    return Err(event);
                }
                let offset = (futurestep - startidx) / SLOTS.pow(k as u32) + self.current_idxs[k];
                self.wheels[k][offset].push(event);
                return Ok(());
            }
        }
        Err(event)
    }

    /// Pop the next timestep's events from the timing wheel and roll the wheel forward.
    pub fn tick(
        &mut self,
        overflow: &mut BTreeSet<Reverse<Event>>,
    ) -> Result<Vec<Event>, SimError> {
        let row: &mut [Vec<Event>] = &mut self.wheels[0];
        let events = std::mem::replace(&mut row[self.current_idxs[0]], Vec::new());
        self.current_idxs[0] = (self.current_idxs[0] + 1) % SLOTS;
        if !events.is_empty() && events[0].time() < self.time.time {
            return Err(SimError::TimeTravel);
        }
        self.time.time += self.time.timestep;
        self.time.step += 1;
        if (self.time.time / self.time.timestep) as u64 % SLOTS as u64 == 0 {
            self.rotate(overflow);
        }
        if events.is_empty() {
            return Err(SimError::NoEvents);
        }
        Ok(events)
    }

    /// Rotate the timing wheel, moving events from the k-th wheel to fill the (k-1)-th wheel.
    pub fn rotate(&mut self, overflow: &mut BTreeSet<Reverse<Event>>) {
        let current_step = self.time.step as u64 + 1;

        for k in 1..HEIGHT {
            let wheel_period = SLOTS.pow(k as u32);
            if current_step % (wheel_period as u64) == 0 {
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
}
