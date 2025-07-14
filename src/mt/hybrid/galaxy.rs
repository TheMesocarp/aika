use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use mesocarp::{
    comms::{
        mailbox::{ThreadedMessenger, ThreadedMessengerUser},
        spmc::Subscriber,
        spsc::BufferWheel,
    },
    logging::journal::Journal,
    MesoError,
};

use crate::{
    mt::hybrid::blocks::{Block, GVTComms},
    objects::Mail,
    AikaError,
};

pub struct PlanetaryRegister<
    const MSG_SLOTS: usize,
    const BLOCK_SLOTS: usize,
    const GVT_SLOTS: usize,
    MessageType: Pod + Zeroable + Clone,
> {
    pub planet_id: usize,
    pub messenger_account: ThreadedMessengerUser<MSG_SLOTS, Mail<MessageType>>,
    pub block_channel: Arc<BufferWheel<BLOCK_SLOTS, Block<BLOCK_SLOTS>>>,
    pub gvt_subscriber: Subscriber<GVT_SLOTS, u64>,
    pub terminal: f64,
    pub timestep: f64,
    pub throttle: u64,
    pub checkpoint_hz: u64,
    pub block_size: u64,
}

pub struct Galaxy<
    const MSG_SLOTS: usize,
    const BLOCK_SLOTS: usize,
    const GVT_SLOTS: usize,
    MessageType: Pod + Zeroable + Clone,
> {
    // block things
    pub block_counter: usize,
    pub block_size: u64,
    pub blocks: Journal,
    pub next: Vec<Option<Block<BLOCK_SLOTS>>>,
    pub pending: Vec<[Option<Block<BLOCK_SLOTS>>; BLOCK_SLOTS]>,
    pub unmatched_sends: usize,
    pub(crate) gvtcomms: GVTComms<BLOCK_SLOTS, GVT_SLOTS>,
    // messenger
    pub messenger: ThreadedMessenger<MSG_SLOTS, Mail<MessageType>>,
    // time things
    pub gvt: u64,
    pub checkpoint_hz: u64,
    pub throttle: u64,
    pub terminal: f64,
    pub timestep: f64,
    // validation things
    pub registered: usize,
    pub planet_count: usize,
}

