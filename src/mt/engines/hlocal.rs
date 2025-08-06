//! `hlocal` contains the infrastructure for `aika`'s hybrid synchronization model, configured to operate with a GVT Master thread.
//!
//! `aika`'s hybrid model is inspired by [Local Time Warp](https://dl.acm.org/doi/abs/10.1145/158459.158474); where local "clusters",
//! or in our case `Planet`, operate conservatively on a partition of the global state, while inter-cluster coordination,
//! in otherwords `Galaxy`, synchronizes clusters optimistically. The GVT is computed asynchronously using a block-based message
//! counting algorithm, defined [here](https://docs.rs/mesocarp/latest/mesocarp/sync/gvt/aika/index.html). The block-based nature
//! implies this is inherently a conservative GVT update: its accuracy is at a trade off with resource usage in the stack during runtime.
//! Additionally, the model supports a synchronization checkpointing system to help reduce the possibility and depth of rollback
//! cascades between clusters.
//!
//! This `hlocal` variant of aika's hybrid model is structured centrally around the `Galaxy` thread, which manage block update processing,
//! GVT update broadcasts to clusters, and inter-cluster message passing via a message bus.
use std::io::Write;
use std::sync::{Arc, Barrier, Mutex};
use std::time::Instant;
use std::{
    cmp::{min, Reverse},
    collections::{BTreeSet, BinaryHeap},
    fs::File,
    thread::sleep,
    time::Duration,
};

use bytemuck::{Pod, Zeroable};
use mesocarp::{
    comms::mailbox::{Message, ThreadedMessenger, ThreadedMessengerUser},
    scheduling::Scheduleable,
    MesoError,
};

use crate::mt::consensus::ComputeLayout;
use crate::{
    actors::{ConnectedActor, Context},
    env::Environment,
    mt::{
        consensus::{Block, BlockSpoke, Consensus},
        engines::HTime,
        logging::setup_hlocal_logging,
        RunMode,
    },
    objects::{AntiMsg, Event, LocalScheduler, Mail, Msg, SchedulingTask, Transfer},
    AikaError,
};

#[derive(Debug, Copy, Clone)]
pub struct Config {
    pub clusters: usize,
    pub batch_size: usize,
    pub block_duration: u64,
    pub terminal: u64,
    pub checkpoint_frequency: u64,
}

