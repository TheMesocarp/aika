// GVT/Coordinator Thread,
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread,
};

use bytemuck::Pod;
use mesocarp::concurrency::spsc::BufferWheel;

use crate::worlds::SimError;

use super::{
    comms::{Comms, Transferable},
    lp::{Object, LP},
    paragent::LogicalProcess,
};

pub struct GVT<const LPS: usize, const SIZE: usize, const SLOTS: usize, const HEIGHT: usize> {
    global_time: usize,
    terminal: usize,
    local_times: [Option<Arc<AtomicUsize>>; LPS],
    pub comms: Option<Comms<LPS, SIZE>>,
    host: Vec<Vec<[Option<Transferable>; SIZE]>>,
    temp_load: Vec<(Arc<BufferWheel<SIZE, Transferable>>, Arc<BufferWheel<SIZE, Transferable>>)>,
    lps: [Option<LP<SLOTS, HEIGHT, SIZE>>; LPS],
    message_overflow: [Vec<Transferable>; LPS],
}

impl<const LPS: usize, const SIZE: usize, const SLOTS: usize, const HEIGHT: usize>
    GVT<LPS, SIZE, SLOTS, HEIGHT>
{
    ///Start the time warp engine
    pub fn start_engine(terminal: usize) -> Box<Self> {
        let lps = [const { None }; LPS];
        let message_overflow: [Vec<Transferable>; LPS] = std::array::from_fn(|_| Vec::new());
        let local_times = [const { None }; LPS];
        let comms = None;
        let host: Vec<Vec<[Option<Transferable>; SIZE]>> = (0..2)
            .map(|_| (0..LPS).map(|_| [const { None }; SIZE]).collect())
            .collect();
        Box::new(GVT {
            global_time: 0,
            local_times,
            terminal,
            comms,
            host,
            temp_load: Vec::new(),
            lps,
            message_overflow,
        })
    }
    /// Spawn a `LP` in the simulator.
    pub fn spawn_process<T: Pod + 'static>(
        &mut self,
        process: Box<dyn LogicalProcess>,
        timestep: f64,
        log_slots: usize,
    ) -> Result<usize, SimError> {
        let ptr_idx = self.lps.iter().rposition(|x| x.is_none());
        if ptr_idx.is_none() {
            return Err(SimError::LPsFull);
        }

        let circ1 = Arc::new(BufferWheel::new());
        let circ2 = Arc::new(BufferWheel::new());
        let step = Arc::new(AtomicUsize::from(0));
        self.local_times[ptr_idx.unwrap()] = Some(Arc::clone(&step));
        let lp_comms = [
            circ1.clone(),
            circ2.clone()
        ];
        let lp = LP::<SLOTS, HEIGHT, SIZE>::new::<T>(
            ptr_idx.unwrap(),
            process,
            timestep,
            step,
            lp_comms,
            log_slots,
        );
        self.lps[ptr_idx.unwrap()] = Some(lp);
        self.temp_load.push((circ1, circ2));
        Ok(ptr_idx.unwrap())
    }

    /// Initialize the Comms struct for main thread
    pub fn init_comms(&mut self) -> Result<(), SimError> {
        let len = self.temp_load.len();
        let mut comms_buffers1 = Vec::new();
        let mut comms_buffers2 = Vec::new();
        for i in 0..len {
            let pair = self.temp_load.remove(0);
            comms_buffers1.push(pair.0);
            comms_buffers2.push(pair.1);
        }
        if comms_buffers1.len() < LPS || comms_buffers2.len() < LPS {
            return Err(SimError::MismatchLPsCount);
        }
        let slc1: Result<[Arc<BufferWheel<SIZE, Transferable>>; LPS], _> = comms_buffers1.try_into();
        let slc2: Result<[Arc<BufferWheel<SIZE, Transferable>>; LPS], _> = comms_buffers2.try_into();
        let comms_wheel = [slc1.unwrap(), slc2.unwrap()];
        self.comms = Some(Comms::new(comms_wheel));
        for i in 0..LPS {
            self.lps[i]
                .as_mut()
                .unwrap()
                .set_terminal(self.terminal as f64);
        }
        Ok(())
    }
    /// Commit an object to a given LP from the main thread prior to run. Meant for initialization
    pub fn commit(&mut self, id: usize, object: Object) -> Result<(), SimError> {
        if id >= self.lps.len() {
            return Err(SimError::InvalidIndex);
        }
        self.lps[id].as_mut().unwrap().commit(object);
        Ok(())
    }
    /// Current GVT step
    pub fn step_counter(&self) -> u64 {
        self.global_time as u64
    }
}

/// Main run function for the timewarp simulator
/// !!! Needs to be fixed! Comms is not updating properly and its causing a full SIZE iteration each loop which is detrimental to performance as is
pub fn run<const LPS: usize, const SIZE: usize, const SLOTS: usize, const HEIGHT: usize>(
    gvt: &'static mut GVT<LPS, SIZE, SLOTS, HEIGHT>,
) -> Result<(), SimError> {
    let mut handles = Vec::new();
    for i in 0..LPS {
        if let Some(mut lp) = gvt.lps[i].take() {
            let handle = thread::spawn(move || return lp.run());
            handles.push(handle);
        }
    }
    let main = {
        let comms = gvt.comms.as_mut().unwrap();
        let local_times = &gvt.local_times;
        let message_overflow = &mut gvt.message_overflow;
        let global_time = &mut gvt.global_time;
        let terminal = &mut gvt.terminal;
        thread::spawn(move || {
            loop {
                let mut min_time = usize::MAX;
                for time in local_times.iter().flatten() {
                    let ltime = time.load(Ordering::Relaxed);
                    if ltime < min_time {
                        min_time = ltime;
                    }
                }
                *global_time = if min_time == usize::MAX { 0 } else { min_time };
                if *global_time >= *terminal {
                    println!("break");
                    break;
                }
                for i in 0..LPS {
                    if message_overflow[i].len() > 0 {
                        let len = message_overflow[i].len();
                        for i in 0..len {
                            let val = message_overflow[i].pop().unwrap();
                            let status = comms.write(val);
                            if status.is_err() {
                                message_overflow[i].push(status.err().unwrap());
                                break;
                            }
                        }
                    }
                }
                let results = comms.poll();
                if results.is_err() {
                    return Err(SimError::PollError);
                }
                for (i, j) in results.unwrap().iter().enumerate() {
                    if j.is_some() {
                        let mut counter = 0;
                        loop {
                            if counter == SIZE {
                                break;
                            }
                            let msg = comms.read(i);
                            if msg.is_err() {
                                break;
                            }
                            let status = comms.write(msg.unwrap());
                            if status.is_err() {
                                let msg = status.err().unwrap();
                                message_overflow[msg.to()].push(msg);
                            }
                            counter += 1;
                        }
                    }
                }
            }
            return Ok(());
        })
    };
    main.join().map_err(|_| SimError::ThreadJoinError)??;
    Ok(())
}
