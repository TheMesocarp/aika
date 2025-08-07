use std::{cmp::{max, min, Reverse}, collections::{BTreeSet, BinaryHeap}, fs::File, io::Write, time::Instant};

use bytemuck::{Pod, Zeroable};
use mesocarp::{comms::mailbox::{Message, ThreadedMessengerUser}, scheduling::Scheduleable};

use crate::{actors::{ConnectedActor, Context}, env::Environment, mt::{consensus::{Block, BlockSpoke}, engines::HTime}, objects::{AntiMsg, Event, LocalScheduler, Mail, Msg, SchedulingTask, Transfer}, AikaError};


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
    pub(crate) message_user: ThreadedMessengerUser<MSG_BW, Mail<MessageType>>,
    pub(crate) blocks: BlockSpoke<BLOCK_BW>,
    /// Current time information of the simulation. these should always be the same across simulation clusters in the same `Galaxy`.
    pub time: HTime,
    // checkpoint management
    last_cp_block_nmb: (usize, usize),
    cp_counter: u64,
    brakes: bool,
    // debug support
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
            last_cp_block_nmb: (id, 0),
            cp_counter: 1,
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
        let now = self.now();
        if let Some(file) = &mut self.log {
            writeln!(
                file,
                "[{}] Scheduling a step at time {time}, current now() time {now}, scheduler times: {:?}",
                self.start.elapsed().as_micros(),
                self.local_messages.clock.time
            )
            .map_err(|_| AikaError::LoggingWriteError)?;
        }
        if time < self.event_system.clock.time {
            return Err(AikaError::TimeTravel);
        } else if time > self.time.terminal {
            return Err(AikaError::PastTerminal);
        }
        if actor >= self.actors.len() {
            return Err(AikaError::InvalidActorId(self.actors.len(), self.context.cluster_id, actor))
        }
        let now = self.now();
        self.commit(Event::new(now, time, actor, SchedulingTask::Wait));
        Ok(())
    }

    /// Get the current time of the simulation
    pub fn now(&self) -> u64 {
        max(self.event_system.clock.time, self.context.time)
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
                    return Err(AikaError::MismatchedDeliveryAddress(self.context.cluster_id, to));
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
                    self.blocks.block.recv(msg.commit_time(), self.time.terminal)?;
                    self.commit_mail(msg)
                }
                Transfer::AntiMsg(anti_msg) => {
                    self.blocks.block.recv_anti(anti_msg.commit_time(), self.time.terminal)?;
                    self.annihilate(anti_msg)
                }
            }
        }
        Ok(())
    }

    // Increment the local clock one step and submit the current block if there is a turnover.
    fn increment(&mut self) -> Result<(), AikaError> {
        if !self.blocks.block.catchup_block || (self.blocks.block.catchup_block && (self.now() < self.blocks.block.start)) {
            self.event_system.increment();
            self.local_messages.increment();
        }
        // check-process block now
        self.context.time += 1;
        let end = self.blocks.block.start + self.blocks.block.dur;
        if let Some(file) = &mut self.log {
            writeln!(
                file,
                "[{}] Local virtual time {:?}: incremented. end time of current block {end}",
                self.start.elapsed().as_micros(),
                self.context.time,
            ).map_err(|_| AikaError::LoggingWriteError)?;
        }
        if self.context.time == end {
            // log current block data to initialize next block
            let dur = self.blocks.block.max_dur;
            let catch_up = self.blocks.block.catchup_block;
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
            // submit the current block than initialize the new one.
            self.blocks
                .submitter
                .write(std::mem::take(&mut self.blocks.block))?;
            self.blocks.block.block_nmb = new_id.1;
            self.blocks.block.producer_id = new_id.0;
            self.blocks.block.start = self.context.time;
            if self.now() < self.time.terminal {
                let diff = self.time.terminal - self.now();
                self.blocks.block.dur = min(dur, diff);
            } else {
                self.blocks.block.dur = dur;
            }
            self.blocks.block.max_dur = dur;
            self.blocks.block.catchup_block = catch_up;

            if let Some(file) = &mut self.log {
                writeln!(
                    file,
                    "[{}] next checkpoint: {:?}",
                    self.start.elapsed().as_micros(),
                    self.time.cp_hz * self.blocks.block.max_dur * self.cp_counter
                )
                .map_err(|_| AikaError::LoggingWriteError)?;
            }

            if (self.time.cp_hz != u64::MAX
                && self.context.time
                    == (self.time.cp_hz
                        * self.blocks.block.max_dur
                        * self.cp_counter)) || self.now() >= self.time.terminal
            { 
                if let Some(file) = &mut self.log {
                    writeln!(
                        file,
                        "[{}] Cluster has reached a checkpoint, or past terminal time locally, awaiting GVT before any more events or messages can process.",
                        self.start.elapsed().as_micros(),
                    )
                    .map_err(|_| AikaError::LoggingWriteError)?;
                }
                self.blocks.block.catchup_block = true;
                self.last_cp_block_nmb = self.blocks.block.block_id();
            }
        }
        Ok(())
    }

    // Check the synchronization of all clocks, and ensure GVT is acting as it should, and we are not at terminal time yet.
    fn check_time_validity(&self) -> Result<(), AikaError> {
        if self.time.gvt > self.context.time {
            return Err(AikaError::GVTPastLocalClock(
                self.context.cluster_id,
                self.context.time,
                self.time.gvt,
            ));
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
                "[{}] step starting at now() time {:?}, scheduler times: {:?}",
                self.start.elapsed().as_micros(),
                now,
                self.local_messages.clock.time
            )
            .map_err(|_| AikaError::LoggingWriteError)?;
        }
        if !self.blocks.block.catchup_block || (self.blocks.block.catchup_block && (self.now() < self.blocks.block.start)) {
            if let Some(file) = &mut self.log {
                writeln!(
                    file,
                    "[{}] meeting step condition for messages and events.",
                    self.start.elapsed().as_micros(),
                )
                .map_err(|_| AikaError::LoggingWriteError)?;
            }
            if let Ok(msgs) = self.local_messages.clock.tick() {
                let len = msgs.len();
                if !msgs.is_empty() {
                    if let Some(file) = &mut self.log {
                        writeln!(
                            file,
                            "[{}] Found {len} messages to process.",
                            self.start.elapsed().as_micros(),
                        )
                        .map_err(|_| AikaError::LoggingWriteError)?;
                    }
                }
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
                let len = events.len();
                if !events.is_empty() {
                    if let Some(file) = &mut self.log {
                        writeln!(
                            file,
                            "[{}] Found {len} events to process.",
                            self.start.elapsed().as_micros(),
                        )
                        .map_err(|_| AikaError::LoggingWriteError)?;
                    }
                }
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
        }
        // collect and send all the sends gathered this time step
        let now = self.now();
        let sends = std::mem::take(&mut self.context.outbox);
        if !self.blocks.block.catchup_block || (self.blocks.block.catchup_block && (self.now() < self.blocks.block.start)) {
            for mail in sends {
                let mut local = false;
                if Some(self.context.cluster_id) == mail.to_world {
                    match mail.open_letter() {
                        Transfer::Msg(msg) => self.commit_mail(msg),
                        Transfer::AntiMsg(anti_msg) => self.annihilate(anti_msg),
                    }
                    local = true;
                } else {
                    if let Some(file) = &mut self.log {
                        writeln!(
                            file,
                            "[{}] sending message to cluster {:?}.",
                            self.start.elapsed().as_micros(),
                            mail.to_world
                        )
                        .map_err(|_| AikaError::LoggingWriteError)?;
                    }
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
            if let Some(gvt) = self.blocks.subscriber.try_recv() {
                if gvt < self.time.gvt {
                    return Err(AikaError::GVTisDecreasing);
                }
                self.time.gvt = gvt;
            }
            if self.time.cp_hz != u64::MAX
                && self.time.gvt
                    == (self.time.cp_hz
                        * self.blocks.block.max_dur
                        * self.cp_counter)
                && self.time.gvt != 0
                && self.context.time < self.time.terminal
                && self.time.gvt != self.context.time
            {   
                self.cp_counter += 1;
                self.context.time = self.local_messages.clock.time;
                let start = self.time.gvt;
                let dur = self.blocks.block.max_dur;
                let nmb = self.last_cp_block_nmb;

                self.blocks.block = Block::new(start, dur, nmb.1, nmb.0, false);

                let diff = self.time.terminal - self.now();
                self.blocks.block.dur = min(dur, diff);
            }
            for _ in 0..8 {
                self.poll_interplanetary_messenger()?;
            }
            // make sure time is valid to proceed.
            match self.check_time_validity() {
                Ok(_) => {}
                Err(err) => match err {
                    AikaError::PastTerminal => {
                        self.rollback(self.time.terminal + 1)?;
                        break
                    },
                    _ => return Err(err),
                },
            }
            // if at a checkpoint limit, busy-wait the thread until the GVT catches up
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
        loop {
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

            // if let Some(file) = &mut self.log {
            //     writeln!(
            //         file,
            //         "[{}] GVT caught up to checkpoint, rolling back to GVT and allowing messages and events to continue processing.",
            //         self.start.elapsed().as_micros(),
            //     )
            //     .map_err(|_| AikaError::LoggingWriteError)?;
            // }

            if self.time.cp_hz != u64::MAX
                && self.time.gvt
                    == (self.time.cp_hz
                        * self.blocks.block.max_dur
                        * self.cp_counter)
                && self.time.gvt != 0
                && self.context.time < self.time.terminal
                && self.time.gvt != self.context.time
            {   
                if let Some(file) = &mut self.log {
                    writeln!(
                        file,
                        "[{}] GVT caught up to checkpoint, rolling back to GVT and allowing messages and events to continue processing.",
                        self.start.elapsed().as_micros(),
                    )
                    .map_err(|_| AikaError::LoggingWriteError)?;
                }
                self.cp_counter += 1;
                self.context.time = self.local_messages.clock.time;
                let start = self.time.gvt;
                let dur = self.blocks.block.max_dur;
                let nmb = self.last_cp_block_nmb;

                self.blocks.block = Block::new(start, dur, nmb.1, nmb.0, false);

                let diff = self.time.terminal - self.now();
                self.blocks.block.dur = min(dur, diff);
            } 
            for _ in 0..8 {
                self.poll_interplanetary_messenger()?;
            }
            // make sure time is valid to proceed.
            match self.check_time_validity() {
                Ok(_) => {}
                Err(err) => match err {
                    AikaError::PastTerminal => {
                        writeln!(
                            self.log.as_mut().unwrap(),
                            "[{}]: Past terminal time detected, breaking.",
                            self.start.elapsed().as_micros(),
                        )
                        .map_err(|_| AikaError::LoggingWriteError)?;
                        self.rollback(self.time.terminal + 1)?;
                        break;
                    }
                    _ => return Err(err),
                },
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

#[cfg(test)]
mod planet_tests {
    use super::*;
    use crate::actors::{Actor, ConnectedActor, Context};
    use crate::env::Stateless;
    use crate::mt::engines::hlocal::Galaxy;
    use crate::objects::{Event, Msg, SchedulingTask};
    
    #[derive(Debug, Copy, Clone)]
    #[repr(C)]
    struct TestMsg;
    unsafe impl Pod for TestMsg {}
    unsafe impl Zeroable for TestMsg {}
    
    #[derive(Debug)]
    struct TestActor {
        counter: usize,
    }
    
    impl Actor<TestMsg> for TestActor {
        fn step(&mut self, ctx: &mut Context<TestMsg>, id: usize) -> Result<Event, AikaError> {
            self.counter += 1;
            Ok(Event::new(ctx.time, ctx.time + 1, id, SchedulingTask::Timeout(1)))
        }
    }
    
    impl ConnectedActor<TestMsg> for TestActor {
        fn read_message(&mut self, _: &mut Context<TestMsg>, _: Msg<TestMsg>, _: usize) -> Result<(), AikaError> {
            self.counter += 1;
            Ok(())
        }
    }

    fn create_test_planet() -> Planet<8, 16, 32, 2, TestMsg> {
        let mut galaxy: Galaxy<8, 16, TestMsg> = Galaxy::new(1, 64).unwrap();
        galaxy.set_time_scale(1000);
        galaxy.with_block_duration(10);
        galaxy.spawn_planet(Stateless).unwrap()
    }

    #[test]
    fn test_planet_actor_spawning() {
        let mut planet = create_test_planet();
        
        assert_eq!(planet.actors.len(), 0);
        
        planet.spawn_actor(TestActor { counter: 0 });
        assert_eq!(planet.actors.len(), 1);
        
        planet.spawn_actor(TestActor { counter: 0 });
        assert_eq!(planet.actors.len(), 2);
    }

    #[test]
    fn test_planet_scheduling() {
        let mut planet = create_test_planet();
        planet.spawn_actor(TestActor { counter: 0 });

        assert!(planet.schedule(10, 0).is_ok());

        planet.context.time = 20;
        planet.local_messages.clock.time = 20;
        planet.event_system.clock.time = 20;

        assert!(matches!(planet.schedule(5, 0), Err(AikaError::TimeTravel)));
        assert!(matches!(planet.schedule(2000, 0), Err(AikaError::PastTerminal)));
        assert!(matches!(planet.schedule(50, 5), Err(AikaError::InvalidActorId(_, _, _))));
    }

    #[test]
    fn test_planet_rollback() {
        let mut planet = create_test_planet();
        planet.spawn_actor(TestActor { counter: 0 });
        
        planet.context.time = 50;
        planet.event_system.clock.time = 50;
        
        for i in 0..10 {
            let msg = Msg::new(TestMsg, i * 5, (i + 1) * 5 + 45, 0, Some(0));
            planet.commit_mail(msg);
        }
        
        assert!(planet.rollback(25).is_ok());
        assert_eq!(planet.context.time, 25);
        assert_eq!(planet.now(), 25);
        
        assert!(matches!(planet.rollback(30), Err(AikaError::TimeTravel)));
    }

    #[test]
    fn test_planet_step() {
        let mut planet = create_test_planet();
        planet.spawn_actor(TestActor { counter: 0 });
        planet.schedule(1, 0).unwrap();

        assert!(planet.step().is_ok());
        assert_eq!(planet.context.time, 1);

        let msg = Msg::new(TestMsg, 0, 2, 0, Some(0));
        planet.commit_mail(msg);
        assert!(planet.step().is_ok());
    }

    #[test]
    fn test_planet_interplanetary_message_polling() {
        let mut planet = create_test_planet();
        planet.spawn_actor(TestActor { counter: 0 });
        
        let msg = Mail::write_letter(
            Transfer::Msg(Msg::new(TestMsg, 5, 10, 0, Some(0))),
            0,
            Some(0)
        );
        planet.message_user.send(msg).unwrap();
        
        assert!(planet.poll_interplanetary_messenger().is_ok());
    }

    #[test]
    fn test_planet_block_submission() {
        let mut planet = create_test_planet();
        
        assert_eq!(planet.blocks.block.start, 0);
        assert_eq!(planet.blocks.block.dur, 10);
        
        for _ in 0..10 {
            planet.increment().unwrap();
        }
        
        assert_eq!(planet.blocks.block.start, 10);
        assert_eq!(planet.blocks.block.block_nmb, 1);
    }

    #[test]
    fn test_planet_checkpoint_handling() {
        let mut galaxy: Galaxy<8, 16, TestMsg> = Galaxy::new(1, 64).unwrap();
        galaxy.set_time_scale(100);
        galaxy.with_block_duration(10);
        galaxy.checkpoints(2);
        
        let mut planet = galaxy.spawn_planet::<32, 2>(Stateless).unwrap();
        planet.spawn_actor(TestActor { counter: 0 });
        
        for _ in 0..20 {
            planet.increment().unwrap();
        }
        
        assert!(planet.blocks.block.catchup_block);
    }

    #[test]
    fn test_planet_gvt_validation() {
        let mut planet = create_test_planet();

        planet.time.gvt = 10;
        planet.context.time = 20;
        assert!(planet.check_time_validity().is_ok());

        planet.time.gvt = 30;
        planet.context.time = 20;
        assert!(matches!(
            planet.check_time_validity(), 
            Err(AikaError::GVTPastLocalClock(_, _, _))
        ));

        planet.time.gvt = 1001;
        planet.context.time = 1002;
        assert!(matches!(
            planet.check_time_validity(),
            Err(AikaError::PastTerminal)
        ));
    }

    #[test]
    fn test_planet_message_routing() {
        let mut planet = create_test_planet();
        planet.spawn_actor(TestActor { counter: 0 });
        planet.spawn_actor(TestActor { counter: 0 });

        let local_msg = Msg::new(TestMsg, 5, 10, 0, Some(1));
        planet.context.send_mail(local_msg, 0).unwrap();

        assert_eq!(planet.context.outbox.len(), 1);

        planet.step().unwrap();

        let remote_msg = Msg::new(TestMsg, 5, 10, 0, Some(0));
        planet.context.send_mail(remote_msg, 1).unwrap();

        assert_eq!(planet.context.outbox.len(), 1);
    }
}