use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use mesocarp::{
    comms::{
        spmc::{Broadcast, Subscriber},
        spsc::BufferWheel,
    },
    MesoError,
};

use crate::AikaError;

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct Block<const BLOCK_SLOTS: usize> {
    pub start: u64,
    pub end: u64,
    pub sends: usize,
    pub recvs: usize,
    pub recvs_from_previous: [usize; BLOCK_SLOTS],
    pub block_id: (usize, usize),
}

impl<const BLOCK_SLOTS: usize> Block<BLOCK_SLOTS> {
    pub fn new(start: u64, end: u64, world_id: usize, block_nmb: usize) -> Result<Self, AikaError> {
        if end <= start {
            return Err(AikaError::TimeTravel);
        }
        Ok(Self {
            start,
            end,
            sends: 0,
            recvs: 0,
            recvs_from_previous: [0; BLOCK_SLOTS],
            block_id: (world_id, block_nmb),
        })
    }

    pub fn send(&mut self) {
        self.sends += 1
    }

    pub fn recv(&mut self, send_timestamp: u64) -> Result<(), AikaError> {
        if send_timestamp < self.start {
            let diff = self.end - self.start;
            let real_diff = self.start - send_timestamp;
            let blocks = (real_diff / diff) as usize;
            if blocks >= BLOCK_SLOTS {
                return Err(AikaError::DistantBlocks(blocks));
            }
            self.recvs_from_previous[blocks] += 1;
            return Ok(());
        }
        self.recvs += 1;
        Ok(())
    }
}

impl<const BLOCK_SLOTS: usize> Default for Block<BLOCK_SLOTS> {
    fn default() -> Self {
        Self {
            start: 0,
            end: 0,
            sends: 0,
            recvs: 0,
            recvs_from_previous: [0; BLOCK_SLOTS],
            block_id: (usize::MAX, usize::MAX),
        }
    }
}

unsafe impl<const BLOCK_SLOTS: usize> Send for Block<BLOCK_SLOTS> {}
unsafe impl<const BLOCK_SLOTS: usize> Sync for Block<BLOCK_SLOTS> {}

unsafe impl<const BLOCK_SLOTS: usize> Pod for Block<BLOCK_SLOTS> {}
unsafe impl<const BLOCK_SLOTS: usize> Zeroable for Block<BLOCK_SLOTS> {}

pub(crate) struct GVTComms<const BLOCK_SLOTS: usize, const GVT_SLOTS: usize> {
    block_comms: Vec<Arc<BufferWheel<BLOCK_SLOTS, Block<BLOCK_SLOTS>>>>,
    gvt_broadcaster: Arc<Broadcast<GVT_SLOTS, u64>>,
}

impl<const BLOCK_SLOTS: usize, const GVT_SLOTS: usize> GVTComms<BLOCK_SLOTS, GVT_SLOTS> {
    pub fn new() -> Result<Self, AikaError> {
        Ok(Self {
            block_comms: Vec::new(),
            gvt_broadcaster: Arc::new(Broadcast::new()?),
        })
    }

    pub fn register(
        &mut self,
    ) -> (
        Arc<BufferWheel<BLOCK_SLOTS, Block<BLOCK_SLOTS>>>,
        Subscriber<GVT_SLOTS, u64>,
    ) {
        let wheel = Arc::new(BufferWheel::new());
        let cloned = Arc::clone(&wheel);
        self.block_comms.push(wheel);
        let sub = self.gvt_broadcaster.register_subscriber();
        (cloned, sub)
    }

    pub fn poll(&mut self) -> Result<Vec<Option<Vec<Block<BLOCK_SLOTS>>>>, AikaError> {
        let mut blocks = Vec::new();
        for i in &mut self.block_comms {
            let mut planet_blocks = Vec::new();
            for _ in 0..BLOCK_SLOTS {
                match i.read() {
                    Ok(block) => planet_blocks.push(block),
                    Err(err) => {
                        if let MesoError::NoPendingUpdates = err {
                            break;
                        }
                        return Err(AikaError::MesoError(err));
                    }
                }
            }
            if !planet_blocks.is_empty() {
                blocks.push(Some(planet_blocks));
                continue;
            }
            blocks.push(None);
        }
        Ok(blocks)
    }

    pub fn broadcast(&mut self, gvt: u64) {
        self.gvt_broadcaster.broadcast(gvt);
    }
}
