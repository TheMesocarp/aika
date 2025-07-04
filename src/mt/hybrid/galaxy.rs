use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use bytemuck::{Pod, Zeroable};
use mesocarp::{comms::mailbox::ThreadedMessenger, scheduling::Scheduleable, MesoError};

use crate::{messages::Mail, mt::hybrid::planet::RegistryOutput, st::TimeInfo, SimError};

pub struct Galaxy<
    const INTER_SLOTS: usize,
    const CLOCK_SLOTS: usize,
    const CLOCK_HEIGHT: usize,
    MessageType: Pod + Zeroable + Clone,
> {
    pub messenger: ThreadedMessenger<INTER_SLOTS, Mail<MessageType>>,
    pub lvts: Vec<Arc<AtomicU64>>,
    pub gvt: Arc<AtomicU64>,
    pub next_checkpoint: Arc<AtomicU64>,
    pub checkpoint_frequency: u64,
    pub throttle_horizon: u64,
    pub time_info: TimeInfo,
    pub registered: usize,
}

impl<
        const INTER_SLOTS: usize,
        const CLOCK_SLOTS: usize,
        const CLOCK_HEIGHT: usize,
        MessageType: Pod + Zeroable + Clone,
    > Galaxy<INTER_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
    pub fn new(
        num_world: usize,
        throttle_horizon: u64,
        checkpoint_frequency: u64,
        terminal: f64,
        timestep: f64,
    ) -> Result<Self, SimError> {
        let gvt = Arc::new(AtomicU64::new(0));
        let mut world_ids = Vec::new();
        for i in 0..num_world {
            world_ids.push(i);
        }
        let messenger = ThreadedMessenger::new(world_ids)?;
        Ok(Self {
            messenger,
            lvts: Vec::new(),
            gvt,
            next_checkpoint: Arc::new(AtomicU64::new(checkpoint_frequency)),
            checkpoint_frequency,
            throttle_horizon,
            time_info: TimeInfo { timestep, terminal },
            registered: 0,
        })
    }

    pub fn spawn_world(&mut self) -> Result<RegistryOutput<INTER_SLOTS, MessageType>, SimError> {
        let arc = Arc::clone(&self.gvt);

        let lvt = Arc::new(AtomicU64::new(0));
        let out = Arc::clone(&lvt);

        self.lvts.push(lvt);

        let user = self.messenger.get_user(self.registered)?;
        let world_id = self.registered;
        self.registered += 1;
        let output =
            RegistryOutput::new(arc, out, Arc::clone(&self.next_checkpoint), user, world_id);
        Ok(output)
    }

    fn deliver_the_mail(&mut self) -> Result<u64, SimError> {
        match self.messenger.poll() {
            Ok(msgs) => {
                let mut lowest = u64::MAX;
                for (_, mail) in &msgs {
                    let time = mail.transfer.commit_time();
                    if time < lowest {
                        lowest = time;
                    }
                }
                self.messenger.deliver(msgs)?;
                Ok(lowest)
            },
            Err(err) => if let MesoError::NoDirectCommsToShare = err {
                Ok(u64::MAX)
            } else {
                return Err(SimError::MesoError(err))
            },
        }
    }

    fn recalc_gvt(&mut self, in_transit_floor: u64) -> Result<(), SimError> {
        // Samadi's is nice but i need something compatible with checkpointing
        let new_time = self.gvt.load(Ordering::Acquire);
        //println!("current gvt: {new_time}");
        let mut lowest = u64::MAX;
        for local in &self.lvts {
            let load = local.load(Ordering::Acquire);
            if load < lowest {
                lowest = load;
            }
        }
        if in_transit_floor < lowest {
            lowest = in_transit_floor;
        }
        if new_time > lowest {
            return Err(SimError::TimeTravel);
        }
        //println!("new_gvt: {lowest}");
        self.gvt.store(lowest, Ordering::Release);
        Ok(())
    }

    fn check_mail_and_gvt(&mut self) -> Result<(), SimError> {
        let transit_time = self.deliver_the_mail()?;
        self.recalc_gvt(transit_time)?;
        Ok(())
    }

    pub fn gvt_daemon(&mut self) -> Result<(), SimError> {
        loop {
            self.check_mail_and_gvt()?;

            let current_gvt = self.gvt.load(Ordering::Acquire);

            // Check if all LPs have reached terminal
            let all_terminal = self.lvts.iter().all(|lvt| {
                let lvt_val = lvt.load(Ordering::Acquire);
                lvt_val as f64 * self.time_info.timestep >= self.time_info.terminal
                // assuming you store this somewhere
            });

            if all_terminal {
                println!("All LPs reached terminal time, shutting down");
                break;
            }

            // Handle checkpointing
            if current_gvt >= self.next_checkpoint.load(Ordering::Acquire) {
                self.next_checkpoint
                    .store(current_gvt + self.checkpoint_frequency, Ordering::Release);
            }
            std::thread::yield_now();
        }
        println!("ended galaxy thread");
        Ok(())
    }
}
