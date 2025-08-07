use std::{fs::File, io::Write, time::Instant};

use bytemuck::{Pod, Zeroable};
use mesocarp::{comms::mailbox::ThreadedMessenger, MesoError};

use crate::{env::Environment, mt::{consensus::{Block, ComputeLayout, Consensus}, engines::{hlocal::Planet, HTime}}, objects::Mail, AikaError};


#[derive(Debug)]
/// A `Galaxy` is an inter-cluster message bus and GVT updater, intended to own its own thread.
pub struct Galaxy<const BLOCK_BW: usize, const MSG_BW: usize, MessageType: Pod + Zeroable + Clone> {
    /// The temporal consensus routine for an updted GVT computation.
    pub consensus: Consensus<BLOCK_BW>,
    pub(crate) interplanetary_messenger: ThreadedMessenger<MSG_BW, Mail<MessageType>>,
    /// Current time information.
    pub time: HTime,
    /// Maximum duration of a block. Also the expected length of a block unless the simulation terminates early.
    pub max_block_dur: u64,
    /// Current count of registered clusters.
    pub registered: usize,
    /// Maximum number of allowed clusters.
    pub planet_count: usize,
    /// The GVT thread is waiting for in flight messages to arrive before closing
    close: (bool, bool),
    /// Log file for the last run.
    log: Option<File>,
    /// start Instant of the simulation, for debug tracking.
    start: Instant,
}

