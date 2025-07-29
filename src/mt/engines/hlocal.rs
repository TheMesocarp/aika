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
use std::{
    cmp::{min, Reverse},
    collections::{BTreeSet, BinaryHeap},
    sync::Arc,
    thread::sleep,
    time::Duration,
};

use bytemuck::{Pod, Zeroable};
use mesocarp::{
    comms::{
        mailbox::{Message, ThreadedMessenger, ThreadedMessengerUser},
        spmc::{Broadcast, Subscriber},
        spsc::BufferWheel,
    },
    logging::journal::Journal,
    scheduling::Scheduleable,
    sync::gvt::aika::{Block, BlockSpoke, Consensus},
    MesoError,
};

use crate::{
    mt::{
        agents::{PlanetContext, ThreadedAgent},
        engines::HTime,
    },
    objects::{
        AntiMsg, Event, LocalEventSystem, LocalMailSystem, Mail, Msg, SchedulingTask, Transfer,
    },
    AikaError,
};

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
    pub close: (bool, bool),
    channels: Vec<Arc<BufferWheel<BLOCK_BW, isize>>>,
    wrapup: Arc<Broadcast<1, bool>>,
    final_counter: isize,
}

impl<const BLOCK_BW: usize, const MSG_BW: usize, MessageType: Pod + Zeroable + Clone>
    Galaxy<BLOCK_BW, MSG_BW, MessageType>
{
    /// Create a new `Galaxy` with `planet_count: usize` maximum number of clusters, and `block_batch_size` arena allocation sizing for block logging.
    pub fn new(planet_count: usize, block_batch_size: usize) -> Result<Self, AikaError> {
        let mut planet_ids = Vec::new();
        for i in 0..planet_count {
            planet_ids.push(i);
        }
        let messenger = ThreadedMessenger::new(planet_ids)?;

        Ok(Self {
            consensus: Consensus::new(
                mesocarp::sync::gvt::ComputeLayout::HubSpoke,
                block_batch_size,
            )?,
            interplanetary_messenger: messenger,
            time: HTime {
                gvt: 0,
                cp_hz: u64::MAX,
                throttle: u64::MAX,
                terminal: f64::MAX,
                timestep: 1.0,
            },
            max_block_dur: 64,
            registered: 0,
            planet_count,
            close: (false, false),
            channels: Vec::new(),
            wrapup: Arc::new(Broadcast::new()?),
            final_counter: 0,
        })
    }

    /// Set the time scale of the simulation (its time step size, and the latest time of termination).
    pub fn set_time_scale(&mut self, timestep: f64, terminal: f64) {
        self.time.terminal = terminal;
        self.time.timestep = timestep
    }

    /// Set the throttle limit for each cluster.
    pub fn throttle(&mut self, throttle: u64) {
        self.time.throttle = throttle
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
    ) -> Result<Planet<BLOCK_BW, MSG_BW, CLOCK_BW, CLOCK_SCALES, MessageType>, AikaError> {
        if self.registered == self.planet_count {
            return Err(AikaError::MaximumAgentsAllowed);
        }
        let id = self.registered;
        self.registered += 1;
        let wheel = Arc::new(BufferWheel::new());
        let cloned = Arc::clone(&wheel);
        self.channels.push(wheel);
        let messenger_account = self.interplanetary_messenger.get_user(id)?;
        let mut spoke = self.consensus.register_producer(None)?.unwrap();
        spoke.block.max_dur = self.max_block_dur;
        spoke.block.dur = self.max_block_dur;
        Planet::from_galaxy_registration(
            self.time,
            spoke,
            messenger_account,
            self.wrapup.register_subscriber(),
            cloned,
            id,
        )
    }

    // Poll and and deliver the mail to the appropriate cluster ID if found.
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

    fn poll_sends(&mut self) -> Result<isize, AikaError> {
        let mut total = 0;
        for i in &self.channels {
            match i.read() {
                Ok(size) => {
                    total += size;
                }
                Err(e) => {
                    if let MesoError::NoPendingUpdates = e {
                        continue;
                    }
                    return Err(AikaError::MesoError(e));
                }
            }
        }
        Ok(total)
    }

    // Check if all clusters are at terminal time.
    fn check_all_terminal(&mut self) -> Result<bool, AikaError> {
        //println!("GVT Master: checking terminal condition at time {:?}", self.time.gvt);
        if self.time.gvt as f64 * self.time.timestep >= self.time.terminal {
            return Ok(true);
        }
        let latest = self.consensus.fetch_latest_uncommited_blocks()?;
        let any = latest.is_empty();
        let mut truth = true;
        for block in latest {
            if let Some(block) = block {
                truth =
                    ((block.start + block.dur) as f64 * self.time.timestep) >= self.time.terminal;
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
        println!(
            "Start block: {:?}",
            self.consensus.blocks.read_state::<Block<BLOCK_BW>>()
        );
        if self.time.terminal == f64::MAX {
            return Err(AikaError::MustSetTerminalTime);
        }
        let mut counter = 0;
        loop {
            self.time.gvt = self.consensus.safe_point;
            // mail
            //println!("GVT Master, GVT {:?}: delivering mail...", self.gvt);
            if counter < 10 {
                println!(
                    "GVT Master, GVT {:?}: polling blocks, updating time consensus...",
                    self.time.gvt
                );
            }
            counter += 1;
            for _ in 0..10 {
                if !self.close.1 {
                    self.deliver_the_mail()?;
                }
                self.consensus.poll_n_slot()?;
                if !self.close.0 {
                    while let Some(new_gvt) = self.consensus.check_update_safe_point()? {
                        if new_gvt == self.time.gvt {
                            break;
                        }
                        println!("GVT Master: Broadcasting new safe point: {new_gvt}");
                        self.consensus.processor.broadcast_new_safe_point(new_gvt)?;
                    }
                } else {
                    if !self.close.1 {
                        while let Some(res) = self.consensus.cusp_return_sends()? {
                            match res {
                                Ok(gvt) => {
                                    if gvt == self.time.gvt {
                                        break;
                                    }
                                    println!("GVT Master: Broadcasting new safe point: {gvt}");
                                    self.consensus.processor.broadcast_new_safe_point(gvt)?;
                                }
                                Err(sends) => {
                                    self.final_counter += self.poll_sends()?;
                                    if sends + self.final_counter == 0 {
                                        self.close.1 = true;
                                        self.wrapup.broadcast(true);
                                    }
                                }
                            }
                        }
                    }
                    // else do some final block polling.
                }
            }

            if self.check_all_terminal()? {
                println!(
                    "GVT Master, GVT {:?}: all planets are waiting",
                    self.time.gvt
                );
                if self.consensus.check_status() {
                    println!(
                        "GVT Master, GVT {:?}: GVT has caught up, consensus reached!",
                        self.time.gvt
                    );
                    break;
                }
                let terminal = (self.time.terminal / self.time.timestep) as u64;
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

/// A `Planet` is a local simulation cluster within a `Galaxy` system, owns a partition of global simulation state.
/// It operates conservative with respect to its local agents, but allows rollbacks from causality violations
/// triggered in inter-cluster messaging.
pub struct Planet<
    const BLOCK_BW: usize,
    const MSG_BW: usize,
    const CLOCK_BW: usize,
    const CLOCK_SCALES: usize,
    MessageType: Pod + Zeroable + Clone,
> {
    /// Collection of agents contributing to this cluster.
    pub agents: Vec<Box<dyn ThreadedAgent<MSG_BW, MessageType>>>,
    /// Cluster's context. cluster and local agent states live here.
    pub context: PlanetContext<MSG_BW, MessageType>,
    event_system: LocalEventSystem<CLOCK_BW, CLOCK_SCALES>,
    local_messages: LocalMailSystem<CLOCK_BW, CLOCK_SCALES, MessageType>,
    message_user: ThreadedMessengerUser<MSG_BW, Mail<MessageType>>,
    blocks: BlockSpoke<BLOCK_BW>,
    /// Current time information of the simulation. these should always be the same across simulation clusters in the same `Galaxy`.
    pub time: HTime,
    brakes: bool,
    wrapup: Subscriber<1, bool>,
    send_comms: Arc<BufferWheel<BLOCK_BW, isize>>,
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
        time: HTime,
        blocks: BlockSpoke<BLOCK_BW>,
        message_user: ThreadedMessengerUser<MSG_BW, Mail<MessageType>>,
        wrapup: Subscriber<1, bool>,
        wheel: Arc<BufferWheel<BLOCK_BW, isize>>,
        id: usize,
    ) -> Result<Self, AikaError> {
        let terminal = (time.terminal / time.timestep) as u64;
        Ok(Self {
            agents: Vec::new(),
            context: PlanetContext::new(32 * 1024, 64 * 1024, id, terminal),
            event_system: LocalEventSystem::new()?,
            local_messages: LocalMailSystem::new()?,
            message_user,
            blocks,
            time,
            brakes: false,
            wrapup,
            send_comms: wheel,
        })
    }

    fn commit(&mut self, event: Event) {
        self.event_system.insert(event)
    }

    fn commit_mail(&mut self, msg: Msg<MessageType>) {
        let msg = self.local_messages.schedule.insert(msg);
        if msg.is_err() {
            self.local_messages
                .overflow
                .push(Reverse(msg.err().unwrap()));
        }
    }

    /// Schedule an event for an agent at a given time.
    pub fn schedule(&mut self, time: u64, agent: usize) -> Result<(), AikaError> {
        if time < self.now() {
            return Err(AikaError::TimeTravel);
        } else if time as f64 * self.time.timestep > self.time.terminal {
            return Err(AikaError::PastTerminal);
        }
        let now = self.now();
        self.commit(Event::new(now, time, agent, SchedulingTask::Wait));
        Ok(())
    }

    /// Get the current time of the simulation.
    #[inline(always)]
    pub fn now(&self) -> u64 {
        self.event_system.local_clock.time
    }

    /// Spawn a new `ThreadedAgent` on this cluster. Specify the arena size for its state allocator.
    pub fn spawn_agent(
        &mut self,
        agent: Box<dyn ThreadedAgent<MSG_BW, MessageType>>,
        state_arena_size: usize,
    ) -> usize {
        self.agents.push(agent);
        self.context
            .agent_states
            .push(Journal::init(state_arena_size));
        self.agents.len() - 1
    }

    /// Spawn a preconfigured `ThreadedAgent`.
    pub fn spawn_agent_preconfigured(
        &mut self,
        agent: Box<dyn ThreadedAgent<MSG_BW, MessageType>>,
    ) -> usize {
        self.agents.push(agent);
        self.agents.len() - 1
    }

    // NEED TO REVIEW
    fn rollback(&mut self, time: u64) -> Result<(), AikaError> {
        let now = self.event_system.local_clock.time;
        if time > now {
            return Err(AikaError::TimeTravel);
        }
        // rollback world and agent states
        self.context.world_state.rollback(time);
        for i in &mut self.context.agent_states {
            i.rollback(time);
        }
        // rollback local message scheduler
        self.local_messages
            .schedule
            .rollback(&mut self.local_messages.overflow, time);
        // rollback and claim all the anti messages produced after the rollback time
        let anti_msgs: Vec<(Mail<MessageType>, u64)> = self.context.anti_msgs.rollback_return(time);

        // send out anti messages generated post rollback.
        for (anti, _) in anti_msgs {
            let anti_time = anti.transfer.commit_time();
            if let Some(to) = anti.to_world {
                if to == self.context.world_id {
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
        self.event_system
            .local_clock
            .rollback(&mut self.event_system.overflow, time);
        // reset context time
        self.context.time = time;

        println!(
            "Planet {:?}, Time {now}: ROLLBACK!!!!! rolling back to {time}",
            self.context.world_id
        );
        Ok(())
    }

    // NEED TO REVIEW
    fn annihilate(&mut self, anti_msg: AntiMsg) {
        let time = anti_msg.time();
        let idxs = self.local_messages.schedule.current_idxs;
        let diff = (time - self.local_messages.schedule.time) as usize;
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
                let msgs = &mut self.local_messages.schedule.wheels[k][offset];
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
                if to != self.context.world_id {
                    println!("!!! PANIC !!! mail planet to ID: {to}, context ID {:?}. source agent {:?} on planet {:?}", self.context.world_id, msg.transfer.from(), msg.from_world);
                    return Err(AikaError::MismatchedDeliveryAddress);
                }
            }
            let time = msg.transfer.time();
            // println!(
            //     "Planet {:?}: opening mail with recieve time {time}",
            //     self.context.world_id
            // );
            if time < self.now() {
                println!(
                    "Planet {:?}, Time {:?}: found old message in poll with recieve time {time}",
                    self.context.world_id,
                    self.now()
                );
                self.rollback(time)?;
            }

            match msg.open_letter() {
                Transfer::Msg(msg) => {
                    self.blocks.block.recv(
                        msg.commit_time(),
                        msg.commit_time() < self.blocks.block.start,
                    )?;
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
        self.event_system
            .local_clock
            .increment(&mut self.event_system.overflow);
        self.local_messages
            .schedule
            .increment(&mut self.local_messages.overflow);
        // check-process block now
        self.context.time += 1;
        let end = self.blocks.block.start + self.blocks.block.dur;
        if self.context.time > end {
            //println!(
            //    "Planet {:?}, Time {:?}: submitting local block #{:?} with end time {:?}",
            //    self.context.world_id, self.context.time, self.blocks.block.block_nmb, end
            //);
            let dur = self.blocks.block.max_dur;
            let mut new_id = self.blocks.block.block_id();
            new_id.1 += 1;

            self.blocks
                .submitter
                .write(std::mem::take(&mut self.blocks.block))?;
            println!(
                "Planet {:?} Time {:?}: submitted block number {:?}, new safe time {end}",
                self.context.world_id,
                self.context.time - 1,
                new_id.1 - 1
            );
            self.blocks.block.block_nmb = new_id.1;
            self.blocks.block.producer_id = new_id.0;
            self.blocks.block.start = self.context.time;
            self.blocks.block.dur = min(dur, (self.time.terminal / self.time.timestep) as u64);
            self.blocks.block.max_dur = dur;
        }
        Ok(())
    }

    // Check the synchronization of all clocks, and ensure GVT is acting as it should, and we are not at terminal time yet.
    fn check_time_validity(&self) -> Result<(), AikaError> {
        if self.context.time != self.local_messages.schedule.time
            || self.local_messages.schedule.time != self.event_system.local_clock.time
        {
            return Err(AikaError::ClockSyncIssue);
        }
        if self.time.gvt > self.context.time {
            return Err(AikaError::GVTPastLocalClock(
                self.context.world_id,
                self.context.time,
                self.time.gvt,
            ));
        }
        if self.time.gvt as f64 * self.time.timestep < self.time.terminal
            && self.context.time as f64 * self.time.timestep > self.time.terminal
        {
            return Err(AikaError::PastTerminalButGVTBehind);
        }
        if self.time.gvt as f64 * self.time.timestep >= self.time.terminal
            && self.context.time as f64 * self.time.timestep > self.time.terminal
        {
            return Err(AikaError::PastTerminal);
        }
        Ok(())
    }

    // Take one step in local cluster time.
    pub(crate) fn step(&mut self) -> Result<(), AikaError> {
        println!(
            "Planet {:?}: step starting at time {:?}",
            self.context.world_id,
            self.now()
        );
        if let Ok(msgs) = self.local_messages.schedule.tick() {
            for msg in msgs {
                let id = msg.to;
                if id.is_none() {
                    for i in 0..self.agents.len() {
                        self.agents[i].read_message(&mut self.context, msg, i);
                    }
                    continue;
                }
                let id = id.unwrap();
                self.agents[id].read_message(&mut self.context, msg, id);
            }
        }
        // process events at the next time step
        if let Ok(events) = self.event_system.local_clock.tick() {
            for event in events {
                let event = self.agents[event.agent].step(&mut self.context, event.agent)?;
                match event.yield_ {
                    SchedulingTask::Timeout(time) => {
                        if (self.now() + time) as f64 * self.time.timestep > self.time.terminal {
                            continue;
                        }

                        self.commit(Event::new(
                            self.now(),
                            self.now() + time,
                            event.agent,
                            SchedulingTask::Wait,
                        ));
                    }
                    SchedulingTask::Schedule(time) => {
                        self.commit(Event::new(
                            self.now(),
                            time,
                            event.agent,
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
            if Some(self.context.world_id) == mail.to_world {
                match mail.open_letter() {
                    Transfer::Msg(msg) => self.commit_mail(msg),
                    Transfer::AntiMsg(anti_msg) => self.annihilate(anti_msg),
                }
            } else {
                self.message_user.send(mail)?;
            }
            if now < self.blocks.block.start {
                let blocks_past =
                    ((self.blocks.block.start - now - 1) / self.blocks.block.max_dur) as usize;
                if blocks_past >= BLOCK_BW {
                    return Err(AikaError::MesoError(MesoError::DistantBlocks(blocks_past)));
                }
                self.blocks.block.delayed_corrections[blocks_past - 1] += 1;
                continue;
            }
            self.blocks.block.sends += 1;
        }

        // increment the clock to the next step
        self.increment()?;
        Ok(())
    }

    /// Run the local cluster. Master loop polls the inter-cluster messenger, checks for GVT updates, then checks its time
    /// validity to proceed. If all is safe to proceed, step the simulation one time step, and check if we now meet the
    /// termination requirements. If not, yield the thread and repeat.
    pub fn run(mut self) -> Result<Self, AikaError> {
        if self.time.terminal == f64::MAX {
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
                //println!("Planet {:?}: new GVT found: {gvt}", self.context.world_id);
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
            // if at a checkpoint or the throttle limit, busy-wait the thread
            if self.time.cp_hz != u64::MAX
                && now
                    == (self.time.cp_hz
                        * self.blocks.block.max_dur
                        * self.blocks.block.block_nmb as u64)
                && now != (self.time.terminal / self.time.timestep) as u64
                && self.time.gvt != now
            {
                //println!("Planet {:?}: checkpoint sleeping", self.context.world_id);
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
    pub(crate) fn run_debug<T: Pod + Zeroable + std::fmt::Debug>(
        &mut self,
    ) -> Result<(), AikaError> {
        if self.time.terminal == f64::MAX {
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
                //println!("Planet {:?}: new GVT found: {gvt}", self.context.world_id);
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
                        if counter < 10 {
                            println!(
                                "Planet {:?}: waiting for GVT to catch up, continuing to poll.",
                                self.context.world_id
                            );
                        } else if counter == 10 {
                            println!(
                                "Planet {:?}: waiting too long for GVT, going quiet.",
                                self.context.world_id
                            );
                        }
                        counter += 1;
                        sleep(Duration::from_nanos(100));
                        std::thread::yield_now();
                        continue;
                    }
                    AikaError::PastTerminal => break,
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
                && now != (self.time.terminal / self.time.timestep) as u64
                && self.time.gvt != now
            {
                //println!("Planet {:?}: checkpoint sleeping", self.context.world_id);
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
        let out = self.context.world_state.read_all::<T>();
        println!(
            "Planet {:?}: Terminated with local clock {:?}.",
            self.context.world_id,
            self.now()
        );
        println!(
            "Planet {:?}: final global simulation state: {:?}",
            self.context.world_id, out
        );
        Ok(())
    }
}

#[cfg(test)]
mod unit_tests {
    use std::thread;

    use super::*;
    use crate::mt::agents::{PlanetContext, ThreadedAgent};
    use crate::objects::{Event, Msg, SchedulingTask};
    use bytemuck::{Pod, Zeroable};

    const AGENTS: usize = 10;
    const BLOCK_BANDWIDTH: usize = 64;
    const MSG_BANDWIDTH: usize = 8;

    #[derive(Copy, Clone, Debug)]
    #[repr(C)]
    struct TestMessage;

    unsafe impl Pod for TestMessage {}
    unsafe impl Zeroable for TestMessage {}

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

    impl ThreadedAgent<MSG_BANDWIDTH, TestMessage> for TestAgent {
        fn step(
            &mut self,
            context: &mut PlanetContext<MSG_BANDWIDTH, TestMessage>,
            agent_id: usize,
        ) -> Result<Event, AikaError> {
            match context.world_state.read_state::<usize>() {
                Ok(state) => {
                    context.world_state.write(state + 1, context.time, None);
                }
                Err(err) => {
                    if let MesoError::UninitializedState = err {
                        context.world_state.write(1usize, context.time, None);
                    }
                }
            }
            Ok(Event::new(
                context.time,
                context.time + 1,
                agent_id,
                SchedulingTask::Timeout(1),
            ))
        }

        fn read_message(
            &mut self,
            _context: &mut PlanetContext<MSG_BANDWIDTH, TestMessage>,
            _msg: Msg<TestMessage>,
            _agent_id: usize,
        ) {
        }
    }

    #[derive(Copy, Clone, Debug)]
    #[repr(C)]
    struct MsgAgent;

    unsafe impl Send for MsgAgent {}
    unsafe impl Sync for MsgAgent {}

    impl ThreadedAgent<MSG_BANDWIDTH, TestMessage> for MsgAgent {
        fn step(
            &mut self,
            context: &mut PlanetContext<MSG_BANDWIDTH, TestMessage>,
            agent_id: usize,
        ) -> Result<Event, AikaError> {
            let id = agent_id;
            let time = context.time;
            context.send_mail(
                Msg::new(TestMessage, time, time + 1, id, Some((id + 1) % AGENTS)),
                context.world_id,
            )?;
            Ok(Event::new(time, time, id, SchedulingTask::Wait))
        }

        fn read_message(
            &mut self,
            context: &mut PlanetContext<MSG_BANDWIDTH, TestMessage>,
            msg: Msg<TestMessage>,
            agent_id: usize,
        ) {
            assert_eq!(context.time, msg.recv);
        }
    }

    fn create_setup<
        const CLOCK_BW: usize,
        const CLOCK_SCALES: usize,
        MessageType: Pod + Zeroable + Clone,
    >(
        terminal: f64,
        block_dur: u64,
    ) -> Result<
        (
            Galaxy<BLOCK_BANDWIDTH, MSG_BANDWIDTH, MessageType>,
            Planet<BLOCK_BANDWIDTH, MSG_BANDWIDTH, CLOCK_BW, CLOCK_SCALES, MessageType>,
        ),
        AikaError,
    >
    where
        TestAgent: ThreadedAgent<MSG_BANDWIDTH, MessageType>,
    {
        let mut galaxy = Galaxy::<BLOCK_BANDWIDTH, MSG_BANDWIDTH, MessageType>::new(6, 12)?;
        galaxy.set_time_scale(1.0, terminal);
        galaxy.with_block_duration(block_dur);
        let mut planet = galaxy.spawn_planet::<CLOCK_BW, CLOCK_SCALES>()?;
        for j in 0..AGENTS {
            let agent = TestAgent::new(j);
            planet.spawn_agent(Box::new(agent), 0);
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
    fn test_simple_setup() {
        let (galaxy, mut planets) = create_setup::<64, 2, TestMessage>(1.0, 1).unwrap();
        schedule_all(&mut planets, 0).unwrap();
        let ghandle = thread::spawn(move || galaxy.master());
        let phandle = thread::spawn(move || planets.run_debug::<usize>());

        let galaxy_result = ghandle.join().expect("Galaxy thread panicked");
        let planet_result = phandle.join().expect("Planet thread panicked");

        // Ensure both threads completed successfully
        assert!(galaxy_result.is_ok());
        assert!(
            planet_result.is_ok(),
            "Planet run loop failed: {:?}",
            planet_result
        );
    }

    #[test]
    fn test_multiplanet_setup() {
        let (mut galaxy, mut planet) = create_setup::<64, 2, TestMessage>(1.0, 1).unwrap();
        schedule_all(&mut planet, 0).unwrap();
        let mut planets = vec![planet];
        for _ in 0..5 {
            let mut planet = galaxy.spawn_planet::<64, 2>().unwrap();
            for j in 0..AGENTS {
                let agent = TestAgent::new(j);
                planet.spawn_agent(Box::new(agent), 0);
            }
            planets.push(planet);
        }
        let ghandle = thread::spawn(move || galaxy.master());

        let phandles = planets
            .into_iter()
            .map(|mut x| {
                thread::spawn(move || {
                    for i in 0..AGENTS {
                        x.schedule(1, i)?;
                    }
                    x.run_debug::<usize>()
                })
            })
            .collect::<Vec<_>>();

        let galaxy_result = ghandle.join().expect("Galaxy thread panicked");
        let planet_result = phandles
            .into_iter()
            .all(|x| x.join().expect("Planet thread panicked").is_ok());

        // Ensure both threads completed successfully
        assert!(galaxy_result.is_ok(), "Galaxy master loop failed");
        assert!(planet_result, "Planet run loop failed: {:?}", planet_result);
    }

    #[test]
    fn test_rollback_accounting() {
        let mut galaxy: Galaxy<8, 8, TestMessage> = Galaxy::new(1, 1).unwrap();
        let mut planet = galaxy.spawn_planet::<16, 2>().unwrap();
        for i in 0..AGENTS {
            planet.spawn_agent(Box::new(TestAgent::new(i)), 0);
            planet.schedule(0, i).unwrap();
        }
        planet.step().unwrap();
        println!("state {:?}", planet.context.world_state.read_all::<usize>());
        planet.step().unwrap();
        //println!("state {:?}", planet.context.world_state.read_state::<usize>().unwrap());
        planet.step().unwrap();
        //println!("state {:?}", planet.context.world_state.read_state::<usize>().unwrap());
        assert_eq!(planet.now(), 3);

        let current = planet.context.world_state.read_state::<usize>().unwrap();
        assert_eq!(*current, 30);
        let res = planet.rollback(1);
        assert!(res.is_ok());
        assert_eq!(planet.now(), 1);
        assert_eq!(planet.context.time, 1);
        let current = planet.context.world_state.read_state::<usize>().unwrap();
        assert_eq!(*current, 10);

        let res = planet.rollback(2);
        assert!(res.is_err());
        match res.err().unwrap() {
            AikaError::TimeTravel => (),
            _ => panic!("Expected TimeTravel error"),
        }

        let (_, mut planets) = create_setup::<64, 2, TestMessage>(50.0, 1).unwrap();
        planets.spawn_agent(Box::new(TestAgent::new(0)), 0);
        planets.spawn_agent(Box::new(TestAgent::new(1)), 0);
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
        // clock rollback deletes anything with commit after rollback time by nature so this just makes sure were disposing of the antimessages properly
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
