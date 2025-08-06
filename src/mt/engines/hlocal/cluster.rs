use std::{cmp::{min, Reverse}, io::Write, collections::{BTreeSet, BinaryHeap}, fs::File, thread::sleep, time::{Duration, Instant}};

use bytemuck::{Pod, Zeroable};
use mesocarp::{comms::mailbox::{Message, ThreadedMessengerUser}, scheduling::Scheduleable};

use crate::{actors::{ConnectedActor, Context}, env::Environment, mt::{consensus::BlockSpoke, engines::HTime}, objects::{AntiMsg, Event, LocalScheduler, Mail, Msg, SchedulingTask, Transfer}, AikaError};


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

    pub(crate) fn commit_mail(&mut self, msg: Msg<MessageType>) {
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
    pub fn set_log(&mut self, file: File) {
        self.log = Some(file);
    }

    pub(crate) fn rollback(&mut self, time: u64) -> Result<(), AikaError> {
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
                    return Err(AikaError::DistantBlocks(blocks_past));
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
                    return Err(AikaError::DistantBlocks(blocks_past));
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
                        std::thread::sleep(Duration::from_nanos(100));
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