impl<const BLOCK_BW: usize, const MSG_BW: usize, MessageType: Pod + Zeroable + Clone>
    Galaxy<BLOCK_BW, MSG_BW, MessageType>
{
    /// Create a new `Galaxy` with `planet_count: usize` maximum number of clusters, and `block_batch_size` arena allocation sizing for block logging.
    pub fn new(planet_count: usize, block_batch_size: usize) -> Result<Self, AikaError> {
        let start = Instant::now();
        let mut planet_ids = Vec::new();
        for i in 0..planet_count {
            planet_ids.push(i);
        }
        let messenger = ThreadedMessenger::new(planet_ids)?;

        Ok(Self {
            consensus: Consensus::new(ComputeLayout::HubSpoke, block_batch_size)?,
            interplanetary_messenger: messenger,
            time: HTime {
                gvt: 0,
                cp_hz: u64::MAX,
                terminal: u64::MAX,
            },
            max_block_dur: 64,
            registered: 0,
            planet_count,
            close: (false, false),
            log: None,
            start,
        })
    }

    /// Set the time scale of the simulation (its time step size, and the latest time of termination).
    pub fn set_time_scale(&mut self, terminal: u64) {
        self.time.terminal = terminal;
    }

    /// Set the synchronization checkpoint frequency.
    pub fn checkpoints(&mut self, frequency: u64) {
        self.time.cp_hz = frequency
    }

    /// Set the duration of a block within the simulation.
    pub fn with_block_duration(&mut self, duration: u64) {
        self.max_block_dur = duration;
    }

    /// Spawn a new simulation cluster on this `Galaxy`'s coordination infrastructure.
    pub fn spawn_planet<const CLOCK_BW: usize, const CLOCK_SCALES: usize>(
        &mut self,
        env: impl Environment + 'static,
    ) -> Result<Planet<BLOCK_BW, MSG_BW, CLOCK_BW, CLOCK_SCALES, MessageType>, AikaError> {
        if self.registered == self.planet_count {
            return Err(AikaError::MaximumClustersAllowed);
        }
        let id = self.registered;
        self.registered += 1;
        let messenger_account = self.interplanetary_messenger.get_user(id)?;
        let mut spoke = self.consensus.register_producer(None)?.unwrap();
        spoke.block.max_dur = self.max_block_dur;
        spoke.block.dur = self.max_block_dur;
        Planet::from_galaxy_registration(env, self.time, spoke, messenger_account, id, self.start)
    }

    // Sets the log file
    pub fn set_log(&mut self, file: File) {
        self.log = Some(file);
    }

    // Poll and and deliver the mail to the appropriate cluster ID if found.
    fn deliver_the_mail(&mut self) -> Result<(), AikaError> {
        match self.interplanetary_messenger.poll() {
            Ok(msgs) => {
                if let Some(file) = &mut self.log {
                    writeln!(
                        file,
                        "[{}] Found {:?} messages in-transit.",
                        self.start.elapsed().as_micros(),
                        msgs.len()
                    )
                    .map_err(|_| AikaError::LoggingWriteError)?;
                }
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

    // Check if all clusters are at terminal time.
    fn check_all_terminal(&mut self) -> Result<bool, AikaError> {
        //println!("GVT Master: checking terminal condition at time {:?}", self.time.gvt);
        if self.time.gvt >= self.time.terminal {
            return Ok(true);
        }
        let latest = self.consensus.fetch_latest_uncommited_blocks()?;
        let any = latest.is_empty();
        let mut truth = true;
        for block in latest {
            if let Some(block) = block {
                truth = (block.start + block.dur) >= self.time.terminal;
                continue;
            }
            return Ok(false);
        }
        Ok(if !any { truth } else { false })
    }

    /// Master loop. Polls and delivers the mail, then checks for block updates,
    /// and if theres a potential GVT update to send out. Will only break once all
    /// blocks have been processed.
    pub fn master(mut self) -> Result<Self, AikaError> {
        if self.time.terminal == u64::MAX {
            return Err(AikaError::MustSetTerminalTime);
        }
        loop {
            self.time.gvt = self.consensus.safe_point;
            // mail
            for _ in 0..10 {
                if !self.close.1 {
                    self.deliver_the_mail()?;
                }
                self.consensus.poll_n_slot()?;
                while let Some(new_gvt) = self.consensus.cusp()? {
                    if new_gvt == self.time.gvt {
                        break;
                    }
                    self.consensus.processor.broadcast_new_safe_point(new_gvt)?;
                }
            }

            if self.check_all_terminal()? {
                if self.time.gvt == self.time.terminal {
                    break;
                }
            }
        }
        Ok(self)
    }

    /// Master loop. Polls and delivers the mail, then checks for block updates,
    /// and if theres a potential GVT update to send out. Will only break once all
    /// blocks have been processed.
    pub fn master_debug(mut self) -> Result<Self, AikaError> {
        writeln!(
            self.log.as_mut().unwrap(),
            "[{}] Start block: {:?}",
            self.start.elapsed().as_micros(),
            self.consensus.blocks.read_state::<Block<BLOCK_BW>>()
        )
        .map_err(|_| AikaError::LoggingWriteError)?;
        if self.time.terminal == u64::MAX {
            return Err(AikaError::MustSetTerminalTime);
        }
        loop {
            self.time.gvt = self.consensus.safe_point;
            // mail
            writeln!(
                self.log.as_mut().unwrap(),
                "[{}] GVT {:?}: delivering mail and polling new blocks...",
                self.start.elapsed().as_micros(),
                self.time.gvt
            )
            .map_err(|_| AikaError::LoggingWriteError)?;
            for _ in 0..10 {
                if !self.close.1 {
                    self.deliver_the_mail()?;
                }
                self.consensus.poll_n_slot()?;
                while let Some(new_gvt) = self
                    .consensus
                    .cusp_debug(self.log.as_mut().unwrap(), self.start)?
                {
                    if new_gvt == self.time.gvt {
                        break;
                    }
                    writeln!(
                        self.log.as_mut().unwrap(),
                        "[{}] GVT Master: Broadcasting new safe point: {new_gvt}",
                        self.start.elapsed().as_micros()
                    )
                    .map_err(|_| AikaError::LoggingWriteError)?;
                    self.consensus.processor.broadcast_new_safe_point(new_gvt)?;
                }
            }

            if self.check_all_terminal()? {
                writeln!(
                    self.log.as_mut().unwrap(),
                    "[{}] GVT Master, GVT {:?}: all planets are waiting",
                    self.start.elapsed().as_micros(),
                    self.time.gvt
                )
                .map_err(|_| AikaError::LoggingWriteError)?;
                if self.time.gvt == self.time.terminal {
                    writeln!(
                        self.log.as_mut().unwrap(),
                        "[{}] GVT {:?}: GVT has caught up, consensus reached!",
                        self.start.elapsed().as_micros(),
                        self.time.gvt
                    )
                    .map_err(|_| AikaError::LoggingWriteError)?;
                    break;
                }
            }
        }
        Ok(self)
    }
}

unsafe impl<const BLOCK_BW: usize, const MSG_BW: usize, MessageType: Pod + Zeroable + Clone> Send
    for Galaxy<BLOCK_BW, MSG_BW, MessageType>
{
}
unsafe impl<const BLOCK_BW: usize, const MSG_BW: usize, MessageType: Pod + Zeroable + Clone> Sync
    for Galaxy<BLOCK_BW, MSG_BW, MessageType>
{
}