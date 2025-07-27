use bytemuck::{Pod, Zeroable};
use mesocarp::{comms::mailbox::ThreadedMessenger, sync::gvt::aika::Consensus, MesoError};

use crate::{mt::chain::{producer::Planet, Time}, objects::Mail, AikaError};


pub struct Galaxy<const BLOCK_BW: usize, const MSG_BW: usize, MessageType: Pod + Zeroable + Clone> {
    pub consensus: Consensus<BLOCK_BW>,
    pub(crate) interplanetary_messenger: ThreadedMessenger<MSG_BW, Mail<MessageType>>,
    pub time: Time,
    pub max_block_dur: u64,
    pub registered: usize,
    pub planet_count: usize,
}

impl<const BLOCK_BW: usize, const MSG_BW: usize, MessageType: Pod + Zeroable + Clone> Galaxy<BLOCK_BW, MSG_BW, MessageType> {
    pub fn new(planet_count: usize, block_batch_size: usize) -> Result<Self, AikaError> {
        let mut planet_ids = Vec::new();
        for i in 0..planet_count {
            planet_ids.push(i);
        }
        let messenger = ThreadedMessenger::new(planet_ids)?;

        Ok(Self {
            consensus: Consensus::new(mesocarp::sync::ComputeLayout::HubSpoke, block_batch_size)?,
            interplanetary_messenger: messenger,
            time: Time { gvt: 0, cp_hz: u64::MAX, throttle: u64::MAX, terminal: f64::MAX, timestep: 1.0 },
            max_block_dur: 64,
            registered: 0,
            planet_count
        })
    }

    pub fn set_time_scale(&mut self, timestep: f64, terminal: f64) {
        self.time.terminal = terminal;
        self.time.timestep = timestep
    }

    pub fn throttle(&mut self, throttle: u64) {
        self.time.throttle = throttle
    }

    pub fn checkpoints(&mut self, frequency: u64) {
        self.time.cp_hz = frequency
    }

    pub fn with_block_duration(&mut self, duration: u64) {
        self.max_block_dur = duration;
    }

    pub fn spawn_planet<const CLOCK_BW: usize, const CLOCK_SCALES: usize>(&mut self) -> Result<Planet<BLOCK_BW, MSG_BW, CLOCK_BW, CLOCK_SCALES, MessageType>, AikaError> {
        if self.registered == self.planet_count {
            return Err(AikaError::MaximumAgentsAllowed);
        }
        let id = self.registered;
        self.registered += 1;
        let messenger_account = self.interplanetary_messenger.get_user(id)?;
        let spoke = self.consensus.register_producer(None)?.unwrap();
        Planet::from_galaxy_registration(self.time, spoke, messenger_account, id)
    }

    fn deliver_the_mail(&mut self) -> Result<(), AikaError> {
        match self.interplanetary_messenger.poll() {
            Ok(msgs) => {
                self.interplanetary_messenger.deliver(msgs)?;
                Ok(())
            }
            Err(err) => {
                if let MesoError::NoDirectCommsToShare = err {
                    Ok(())
                } else {
                    Err(AikaError::MesoError(err))
                }
            }
        }
    }

    fn check_all_terminal(&mut self) -> Result<bool, AikaError> {
        if self.time.gvt as f64 * self.time.timestep >= self.time.terminal {
            return Ok(true);
        }
        let latest = self.consensus.fetch_latest_uncommited_blocks()?;
        let mut truth = true;
        for block in latest {
            if let Some(block) = block {
                truth = ((block.start + block.dur) as f64 * self.time.timestep) >= self.time.terminal;
                continue;
            }
            return Ok(false);
        }
        Ok(truth)
    }

    pub fn master(&mut self) -> Result<(), AikaError> {
        loop {
            // mail
            //println!("GVT Master, GVT {:?}: delivering mail...", self.gvt);
            for _ in 0..10 {
                self.deliver_the_mail()?;
                self.consensus.poll_n_slot()?;
                while let Some(new_gvt) = self.consensus.check_update_safe_point()? {
                    self.consensus.processor.broadcast_new_safe_point(new_gvt);
                }
            }
            //println!("GVT Master, GVT {:?}: polling blocks, updating time consensus...", self.gvt);
            if self.check_all_terminal()? {
                //println!("GVT Master, GVT {:?}: all planets are waiting", self.gvt);
                if self.consensus.check_status() {
                    //println!("GVT Master, GVT {:?}: GVT has caught up, consensus reached!", self.gvt);
                    break;
                }
            }
        }
        Ok(())
    }
}

unsafe impl<const BLOCK_BW: usize, const MSG_BW: usize, MessageType: Pod + Zeroable + Clone> Send for Galaxy<BLOCK_BW, MSG_BW, MessageType> {}
unsafe impl<const BLOCK_BW: usize, const MSG_BW: usize, MessageType: Pod + Zeroable + Clone> Sync for Galaxy<BLOCK_BW, MSG_BW, MessageType> {}