impl Config {
    pub fn new(
        clusters: usize,
        batch_size: usize,
        block_duration: u64,
        terminal: u64,
        checkpoint_frequency: u64,
    ) -> Self {
        Self {
            clusters,
            batch_size,
            block_duration,
            terminal,
            checkpoint_frequency,
        }
    }
}

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
    fn set_log(&mut self, file: File) {
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
                while let Some(new_gvt) = self.consensus.check_update_safe_point()? {
                    if new_gvt == self.time.gvt {
                        break;
                    }
                    self.consensus.processor.broadcast_new_safe_point(new_gvt)?;
                }
            }

            if self.check_all_terminal()? {
                if self.consensus.check_status() {
                    break;
                }
                let terminal = self.time.terminal;
                if self.consensus.all_producers_at_terminal(terminal) {
                    self.close.0 = true;
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
                if self.consensus.check_status() {
                    writeln!(
                        self.log.as_mut().unwrap(),
                        "[{}] GVT {:?}: GVT has caught up, consensus reached!",
                        self.start.elapsed().as_micros(),
                        self.time.gvt
                    )
                    .map_err(|_| AikaError::LoggingWriteError)?;
                    break;
                }
                let terminal = self.time.terminal;
                if self.consensus.all_producers_at_terminal(terminal) {
                    self.close.0 = true;
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

#[derive(Debug)]
/// A `Planet` is a local simulation cluster within a `Galaxy` system, owns a partition of global simulation state.
/// It operates conservative with respect to its local actors, but allows rollbacks from causality violations
/// triggered in inter-cluster messaging.
pub struct Planet<
    const BLOCK_BW: usize,
    const MSG_BW: usize,
    const CLOCK_BW: usize,
    const CLOCK_SCALES: usize,
    MessageType: Pod + Zeroable + Clone,
> {
    /// Collection of actors contributing to this cluster.
    pub actors: Vec<Box<dyn ConnectedActor<MessageType>>>,
    /// Cluster's context. cluster and local actor states live here.
    pub context: Context<MessageType>,
    event_system: LocalScheduler<CLOCK_BW, CLOCK_SCALES, Event>,
    local_messages: LocalScheduler<CLOCK_BW, CLOCK_SCALES, Msg<MessageType>>,
    message_user: ThreadedMessengerUser<MSG_BW, Mail<MessageType>>,
    blocks: BlockSpoke<BLOCK_BW>,
    /// Current time information of the simulation. these should always be the same across simulation clusters in the same `Galaxy`.
    pub time: HTime,
    rollback_active: bool,
    brakes: bool,
    log: Option<File>,
    start: Instant,
}

unsafe impl<
        const BLOCK_BW: usize,
        const MSG_BW: usize,
        const CLOCK_BW: usize,
        const CLOCK_SCALES: usize,
        MessageType: Pod + Zeroable + Clone,
    > Send for Planet<BLOCK_BW, MSG_BW, CLOCK_BW, CLOCK_SCALES, MessageType>
{
}
unsafe impl<
        const BLOCK_BW: usize,
        const MSG_BW: usize,
        const CLOCK_BW: usize,
        const CLOCK_SCALES: usize,
        MessageType: Pod + Zeroable + Clone,
    > Sync for Planet<BLOCK_BW, MSG_BW, CLOCK_BW, CLOCK_SCALES, MessageType>
{
}

impl<
        const BLOCK_BW: usize,
        const MSG_BW: usize,
        const CLOCK_BW: usize,
        const CLOCK_SCALES: usize,
        MessageType: Pod + Zeroable + Clone,
    > Planet<BLOCK_BW, MSG_BW, CLOCK_BW, CLOCK_SCALES, MessageType>
{
    pub(crate) fn from_galaxy_registration(
        env: impl Environment + 'static,
        time: HTime,
        blocks: BlockSpoke<BLOCK_BW>,
        message_user: ThreadedMessengerUser<MSG_BW, Mail<MessageType>>,
        id: usize,
        start: Instant,
    ) -> Result<Self, AikaError> {
        let terminal = time.terminal;
        Ok(Self {
            actors: Vec::new(),
            context: Context::new(env, true, id, terminal),
            event_system: LocalScheduler::new()?,
            local_messages: LocalScheduler::new()?,
            message_user,
            blocks,
            time,
            rollback_active: false,
            brakes: false,
            log: None,
            start,
        })
    }

    fn commit(&mut self, event: Event) {
        self.event_system.insert(event)
    }

    fn commit_mail(&mut self, msg: Msg<MessageType>) {
        self.local_messages.insert(msg)
    }

    /// Schedule an event for an actor at a given time.
    pub fn schedule(&mut self, time: u64, actor: usize) -> Result<(), AikaError> {
        if time < self.now() {
            return Err(AikaError::TimeTravel);
        } else if time > self.time.terminal {
            return Err(AikaError::PastTerminal);
        }
        let now = self.now();
        self.commit(Event::new(now, time, actor, SchedulingTask::Wait));
        Ok(())
    }

    /// Get the current time of the simulation.
    #[inline(always)]
    pub fn now(&self) -> u64 {
        self.event_system.clock.time
    }

    /// Spawn a new `ConnectedActor` on this cluster. Specify the arena size for its state allocator.
    pub fn spawn_actor(&mut self, actor: impl ConnectedActor<MessageType> + 'static) -> usize {
        let actor = Box::new(actor);
        self.actors.push(actor);
        self.actors.len() - 1
    }

    // Sets the log file
    fn set_log(&mut self, file: File) {
        self.log = Some(file);
    }

    // NEED TO REVIEW
    fn rollback(&mut self, time: u64) -> Result<(), AikaError> {
        let now = self.now();
        if time > now {
            return Err(AikaError::TimeTravel);
        }
        if time < self.blocks.block.start {
            self.rollback_active = true;
        }
        // rollback world and actor states
        self.context.env.rollback(time);
        // rollback local message scheduler
        self.local_messages.rollback(time);
        // rollback and claim all the anti messages produced after the rollback time
        let anti_msgs: Vec<(Mail<MessageType>, u64)> = self.context.anti_msgs.rollback_return(time);

        // send out anti messages generated post rollback.
        for (anti, _) in anti_msgs {
            let anti_time = anti.transfer.commit_time();
            if let Some(to) = anti.to_world {
                if to == self.context.cluster_id {
                    let anti = anti.open_letter();
                    if let Transfer::AntiMsg(anti) = anti {
                        self.annihilate(anti);
                    }
                } else {
                    self.message_user.send(anti)?;
                }
            } else {
                self.message_user.send(anti)?;
            }
            if anti_time < self.blocks.block.start {
                let blocks_past = ((self.blocks.block.start - anti_time - 1)
                    / self.blocks.block.max_dur) as usize;
                if blocks_past >= BLOCK_BW {
                    return Err(AikaError::MesoError(MesoError::DistantBlocks(blocks_past)));
                }
                self.blocks.block.delayed_corrections[blocks_past] -= 1;
                continue;
            }
            self.blocks.block.local_corrections -= 1;
        }

        // rollback local event scheduling system.
        self.event_system.rollback(time);
        // reset context time
        self.context.time = time;

        if let Some(file) = &mut self.log {
            writeln!(
                file,
                "[{}] Time {now}: ROLLBACK!!!!! rolling back to {time}",
                self.start.elapsed().as_micros(),
            )
            .map_err(|_| AikaError::LoggingWriteError)?;
        }
        Ok(())
    }

    // NEED TO REVIEW
    fn annihilate(&mut self, anti_msg: AntiMsg) {
        let time = anti_msg.time();
        let idxs = self.local_messages.clock.current_idxs;
        let diff = (time - self.local_messages.clock.time) as usize;
        for (k, idx) in idxs.iter().enumerate().take(CLOCK_SCALES) {
            let startidx = ((CLOCK_BW).pow(1 + k as u32) - CLOCK_BW) / (CLOCK_BW - 1); // start index for each level
            let endidx = ((CLOCK_BW).pow(2 + k as u32) - CLOCK_BW) / (CLOCK_BW - 1) - 1; // end index for each level
            if diff >= startidx {
                if diff >= (((CLOCK_BW).pow(1 + CLOCK_SCALES as u32) - CLOCK_BW) / (CLOCK_BW - 1)) {
                    break;
                }
                if diff > endidx {
                    continue;
                }
                let offset = ((diff - startidx) / (CLOCK_BW.pow(k as u32)) + idx) % CLOCK_BW;
                let msgs = &mut self.local_messages.clock.wheels[k][offset];
                let mut remaining = Vec::new();
                while let Some(msg) = msgs.pop() {
                    if anti_msg.annihilate(&msg) {
                        continue;
                    }
                    remaining.push(msg);
                }
                *msgs = remaining;
                return;
            }
        }
        // fallback if timestamp beyond clock horizon
        let mut to_be_removed = BTreeSet::new();
        for i in self.local_messages.overflow.iter().enumerate() {
            if anti_msg.annihilate(&i.1 .0) {
                to_be_removed.insert(Reverse(i.0));
            }
        }
        let current = self.local_messages.overflow.clone();
        let mut vec = current.into_iter().collect::<Vec<_>>();
        for i in to_be_removed {
            let idx = i.0;
            vec.remove(idx);
        }
        self.local_messages.overflow = BinaryHeap::from_iter(vec);
    }

    // Poll for inter-cluster messages and slot appropriately or rollback if necessary. Count the receives.
    fn poll_interplanetary_messenger(&mut self) -> Result<(), AikaError> {
        let maybe = self.message_user.poll();
        if maybe.is_none() {
            return Ok(());
        }
        for msg in maybe.unwrap() {
            if let Some(to) = msg.to_world {
                if to != self.context.cluster_id {
                    if let Some(file) = &mut self.log {
                        writeln!(
                            file,
                            "[{}] !!! PANIC !!! Mismatched delivery addresses. Was meant for cluster No. {to}, received by cluster No. {:?}. source actor {:?} on planet {:?}", 
                            self.start.elapsed().as_micros(),
                            self.context.cluster_id,
                            msg.transfer.from(),
                            msg.from_world
                        ).map_err(|_| AikaError::LoggingWriteError)?;
                    }
                    return Err(AikaError::MismatchedDeliveryAddress);
                }
            }
            let time = msg.transfer.time();
            // println!(
            //     "Planet {:?}: opening mail with recieve time {time}",
            //     self.context.cluster_id
            // );
            let now = self.now();
            if time < now {
                if let Some(file) = &mut self.log {
                    writeln!(
                        file,
                        "[{}] Local virtual time {:?}: found old message in poll with recieve time {time}.",
                        self.start.elapsed().as_micros(),
                        now
                    ).map_err(|_| AikaError::LoggingWriteError)?;
                }
                self.rollback(time)?;
            }

            match msg.open_letter() {
                Transfer::Msg(msg) => {
                    self.blocks.block.recv(msg.commit_time())?;
                    self.commit_mail(msg)
                }
                Transfer::AntiMsg(anti_msg) => {
                    self.blocks.block.recv_anti(anti_msg.commit_time())?;
                    self.annihilate(anti_msg)
                }
            }
        }
        Ok(())
    }

    // Increment the local clock one step and submit the current block if there is a turnover.
    fn increment(&mut self) -> Result<(), AikaError> {
        self.event_system.increment();
        self.local_messages.increment();
        // check-process block now
        self.context.time += 1;
        let end = self.blocks.block.start + self.blocks.block.dur;
        if self.context.time == end {
            let dur = self.blocks.block.max_dur;
            let mut new_id = self.blocks.block.block_id();
            new_id.1 += 1;
            if let Some(file) = &mut self.log {
                writeln!(
                    file,
                    "[{}] Local virtual time {:?}: submitted block number {:?}, new proposed safe virtual time {end}",
                    self.start.elapsed().as_micros(),
                    self.context.time,
                    new_id.1 - 1
                ).map_err(|_| AikaError::LoggingWriteError)?;
                writeln!(
                    file,
                    "[{}] Block: {:?}",
                    self.start.elapsed().as_micros(),
                    self.blocks.block
                )
                .map_err(|_| AikaError::LoggingWriteError)?;
            }
            self.blocks
                .submitter
                .write(std::mem::take(&mut self.blocks.block))?;
            self.blocks.block.block_nmb = new_id.1;
            self.blocks.block.producer_id = new_id.0;
            self.blocks.block.start = self.context.time;
            let diff = self.time.terminal - self.now();
            self.blocks.block.dur = min(dur, diff);
            self.blocks.block.max_dur = dur;
        }
        Ok(())
    }

    // Check the synchronization of all clocks, and ensure GVT is acting as it should, and we are not at terminal time yet.
    fn check_time_validity(&self) -> Result<(), AikaError> {
        if self.context.time != self.local_messages.clock.time
            || self.local_messages.clock.time != self.event_system.clock.time
        {
            return Err(AikaError::ClockSyncIssue);
        }
        if self.time.gvt > self.context.time {
            return Err(AikaError::GVTPastLocalClock(
                self.context.cluster_id,
                self.context.time,
                self.time.gvt,
            ));
        }
        if self.time.gvt < self.time.terminal && self.context.time > self.time.terminal {
            return Err(AikaError::PastTerminalButGVTBehind);
        }
        if self.time.gvt >= self.time.terminal && self.context.time > self.time.terminal {
            return Err(AikaError::PastTerminal);
        }
        Ok(())
    }

    // Take one step in local cluster time.
    pub(crate) fn step(&mut self) -> Result<(), AikaError> {
        let now = self.now();
        if let Some(file) = &mut self.log {
            writeln!(
                file,
                "[{}] step starting at time {:?}",
                self.start.elapsed().as_micros(),
                now
            )
            .map_err(|_| AikaError::LoggingWriteError)?;
        }
        if let Ok(msgs) = self.local_messages.clock.tick() {
            for msg in msgs {
                let id = msg.to;
                if id.is_none() {
                    for i in 0..self.actors.len() {
                        self.actors[i].read_message(&mut self.context, msg, i)?;
                    }
                    continue;
                }
                let id = id.unwrap();
                self.actors[id].read_message(&mut self.context, msg, id)?;
            }
        }
        // process events at the next time step
        if let Ok(events) = self.event_system.clock.tick() {
            for event in events {
                let event = self.actors[event.actor].step(&mut self.context, event.actor)?;
                match event.task {
                    SchedulingTask::Timeout(time) => {
                        if (self.now() + time) > self.time.terminal {
                            continue;
                        }

                        self.commit(Event::new(
                            self.now(),
                            self.now() + time,
                            event.actor,
                            SchedulingTask::Wait,
                        ));
                    }
                    SchedulingTask::Schedule(time) => {
                        self.commit(Event::new(
                            self.now(),
                            time,
                            event.actor,
                            SchedulingTask::Wait,
                        ));
                    }
                    SchedulingTask::Trigger { time, idx } => {
                        self.commit(Event::new(self.now(), time, idx, SchedulingTask::Wait));
                    }
                    SchedulingTask::Wait => {}
                    SchedulingTask::Break => {
                        self.brakes = true;
                        break;
                    }
                }
            }
        }

        // collect and send all the sends gathered this time step
        let now = self.now();
        let sends = std::mem::take(&mut self.context.outbox);
        for mail in sends {
            let mut local = false;
            if Some(self.context.cluster_id) == mail.to_world {
                match mail.open_letter() {
                    Transfer::Msg(msg) => self.commit_mail(msg),
                    Transfer::AntiMsg(anti_msg) => self.annihilate(anti_msg),
                }
                local = true;
            } else {
                self.message_user.send(mail)?;
            }
            if now < self.blocks.block.start {
                let blocks_past =
                    ((self.blocks.block.start - now - 1) / self.blocks.block.max_dur) as usize;
                if blocks_past >= BLOCK_BW {
                    return Err(AikaError::MesoError(MesoError::DistantBlocks(blocks_past)));
                }
                self.blocks.block.delayed_corrections[blocks_past] += 1;
                continue;
            }
            if !local {
                self.blocks.block.sends += 1;
            }
        }

        // increment the clock to the next step
        self.increment()?;
        Ok(())
    }

    /// Run the local cluster. Master loop polls the inter-cluster messenger, checks for GVT updates, then checks its time
    /// validity to proceed. If all is safe to proceed, step the simulation one time step, and check if we now meet the
    /// termination requirements. If not, yield the thread and repeat.
    pub fn run(mut self) -> Result<Self, AikaError> {
        if self.time.terminal == u64::MAX {
            return Err(AikaError::MustSetTerminalTime);
        }
        if self.blocks.block.dur == 0 {
            return Err(AikaError::MustSetBlockDuration);
        }
        loop {
            for _ in 0..8 {
                self.poll_interplanetary_messenger()?;
            }
            if let Some(gvt) = self.blocks.subscriber.try_recv() {
                if gvt < self.time.gvt {
                    return Err(AikaError::GVTisDecreasing);
                }
                self.time.gvt = gvt;
            }
            let now = self.now();
            // make sure time is valid to proceed.
            match self.check_time_validity() {
                Ok(_) => {}
                Err(err) => match err {
                    AikaError::PastTerminalButGVTBehind => {
                        sleep(Duration::from_nanos(100));
                        std::thread::yield_now();
                        continue;
                    }
                    AikaError::PastTerminal => break,
                    _ => return Err(err),
                },
            }
            // if at a checkpoint limit, busy-wait the thread until the GVT catches up
            if self.time.cp_hz != u64::MAX
                && now
                    == (self.time.cp_hz
                        * self.blocks.block.max_dur
                        * self.blocks.block.block_nmb as u64)
                && now != self.time.terminal
                && self.time.gvt != now
            {
                sleep(Duration::from_nanos(100));
                std::thread::yield_now();
                continue;
            }
            self.step()?;
            if self.brakes {
                break;
            }
            std::thread::yield_now();
        }
        Ok(self)
    }

    #[allow(dead_code)]
    pub(crate) fn run_debug(mut self) -> Result<Self, AikaError> {
        if self.time.terminal == u64::MAX {
            return Err(AikaError::MustSetTerminalTime);
        }
        if self.blocks.block.dur == 0 {
            return Err(AikaError::MustSetBlockDuration);
        }
        let mut counter = 0;
        loop {
            for _ in 0..8 {
                self.poll_interplanetary_messenger()?;
            }
            if let Some(gvt) = self.blocks.subscriber.try_recv() {
                writeln!(
                    self.log.as_mut().unwrap(),
                    "[{}] new GVT found: {gvt}",
                    self.start.elapsed().as_micros()
                )
                .map_err(|_| AikaError::LoggingWriteError)?;
                if gvt < self.time.gvt {
                    return Err(AikaError::GVTisDecreasing);
                }
                self.time.gvt = gvt;
            }
            let now = self.now();
            // make sure time is valid to proceed.
            match self.check_time_validity() {
                Ok(_) => {}
                Err(err) => match err {
                    AikaError::PastTerminalButGVTBehind => {
                        match counter.cmp(&10) {
                            std::cmp::Ordering::Less => {
                                writeln!(
                                    self.log.as_mut().unwrap(),
                                    "[{}]: waiting for GVT to catch up, continuing to poll.",
                                    self.start.elapsed().as_micros(),
                                )
                                .map_err(|_| AikaError::LoggingWriteError)?;
                            }
                            std::cmp::Ordering::Equal => {
                                writeln!(
                                    self.log.as_mut().unwrap(),
                                    "[{}]: waiting too long for GVT, going quiet.",
                                    self.start.elapsed().as_micros(),
                                )
                                .map_err(|_| AikaError::LoggingWriteError)?;
                            }
                            _ => {}
                        }
                        counter += 1;
                        sleep(Duration::from_nanos(100));
                        std::thread::yield_now();
                        continue;
                    }
                    AikaError::PastTerminal => {
                        writeln!(
                            self.log.as_mut().unwrap(),
                            "[{}]: Past terminal time detected, breaking.",
                            self.start.elapsed().as_micros(),
                        )
                        .map_err(|_| AikaError::LoggingWriteError)?;
                        break;
                    }
                    _ => return Err(err),
                },
            }
            counter = 0;
            // if at a checkpoint or the throttle limit, busy-wait the thread
            if self.time.cp_hz != u64::MAX
                && now
                    == (self.time.cp_hz
                        * self.blocks.block.max_dur
                        * self.blocks.block.block_nmb as u64)
                && now != self.time.terminal
                && self.time.gvt != now
            {
                writeln!(
                    self.log.as_mut().unwrap(),
                    "[{}] checkpoint sleeping",
                    self.start.elapsed().as_micros()
                )
                .map_err(|_| AikaError::LoggingWriteError)?;
                sleep(Duration::from_nanos(100));
                std::thread::yield_now();
                continue;
            }
            self.step()?;
            if self.brakes {
                break;
            }
            std::thread::yield_now();
        }
        let time = self.now();
        writeln!(
            self.log.as_mut().unwrap(),
            "[{}] Terminated with local clock {:?}.",
            self.start.elapsed().as_micros(),
            time
        )
        .map_err(|_| AikaError::LoggingWriteError)?;
        Ok(self)
    }
}

pub struct Stager<
    const BLOCK_BW: usize,
    const MSG_BW: usize,
    const CLOCK_BW: usize,
    const CLOCK_SCALES: usize,
    MessageType: Pod + Zeroable + Clone,
> {
    pub galaxy: Option<Galaxy<BLOCK_BW, MSG_BW, MessageType>>,
    pub planets: Vec<Planet<BLOCK_BW, MSG_BW, CLOCK_BW, CLOCK_SCALES, MessageType>>,
    configured: bool,
}

impl<
        const BLOCK_BW: usize,
        const MSG_BW: usize,
        const CLOCK_BW: usize,
        const CLOCK_SCALES: usize,
        MessageType: Pod + Zeroable + Clone,
    > Stager<BLOCK_BW, MSG_BW, CLOCK_BW, CLOCK_SCALES, MessageType>
{
    pub fn new() -> Result<Self, AikaError> {
        Ok(Self {
            galaxy: None,
            planets: Vec::new(),
            configured: false,
        })
    }

    pub fn config(&mut self, config: Config) -> Result<(), AikaError> {
        let mut galaxy = Galaxy::new(config.clusters, config.batch_size)?;
        galaxy.set_time_scale(config.terminal);
        galaxy.with_block_duration(config.block_duration);
        galaxy.checkpoints(config.checkpoint_frequency);
        self.galaxy = Some(galaxy);
        self.configured = true;
        Ok(())
    }

    pub fn create_cluster(&mut self, env: impl Environment + 'static) -> Result<(), AikaError> {
        if self.configured {
            let cluster = self.galaxy.as_mut().unwrap().spawn_planet(env)?;
            self.planets.push(cluster);
            return Ok(());
        }
        Err(AikaError::UnconfiguredStager)
    }

    pub fn spawn_actor_on_cluster(
        &mut self,
        cluster: usize,
        actor: impl ConnectedActor<MessageType> + 'static,
    ) -> Result<(), AikaError> {
        let clusters = self.planets.len();
        if clusters <= cluster {
            return Err(AikaError::InvalidClusterId(clusters, cluster));
        }
        self.planets[cluster].spawn_actor(actor);
        Ok(())
    }

    pub fn schedule(&mut self, cluster: usize, actor: usize, time: u64) -> Result<(), AikaError> {
        let clusters = self.planets.len();
        if clusters <= cluster {
            return Err(AikaError::InvalidClusterId(clusters, cluster));
        }
        let actors = self.planets[cluster].actors.len();
        if actors <= actor {
            return Err(AikaError::InvalidActorId(actors, cluster, actor));
        }
        self.planets[cluster].schedule(time, actor)?;
        Ok(())
    }

    pub fn schedule_cluster(&mut self, cluster: usize, time: u64) -> Result<(), AikaError> {
        let clusters = self.planets.len();
        if clusters <= cluster {
            return Err(AikaError::InvalidClusterId(clusters, cluster));
        }
        let actors = self.planets[cluster].actors.len();
        for i in 0..actors {
            self.planets[cluster].schedule(time, i)?;
        }
        Ok(())
    }

    pub fn schedule_all(&mut self, time: u64) -> Result<(), AikaError> {
        let clusters = self.planets.len();
        for cluster in 0..clusters {
            let actors = self.planets[cluster].actors.len();
            for i in 0..actors {
                self.planets[cluster].schedule(time, i)?;
            }
        }
        Ok(())
    }

    pub fn run(self, mode: RunMode) -> Result<Self, AikaError> {
        match self.check_ready() {
            Ok(_) => {}
            Err(err) => match err {
                None => {
                    return Err(AikaError::NotAllClustersRegistered);
                }
                Some(i) => return Err(AikaError::NoActors(i)),
            },
        }

        let num_threads = self.planets.len() + 1;
        let barrier = Arc::new(Barrier::new(num_threads));
        let start_time = Arc::new(Mutex::new(None::<Instant>));

        let mut pfiles = vec![];
        let mut gfile = None;
        if RunMode::Debug == mode {
            let mut files = setup_hlocal_logging(self.planets.len())
                .map_err(|_| AikaError::LoggingSetupFailure)?;
            gfile = files.pop();
            pfiles = files;
        }

        let mut galaxy = self.galaxy.unwrap();
        let planets = self.planets;
        let phandles = match mode {
            RunMode::Fast => planets
                .into_iter()
                .enumerate()
                .map(|(i, planet)| {
                    let barrier_clone = Arc::clone(&barrier);
                    std::thread::Builder::new()
                        .name(format!("Cluster {i}"))
                        .spawn(move || {
                            barrier_clone.wait();
                            let planet = planet.run_debug()?;
                            Ok::<
                                Planet<BLOCK_BW, MSG_BW, CLOCK_BW, CLOCK_SCALES, MessageType>,
                                AikaError,
                            >(planet)
                        })
                        .map_err(|_| AikaError::ThreadPanic)
                })
                .collect::<Result<Vec<_>, _>>(),
            RunMode::Debug => {
                let planets = planets.into_iter().zip(pfiles).collect::<Vec<_>>();
                planets
                    .into_iter()
                    .enumerate()
                    .map(|(i, (mut planet, file))| {
                        let barrier_clone = Arc::clone(&barrier);
                        let start_time_clone = Arc::clone(&start_time);
                        planet.set_log(file);
                        std::thread::Builder::new()
                            .name(format!("Cluster {i}"))
                            .spawn(move || {
                                if i == 0 {
                                    let mut start = start_time_clone.lock().unwrap();
                                    *start = Some(Instant::now());
                                }
                                barrier_clone.wait();
                                let planet = planet.run_debug()?;
                                Ok::<
                                    Planet<BLOCK_BW, MSG_BW, CLOCK_BW, CLOCK_SCALES, MessageType>,
                                    AikaError,
                                >(planet)
                            })
                            .map_err(|_| AikaError::ThreadPanic)
                    })
                    .collect::<Result<Vec<_>, _>>()
            }
        }?;

        let ghandle = std::thread::Builder::new()
            .name("Galaxy".to_owned())
            .spawn(move || {
                let galaxy = match mode {
                    RunMode::Fast => {
                        barrier.wait();
                        galaxy.master()?
                    }
                    RunMode::Debug => {
                        let gfile = gfile.unwrap();
                        galaxy.set_log(gfile);
                        barrier.wait();
                        galaxy.master_debug()?
                    }
                };
                Ok::<Galaxy<BLOCK_BW, MSG_BW, MessageType>, AikaError>(galaxy)
            })
            .map_err(|_| AikaError::ThreadPanic)?;

        let mut planets = Vec::new();
        for handle in phandles {
            let planet = handle.join().map_err(|_| AikaError::ThreadPanic)??;
            planets.push(planet);
        }
        let galaxy = Some(ghandle.join().map_err(|_| AikaError::ThreadPanic)??);
        let execution_time = {
            let start = start_time.lock().unwrap();
            start.unwrap().elapsed()
        };
        if mode == RunMode::Debug {
            println!("Runtime: {execution_time:?}")
        }
        Ok(Self {
            galaxy,
            planets,
            configured: true,
        })
    }

    fn check_ready(&self) -> Result<(), Option<usize>> {
        if self.galaxy.is_none() {
            return Err(None);
        }
        let galaxy = self.galaxy.as_ref().unwrap();
        if galaxy.registered != galaxy.planet_count {
            return Err(None);
        }
        for (i, planet) in self.planets.iter().enumerate() {
            if planet.actors.is_empty() {
                return Err(Some(i));
            }
        }
        Ok(())
    }
}

#[macro_export]
macro_rules! stager {
    // Just the message type - use all defaults
    ($msg_type:ty) => {
        $crate::mt::engines::hlocal::Stager::<32, 128, 128, 2, $msg_type>::new()
    };

    // With BLOCK_BW only
    ($msg_type:ty, BLOCK_BW = $block_bw:expr) => {
        $crate::mt::engines::hlocal::Stager::<$block_bw, 128, 128, 2, $msg_type>::new()
    };

    // With MSG_BW only
    ($msg_type:ty, MSG_BW = $msg_bw:expr) => {
        $crate::mt::engines::hlocal::Stager::<32, $msg_bw, 128, 2, $msg_type>::new()
    };

    // With CLOCK_BW only
    ($msg_type:ty, CLOCK_BW = $clock_bw:expr) => {
        $crate::mt::engines::hlocal::Stager::<32, 128, $clock_bw, 2, $msg_type>::new()
    };

    // With CLOCK_SCALES only
    ($msg_type:ty, CLOCK_SCALES = $clock_scales:expr) => {
        $crate::mt::engines::hlocal::Stager::<32, 128, 128, $clock_scales, $msg_type>::new()
    };

    // With BLOCK_BW and MSG_BW
    ($msg_type:ty, BLOCK_BW = $block_bw:expr, MSG_BW = $msg_bw:expr) => {
        $crate::mt::engines::hlocal::Stager::<$block_bw, $msg_bw, 128, 2, $msg_type>::new()
    };

    // With BLOCK_BW and CLOCK_BW
    ($msg_type:ty, BLOCK_BW = $block_bw:expr, CLOCK_BW = $clock_bw:expr) => {
        $crate::mt::engines::hlocal::Stager::<$block_bw, 32, $clock_bw, 2, $msg_type>::new()
    };

    // With BLOCK_BW and CLOCK_SCALES
    ($msg_type:ty, BLOCK_BW = $block_bw:expr, CLOCK_SCALES = $clock_scales:expr) => {
        $crate::mt::engines::hlocal::Stager::<$block_bw, 128, 128, $clock_scales, $msg_type>::new()
    };

    // With MSG_BW and CLOCK_BW
    ($msg_type:ty, MSG_BW = $msg_bw:expr, CLOCK_BW = $clock_bw:expr) => {
        $crate::mt::engines::hlocal::Stager::<32, $msg_bw, $clock_bw, 2, $msg_type>::new()
    };

    // With MSG_BW and CLOCK_SCALES
    ($msg_type:ty, MSG_BW = $msg_bw:expr, CLOCK_SCALES = $clock_scales:expr) => {
        $crate::mt::engines::hlocal::Stager::<32, $msg_bw, 128, $clock_scales, $msg_type>::new()
    };

    // With CLOCK_BW and CLOCK_SCALES
    ($msg_type:ty, CLOCK_BW = $clock_bw:expr, CLOCK_SCALES = $clock_scales:expr) => {
        $crate::mt::engines::hlocal::Stager::<32, 128, $clock_bw, $clock_scales, $msg_type>::new()
    };

    // With BLOCK_BW, MSG_BW, and CLOCK_BW
    ($msg_type:ty, BLOCK_BW = $block_bw:expr, MSG_BW = $msg_bw:expr, CLOCK_BW = $clock_bw:expr) => {
        $crate::mt::engines::hlocal::Stager::<$block_bw, $msg_bw, $clock_bw, 2, $msg_type>::new()
    };

    // With BLOCK_BW, MSG_BW, and CLOCK_SCALES
    ($msg_type:ty, BLOCK_BW = $block_bw:expr, MSG_BW = $msg_bw:expr, CLOCK_SCALES = $clock_scales:expr) => {
        $crate::mt::engines::hlocal::Stager::<$block_bw, $msg_bw, 128, $clock_scales, $msg_type>::new()
    };

    // With BLOCK_BW, CLOCK_BW, and CLOCK_SCALES
    ($msg_type:ty, BLOCK_BW = $block_bw:expr, CLOCK_BW = $clock_bw:expr, CLOCK_SCALES = $clock_scales:expr) => {
        $crate::mt::engines::hlocal::Stager::<$block_bw, 128, $clock_bw, $clock_scales, $msg_type>::new()
    };

    // With MSG_BW, CLOCK_BW, and CLOCK_SCALES
    ($msg_type:ty, MSG_BW = $msg_bw:expr, CLOCK_BW = $clock_bw:expr, CLOCK_SCALES = $clock_scales:expr) => {
        $crate::mt::engines::hlocal::Stager::<32, $msg_bw, $clock_bw, $clock_scales, $msg_type>::new()
    };

    // With all parameters
    ($msg_type:ty, BLOCK_BW = $block_bw:expr, MSG_BW = $msg_bw:expr, CLOCK_BW = $clock_bw:expr, CLOCK_SCALES = $clock_scales:expr) => {
        $crate::mt::engines::hlocal::Stager::<$block_bw, $msg_bw, $clock_bw, $clock_scales, $msg_type>::new()
    };
}

#[cfg(test)]
mod unit_tests {
    use std::thread;

    use super::*;
    use crate::actors::{Actor, ConnectedActor, Context};
    use crate::env::SimpleUnified;
    use crate::objects::{Event, Msg, SchedulingTask};
    use bytemuck::{Pod, Zeroable};
    use mesocarp::logging::journal::Journal;

    const AGENTS: usize = 10;
    const BLOCK_BANDWIDTH: usize = 64;
    const MSG_BANDWIDTH: usize = 8;

    #[derive(Copy, Clone, Debug)]
    #[repr(C)]
    struct TestMessage;

    unsafe impl Pod for TestMessage {}
    unsafe impl Zeroable for TestMessage {}

    #[derive(Debug)]
    struct TestAgent {
        _counter: usize,
        _id: usize,
    }

    impl TestAgent {
        pub fn new(id: usize) -> Self {
            Self {
                _counter: 0,
                _id: id,
            }
        }
    }

    impl Actor<TestMessage> for TestAgent {
        fn step(
            &mut self,
            context: &mut Context<TestMessage>,
            actor_id: usize,
        ) -> Result<Event, AikaError> {
            let journal = &mut context.env.downcast_mut::<SimpleUnified>().unwrap().inner;
            match journal.read_state::<usize>() {
                Ok(state) => {
                    journal.write(state + 1, context.time, None);
                }
                Err(err) => {
                    if let MesoError::UninitializedState = err {
                        journal.write(1usize, context.time, None);
                    }
                }
            }
            Ok(Event::new(
                context.time,
                context.time + 1,
                actor_id,
                SchedulingTask::Timeout(1),
            ))
        }
    }

    impl ConnectedActor<TestMessage> for TestAgent {
        fn read_message(
            &mut self,
            _context: &mut Context<TestMessage>,
            _msg: Msg<TestMessage>,
            _actor_id: usize,
        ) -> Result<(), AikaError> {
            Ok(())
        }
    }

    #[allow(dead_code)]
    #[derive(Copy, Clone, Debug)]
    #[repr(C)]
    struct MsgAgent;

    unsafe impl Send for MsgAgent {}
    unsafe impl Sync for MsgAgent {}

    impl Actor<TestMessage> for MsgAgent {
        fn step(
            &mut self,
            context: &mut Context<TestMessage>,
            actor_id: usize,
        ) -> Result<Event, AikaError> {
            let id = actor_id;
            let time = context.time;
            context.send_mail(
                Msg::new(TestMessage, time, time + 1, id, Some((id + 1) % AGENTS)),
                context.cluster_id,
            )?;
            Ok(Event::new(time, time, id, SchedulingTask::Wait))
        }
    }

    impl ConnectedActor<TestMessage> for MsgAgent {
        fn read_message(
            &mut self,
            context: &mut Context<TestMessage>,
            msg: Msg<TestMessage>,
            _actor_id: usize,
        ) -> Result<(), AikaError> {
            assert_eq!(context.time, msg.recv);
            Ok(())
        }
    }

    fn create_setup<
        const CLOCK_BW: usize,
        const CLOCK_SCALES: usize,
        MessageType: Pod + Zeroable + Clone,
    >(
        terminal: u64,
        block_dur: u64,
    ) -> Result<
        (
            Galaxy<BLOCK_BANDWIDTH, MSG_BANDWIDTH, MessageType>,
            Planet<BLOCK_BANDWIDTH, MSG_BANDWIDTH, CLOCK_BW, CLOCK_SCALES, MessageType>,
        ),
        AikaError,
    >
    where
        TestAgent: ConnectedActor<MessageType>,
    {
        let mut galaxy = Galaxy::<BLOCK_BANDWIDTH, MSG_BANDWIDTH, MessageType>::new(6, 12)?;
        galaxy.set_time_scale(terminal);
        galaxy.with_block_duration(block_dur);
        let mut planet = galaxy.spawn_planet::<CLOCK_BW, CLOCK_SCALES>(SimpleUnified {
            inner: Journal::init(1024),
        })?;
        for j in 0..AGENTS {
            let actor = TestAgent::new(j);
            planet.spawn_actor(actor);
        }
        Ok((galaxy, planet))
    }

    fn schedule_all<
        const CLOCK_BW: usize,
        const CLOCK_SCALES: usize,
        MessageType: Pod + Zeroable + Clone,
    >(
        planets: &mut Planet<BLOCK_BANDWIDTH, MSG_BANDWIDTH, CLOCK_BW, CLOCK_SCALES, MessageType>,
        time: u64,
    ) -> Result<(), AikaError> {
        for i in 0..AGENTS {
            planets.schedule(time, i)?;
        }
        Ok(())
    }

    #[test]
    fn test_stager_macro() {
        stager!(TestMessage).unwrap();
        stager!(TestMessage, BLOCK_BW = 48).unwrap();
        stager!(TestMessage, BLOCK_BW = 48, MSG_BW = 12).unwrap();
        stager!(TestMessage, MSG_BW = 12, CLOCK_BW = 128).unwrap();
        stager!(TestMessage, CLOCK_BW = 128, CLOCK_SCALES = 1).unwrap();
    }

    #[test]
    fn test_simple_setup() {
        let (galaxy, mut planets) = create_setup::<64, 2, TestMessage>(1, 1).unwrap();
        schedule_all(&mut planets, 0).unwrap();
        let ghandle = thread::spawn(move || galaxy.master());
        let phandle = thread::spawn(move || planets.run());

        let galaxy_result = ghandle.join().expect("Galaxy thread panicked");
        let planet_result = phandle.join().expect("Planet thread panicked");

        // Ensure both threads completed successfully
        assert!(galaxy_result.is_ok());
        assert!(
            planet_result.is_ok(),
            "Planet run loop failed: {planet_result:?}"
        );
    }

    #[test]
    fn test_multiplanet_setup() {
        const CLUSTERS: usize = 6;
        let mut stager =
            Stager::<BLOCK_BANDWIDTH, MSG_BANDWIDTH, 64, 2, TestMessage>::new().unwrap();

        let config = Config {
            clusters: CLUSTERS,
            batch_size: 12,
            block_duration: 1,
            terminal: 20,
            checkpoint_frequency: u64::MAX,
        };
        stager.config(config).unwrap();

        for i in 0..CLUSTERS {
            let env = SimpleUnified {
                inner: Journal::init(1024),
            };
            stager.create_cluster(env).unwrap();
            for j in 0..AGENTS {
                let actor = TestAgent::new(j);
                stager.spawn_actor_on_cluster(i, actor).unwrap();
            }
        }
        stager.schedule_all(1).unwrap();

        let run_result = stager.run(RunMode::Debug);

        assert!(
            run_result.is_ok(),
            "Stager run loop failed: {:?}",
            run_result.err()
        );
    }

    #[test]
    fn test_rollback_accounting() {
        let mut galaxy: Galaxy<8, 8, TestMessage> = Galaxy::new(1, 1).unwrap();
        let mut planet = galaxy
            .spawn_planet::<16, 2>(SimpleUnified {
                inner: Journal::init(1024),
            })
            .unwrap();
        for i in 0..AGENTS {
            planet.spawn_actor(TestAgent::new(i));
            planet.schedule(0, i).unwrap();
        }
        planet.step().unwrap();
        //println!("state {:?}", planet.context.world_state.read_all::<usize>());
        planet.step().unwrap();
        //println!("state {:?}", planet.context.world_state.read_state::<usize>().unwrap());
        planet.step().unwrap();
        //println!("state {:?}", planet.context.world_state.read_state::<usize>().unwrap());
        assert_eq!(planet.now(), 3);

        let state = &planet
            .context
            .env
            .downcast_ref::<SimpleUnified>()
            .unwrap()
            .inner;
        let current = state.read_state::<usize>().unwrap();
        assert_eq!(*current, 30);
        let res = planet.rollback(1);
        let state = &planet
            .context
            .env
            .downcast_ref::<SimpleUnified>()
            .unwrap()
            .inner;
        assert!(res.is_ok());
        assert_eq!(planet.now(), 1);
        assert_eq!(planet.context.time, 1);
        let current = state.read_state::<usize>().unwrap();
        assert_eq!(*current, 10);

        let res = planet.rollback(2);
        assert!(res.is_err());
        match res.err().unwrap() {
            AikaError::TimeTravel => (),
            _ => panic!("Expected TimeTravel error"),
        }

        let (_, mut planets) = create_setup::<64, 2, TestMessage>(50, 1).unwrap();
        planets.spawn_actor(TestAgent::new(0));
        planets.spawn_actor(TestAgent::new(1));
        for i in 0..100 {
            planets.commit_mail(Msg::new(TestMessage, i, i + 10, 0, Some(1)));
            planets.context.anti_msgs.write(
                Mail::write_letter(
                    Transfer::<TestMessage>::AntiMsg(AntiMsg::new(i, i + 10, 0, Some(1))),
                    0,
                    Some(0),
                ),
                i,
                None,
            );
        }
        for _ in 0..50 {
            planets.step().unwrap();
        }
        planets.rollback(25).unwrap();
        assert_eq!(
            planets
                .context
                .anti_msgs
                .read_all::<Mail<TestMessage>>()
                .len(),
            25
        );
        assert_eq!(
            planets
                .context
                .anti_msgs
                .read_state::<Mail<TestMessage>>()
                .unwrap()
                .open_letter()
                .commit_time(),
            24
        )
    }
}

#[cfg(test)]
mod messaging_tests {
    use bytemuck::{Pod, Zeroable};

    use crate::{
        actors::{Actor, ConnectedActor},
        env::Stateless,
        mt::{
            engines::hlocal::{Config, Stager},
            RunMode,
        },
        objects::{Event, Msg},
        AikaError,
    };

    const BLOCK_BW: usize = 8;
    const MSG_BW: usize = 32;
    const CLOCK_BW: usize = 128;
    const CLOCK_SCALES: usize = 1;

    #[derive(Debug, Copy, Clone)]
    struct Message;

    unsafe impl Pod for Message {}
    unsafe impl Zeroable for Message {}

    #[derive(Debug)]
    struct MessagingActor {
        recieved: usize,
        sent: usize,
        target: Option<usize>,
        cluster: usize,
        delay1: u64,
        delay2: u64,
    }

    impl MessagingActor {
        fn new(target: Option<usize>, to_cluster: usize, delay1: u64, delay2: u64) -> Self {
            Self {
                recieved: 0,
                sent: 0,
                target,
                cluster: to_cluster,
                delay1,
                delay2,
            }
        }
    }

    impl Actor<Message> for MessagingActor {
        fn step(
            &mut self,
            env: &mut crate::actors::Context<Message>,
            actor_id: usize,
        ) -> Result<crate::prelude::Event, crate::AikaError> {
            let time = env.time;
            let msg = Msg::new(Message, time, time + self.delay1, actor_id, self.target);
            env.send_mail(msg, self.cluster)?;
            self.sent += 1;
            Ok(Event::new(
                time,
                time,
                actor_id,
                crate::objects::SchedulingTask::Wait,
            ))
        }
    }

    impl ConnectedActor<Message> for MessagingActor {
        fn read_message(
            &mut self,
            env: &mut crate::actors::Context<Message>,
            msg: crate::prelude::Msg<Message>,
            actor_id: usize,
        ) -> Result<(), AikaError> {
            self.recieved += 1;
            let time = env.time;
            if self.target != Some(msg.from) {
                let msg = Msg::new(Message, time, time + self.delay2, actor_id, Some(msg.from));
                env.send_mail(msg, self.cluster)?;
                self.sent += 1;
            } else {
                let _ = self.step(env, actor_id)?;
            }
            Ok(())
        }
    }

    #[test]
    fn test_local_messaging() {
        let mut stager: Stager<BLOCK_BW, MSG_BW, CLOCK_BW, CLOCK_SCALES, Message> =
            Stager::new().unwrap();

        let config = Config::new(1, 128, 20, 2048, 10000000);
        stager.config(config).unwrap();

        stager.create_cluster(Stateless).unwrap();

        stager
            .spawn_actor_on_cluster(0, MessagingActor::new(Some(1), 0, 5, 1))
            .unwrap();
        stager
            .spawn_actor_on_cluster(0, MessagingActor::new(Some(0), 0, 5, 1))
            .unwrap();

        stager.schedule_cluster(0, 1).unwrap();

        stager.run(RunMode::Debug).unwrap();
    }

    #[test]
    fn test_local_messaging_heavy() {
        let mut stager: Stager<BLOCK_BW, MSG_BW, CLOCK_BW, CLOCK_SCALES, Message> =
            Stager::new().unwrap();

        let config = Config::new(1, 128, 20, 2048, 10000000);
        stager.config(config).unwrap();

        stager.create_cluster(Stateless).unwrap();
        for i in 0..2000 {
            stager
                .spawn_actor_on_cluster(0, MessagingActor::new(Some((i + 1) % 200), 0, 5, 1))
                .unwrap();
        }
        stager.schedule_cluster(0, 1).unwrap();

        stager.run(RunMode::Debug).unwrap();
    }

    #[test]
    fn test_intercluster_messaging() {
        let mut stager: Stager<BLOCK_BW, MSG_BW, CLOCK_BW, CLOCK_SCALES, Message> =
            Stager::new().unwrap();

        let config = Config::new(2, 128, 20, 2048, 10000000);
        stager.config(config).unwrap();

        stager.create_cluster(Stateless).unwrap();
        stager.create_cluster(Stateless).unwrap();

        stager
            .spawn_actor_on_cluster(0, MessagingActor::new(Some(0), 1, 1, 1))
            .unwrap();
        stager
            .spawn_actor_on_cluster(1, MessagingActor::new(Some(0), 0, 1, 1))
            .unwrap();

        stager.schedule_cluster(0, 1).unwrap();
        stager.schedule_cluster(1, 1).unwrap();

        stager.run(RunMode::Debug).unwrap();
    }
}