impl<
        const MSG_SLOTS: usize,
        const BLOCK_SLOTS: usize,
        const GVT_SLOTS: usize,
        MessageType: Pod + Zeroable + Clone,
    > Galaxy<MSG_SLOTS, BLOCK_SLOTS, GVT_SLOTS, MessageType>
{
    pub fn create(planet_count: usize) -> Result<Self, AikaError> {
        let blocks = Journal::init(64 * 1024);
        let gvtcomms = GVTComms::new()?;

        let mut planet_ids = Vec::new();
        for i in 0..planet_count {
            planet_ids.push(i);
        }
        let messenger = ThreadedMessenger::new(planet_ids)?;
        let pending: Vec<[Option<Block<BLOCK_SLOTS>>; BLOCK_SLOTS]> =
            vec![[Option::<Block<BLOCK_SLOTS>>::None; BLOCK_SLOTS]; planet_count];
        Ok(Self {
            block_counter: 0,
            block_size: 16,
            blocks,
            next: vec![Option::<Block<BLOCK_SLOTS>>::None; planet_count],
            pending,
            unmatched_sends: 0,
            gvtcomms,
            messenger,
            gvt: 0,
            checkpoint_hz: u64::MAX,
            throttle: u64::MAX,
            terminal: f64::MAX,
            timestep: 1.0,
            registered: 0,
            planet_count,
        })
    }

    pub fn set_time_scale(&mut self, timestep: f64, terminal: f64) {
        self.terminal = terminal;
        self.timestep = timestep
    }

    pub fn throttle(&mut self, throttle: u64) {
        self.throttle = throttle
    }

    pub fn checkpoints(&mut self, frequency: u64) {
        self.checkpoint_hz = frequency
    }

    pub fn spawn_planet(
        &mut self,
    ) -> Result<PlanetaryRegister<MSG_SLOTS, BLOCK_SLOTS, GVT_SLOTS, MessageType>, AikaError> {
        if self.registered == self.planet_count {
            return Err(AikaError::MaximumAgentsAllowed);
        }
        let id = self.registered;
        self.registered += 1;
        let messenger_account = self.messenger.get_user(id)?;
        let (block_channel, gvt_subscriber) = self.gvtcomms.register();
        Ok(PlanetaryRegister {
            planet_id: id,
            messenger_account,
            block_channel,
            gvt_subscriber,
            terminal: self.terminal,
            timestep: self.timestep,
            throttle: self.throttle,
            checkpoint_hz: self.checkpoint_hz,
            block_size: self.block_size,
        })
    }

    fn deliver_the_mail(&mut self) -> Result<(), AikaError> {
        match self.messenger.poll() {
            Ok(msgs) => {
                self.messenger.deliver(msgs)?;
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

    fn poll_blocks(&mut self) -> Result<(), AikaError> {
        let blocks = self.gvtcomms.poll()?;
        for (i, planet_blocks) in blocks.into_iter().enumerate() {
            if let Some(pblocks) = planet_blocks {
                for block in pblocks {
                    println!("GVT Master: received block from planet #{i}, with start time {:?}", block.start);
                    if block.start == self.gvt + 1 {
                        println!("GVT Master: placing block in next slot");
                        self.next[i] = Some(block);
                        continue;
                    }
                    let diff = ((block.start - self.gvt) / self.block_size) as usize - 1;
                    if diff >= BLOCK_SLOTS {
                        return Err(AikaError::DistantBlocks(diff));
                    }
                    println!("GVT Master: placing block in pending slot number {diff}");
                    self.pending[i][diff] = Some(block);
                }
            }
        }
        Ok(())
    }

    fn fetch_latest_uncommited_blocks(
        &mut self,
    ) -> Result<Vec<Option<Block<BLOCK_SLOTS>>>, AikaError> {
        let mut latests = Vec::new();
        for i in &self.next {
            latests.push(i.clone());
        }

        for (idx, row) in self.pending.iter().enumerate() {
            for i in row {
                if let Some(block) = i {
                    let cloned = Some(block.clone());
                    latests[idx] = cloned;
                }
            }
        }
        Ok(latests)
    }

    fn update_consensus(&mut self) -> Result<Option<u64>, AikaError> {
        if self.next.iter().all(|x| x.is_some()) {
            let mut sends = 0;
            let mut recvs = 0;
            let mut start = 0;
            let mut end = 0;
            let mut recvs_from_previous = [0usize; BLOCK_SLOTS];

            // fetch next block stats
            for i in &mut self.next {
                if let Some(block) = i {
                    sends += block.sends;
                    recvs += block.recvs;
                    if start == end && end == 0 {
                        end = block.end;
                        start = block.start;
                    }
                    if end != block.end && start != block.start {
                        return Err(AikaError::MismatchBlockTimeStamps(self.block_counter, block.start, block.end));
                    }
                    recvs_from_previous
                        .iter_mut()
                        .zip(block.recvs_from_previous.iter())
                        .for_each(|(x, y)| *x += *y);
                }
            }

            let unmatched = sends - recvs;
            println!("GVT Master: block number {:?}, found with {unmatched} unmatched messages.", self.block_counter);
            self.unmatched_sends = unmatched;
            // if all messages are accounted for locally, and we are indeed looking at the next block, commit and move on
            if unmatched == 0 && end > self.gvt {
                self.commit_block(start, end, sends, recvs, recvs_from_previous)?;
                return Ok(Some(end));
            }
            // check blocks after "self.next" for receives from this block.
            let mut late_recvs = 0;
            for row in self.pending.iter() {
                for (i, maybe) in row.iter().enumerate() {
                    match maybe {
                        Some(block) => {
                            late_recvs += block.recvs_from_previous[i];
                        }
                        None => break,
                    }
                }
            }

            // if all messages are eventually accounted for, no more rollbacks can happen into this block and its safe to commit
            if unmatched - late_recvs == 0 {
                self.commit_block(start, end, sends, recvs, recvs_from_previous)?;
                return Ok(Some(end));
            }
        }
        Ok(None)
    }

    fn commit_block(
        &mut self,
        start: u64,
        end: u64,
        sends: usize,
        recvs: usize,
        recvs_from_previous: [usize; BLOCK_SLOTS],
    ) -> Result<(), AikaError> {
        println!("GVT Master: committing block #{:?} with new gvt {end}", self.block_counter + 1);
        self.block_counter += 1;
        let mut new = Block::<BLOCK_SLOTS>::new(start, end, usize::MAX, self.block_counter)?;
        new.recvs = recvs;
        new.sends = sends;
        new.recvs_from_previous = recvs_from_previous;
        self.blocks.write(new, end, None);
        self.gvt = end;

        self.next.fill(None);

        let mut new_pendings = 0;
        for (planet_idx, planet_pending) in self.pending.iter_mut().enumerate() {
            // Move the closest pending block (if any) to self.next
            if let Some(block) = planet_pending[0].take() {
                let diff = block.sends as i32 - block.recvs as i32;
                new_pendings += diff;
                self.next[planet_idx] = Some(block);
            }

            // Shift all other pending blocks left
            for i in 0..(BLOCK_SLOTS - 1) {
                planet_pending[i] = planet_pending[i + 1].take();
            }
            planet_pending[BLOCK_SLOTS - 1] = None;
        }
        if new_pendings < 0 {
            return Err(AikaError::TimeTravel);
        }

        self.unmatched_sends = new_pendings as usize;
        Ok(())
    }

    fn check_all_terminal(&mut self) -> Result<bool, AikaError> {
        if self.gvt as f64 * self.timestep >= self.terminal {
            return Ok(true);
        }
        let latest = self.fetch_latest_uncommited_blocks()?;
        let mut truth = true;
        for block in latest {
            if let Some(block) = block {
                truth = (block.end as f64 * self.timestep) >= self.terminal;
                continue;
            }
            return Ok(false);
        }
        Ok(truth)
    }

    fn check_terminate(&mut self) -> bool {
        if self.unmatched_sends != 0 {
            println!("GVT Master: unmatched sends aren't zero yet! {:?}", self.unmatched_sends);
            return false;
        }
        if !self.next.iter().all(|x| x.is_none()) {
            println!("GVT Master: there are still blocks pending approval!");
            return false;
        }
        true
    }

    pub fn master(&mut self) -> Result<(), AikaError> {
        loop {
            // mail
            //println!("GVT Master, GVT {:?}: delivering mail...", self.gvt);
            for _ in 0..10 {
                self.deliver_the_mail()?;
                self.poll_blocks()?;
            }
            //println!("GVT Master, GVT {:?}: polling blocks, updating time consensus...", self.gvt);
            // block polling and try commiting
            while let Some(new_gvt) = self.update_consensus()? {
                self.gvtcomms.broadcast(new_gvt);
            }
            // check if all worlds have reached an end, and check if all messages have been received and blocks processed
            if self.check_all_terminal()? {
                println!("GVT Master, GVT {:?}: all planets are waiting", self.gvt);
                if self.check_terminate() {
                    println!("GVT Master, GVT {:?}: GVT has caught up, consensus reached!", self.gvt);
                    break;
                }
            }
        }
        Ok(())
    }

    pub fn with_block_size(&mut self, block_size: u64) {
        self.block_size = block_size;
    }
}
