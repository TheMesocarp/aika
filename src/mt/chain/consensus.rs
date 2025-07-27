use bytemuck::{Pod, Zeroable};
use mesocarp::{comms::mailbox::ThreadedMessenger, sync::gvt::aika::Consensus, MesoError};

use crate::{mt::chain::Time, objects::Mail, AikaError};


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

    pub fn with_block_duration(&mut self, duration: u64) {
        self.max_block_dur = duration;
    }
}

unsafe impl<const BLOCK_BW: usize, const MSG_BW: usize, MessageType: Pod + Zeroable + Clone> Send for Galaxy<BLOCK_BW, MSG_BW, MessageType> {}
unsafe impl<const BLOCK_BW: usize, const MSG_BW: usize, MessageType: Pod + Zeroable + Clone> Sync for Galaxy<BLOCK_BW, MSG_BW, MessageType> {}