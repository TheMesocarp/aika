use std::{
    fs::File,
    io::Write,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use bytemuck::{Pod, Zeroable};
use mesocarp::comms::buses::ThreadedMessenger;

use crate::{
    env::Environment,
    mt::{
        consensus::{Block, ComputeLayout, Consensus},
        engines::{
            hlocal::{bus::MessageBus, Planet},
            HTime,
        },
    },
    objects::Mail,
    AikaError,
};

#[derive(Debug)]
/// A `Substrate` is an inter-cluster message bus and GVT updater, intended to own its own thread.
pub struct Substrate<
    const BLOCK_BW: usize,
    const MSG_BW: usize,
    MessageType: Pod + Zeroable + Clone,
> {
    /// The temporal consensus routine for an updted GVT computation.
    pub consensus: Consensus<BLOCK_BW>,
    pub(crate) messenger: ThreadedMessenger<MSG_BW, Mail<MessageType>>,
    /// Current time information.
    pub time: HTime,
    /// Maximum duration of a block. Also the expected length of a block unless the simulation terminates early.
    pub max_block_dur: u64,
    /// Current count of registered clusters.
    pub registered: usize,
    /// Maximum number of allowed clusters.
    pub planet_count: usize,
    /// Log file for the last run.
    log: Option<(File, File)>,
    /// start Instant of the simulation, for debug tracking.
    start: Instant,
}

impl<const BLOCK_BW: usize, const MSG_BW: usize, MessageType: Pod + Zeroable + Clone>
    Substrate<BLOCK_BW, MSG_BW, MessageType>
{
    /// Create a new `Substrate` with `planet_count: usize` maximum number of clusters, and `block_batch_size` arena allocation sizing for block logging.
    pub fn new(planet_count: usize, block_batch_size: usize) -> Result<Self, AikaError> {
        let start = Instant::now();
        let mut planet_ids = Vec::new();
        for i in 0..planet_count {
            planet_ids.push(i);
        }
        let messenger = ThreadedMessenger::new(planet_count)?;

        Ok(Self {
            consensus: Consensus::new(ComputeLayout::HubSpoke, block_batch_size)?,
            messenger,
            time: HTime {
                gvt: 0,
                cp_hz: u64::MAX,
                terminal: u64::MAX,
            },
            max_block_dur: 64,
            registered: 0,
            planet_count,
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

    /// Spawn a new simulation cluster on this `Substrate`'s coordination infrastructure.
    pub fn spawn_cluster<const CLOCK_BW: usize, const CLOCK_SCALES: usize>(
        &mut self,
        env: impl Environment + 'static,
    ) -> Result<Planet<BLOCK_BW, MSG_BW, CLOCK_BW, CLOCK_SCALES, MessageType>, AikaError> {
        if self.registered == self.planet_count {
            return Err(AikaError::MaximumClustersAllowed);
        }
        let id = self.registered;
        self.registered += 1;
        let messenger_account = self.messenger.get_user()?;
        let mut spoke = self.consensus.register_producer(None)?.unwrap();
        spoke.block.max_dur = self.max_block_dur;
        spoke.block.dur = self.max_block_dur;
        Planet::from_galaxy_registration(env, self.time, spoke, messenger_account, id, self.start)
    }

    // Sets the log file
    pub fn set_log(&mut self, file: (File, File)) {
        self.log = Some(file);
    }

    pub fn split_substrate(
        self,
    ) -> Result<(GVT<BLOCK_BW>, MessageBus<MSG_BW, MessageType>), AikaError> {
        if self.registered != self.planet_count {
            return Err(AikaError::NotAllClustersRegistered);
        }
        let mut gvtfile = None;
        let mut busfile = None;
        if self.log.is_some() {
            let (gfile, bfile) = self.log.unwrap();
            gvtfile = Some(gfile);
            busfile = Some(bfile);
        }

        let term = Arc::new(AtomicBool::new(false));
        let cloned = Arc::clone(&term);

        let gvt = GVT::from_substrate(
            self.consensus,
            self.time,
            self.max_block_dur,
            gvtfile,
            self.start,
            term,
        );
        let bus = MessageBus::from_substrate(self.messenger, busfile, self.start, cloned);
        Ok((gvt, bus))
    }

    pub fn rejoin_substrate(gvt: GVT<BLOCK_BW>, bus: MessageBus<MSG_BW, MessageType>) -> Self {
        let count = bus.messenger.capacity;
        let mut log = None;
        if bus.log.is_some() {
            let blog = bus.log.unwrap();
            let glog = gvt.log.unwrap();
            log = Some((glog, blog));
        }
        Self {
            consensus: gvt.consensus,
            messenger: bus.messenger,
            time: gvt.time,
            max_block_dur: gvt.max_block_dur,
            registered: count,
            planet_count: count,
            log,
            start: gvt.start,
        }
    }
}

unsafe impl<const BLOCK_BW: usize, const MSG_BW: usize, MessageType: Pod + Zeroable + Clone> Send
    for Substrate<BLOCK_BW, MSG_BW, MessageType>
{
}
unsafe impl<const BLOCK_BW: usize, const MSG_BW: usize, MessageType: Pod + Zeroable + Clone> Sync
    for Substrate<BLOCK_BW, MSG_BW, MessageType>
{
}

pub struct GVT<const BLOCK_BW: usize> {
    /// The temporal consensus routine for an updted GVT computation.
    pub consensus: Consensus<BLOCK_BW>,
    /// Current time information.
    pub time: HTime,
    /// Maximum duration of a block. Also the expected length of a block unless the simulation terminates early.
    pub max_block_dur: u64,
    /// Log file for the last run.
    log: Option<File>,
    /// start Instant of the simulation, for debug tracking.
    start: Instant,
    terminal_flag: Arc<AtomicBool>,
}

impl<const BLOCK_BW: usize> GVT<BLOCK_BW> {
    fn from_substrate(
        consensus: Consensus<BLOCK_BW>,
        time: HTime,
        max_block_dur: u64,
        log: Option<File>,
        start: Instant,
        terminal_flag: Arc<AtomicBool>,
    ) -> Self {
        Self {
            consensus,
            time,
            max_block_dur,
            log,
            start,
            terminal_flag,
        }
    }

    // Check if all clusters are at terminal time.
    fn check_all_terminal(&mut self) -> Result<bool, AikaError> {
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
                self.consensus.poll_n_slot()?;
                while let Some(new_gvt) = self.consensus.cusp()? {
                    if new_gvt == self.time.gvt {
                        break;
                    }
                    self.consensus.processor.broadcast_new_safe_point(new_gvt)?;
                }
            }

            if self.check_all_terminal()? && self.time.gvt == self.time.terminal {
                break;
            }
        }
        self.terminal_flag.store(true, Ordering::Release);
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
            std::thread::sleep(Duration::from_nanos(5));
        }
        if let Some(log) = &mut self.log {
            writeln!(
                log,
                "[{}] Sending termination flag to message bus.",
                self.start.elapsed().as_micros()
            )
            .map_err(|_| AikaError::LoggingWriteError)?;
        }
        self.terminal_flag.store(true, Ordering::SeqCst);
        Ok(self)
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;
    use crate::actors::{Actor, ConnectedActor, Context};
    use crate::env::Stateless;
    use crate::objects::{Msg, SchedulingTask};

    #[derive(Debug, Copy, Clone)]
    #[repr(C)]
    struct TestMsg;
    unsafe impl Pod for TestMsg {}
    unsafe impl Zeroable for TestMsg {}

    #[derive(Debug)]
    #[allow(dead_code)]
    struct DummyActor;
    impl Actor<TestMsg> for DummyActor {
        fn step(
            &mut self,
            _: &mut Context<TestMsg>,
            _id: usize,
        ) -> Result<SchedulingTask, AikaError> {
            Ok(SchedulingTask::Wait)
        }
    }
    impl ConnectedActor<TestMsg> for DummyActor {
        fn read_message(
            &mut self,
            _: &mut Context<TestMsg>,
            _: Msg<TestMsg>,
            _: usize,
        ) -> Result<(), AikaError> {
            Ok(())
        }
    }

    #[test]
    fn test_galaxy_creation() {
        let galaxy: Substrate<8, 16, TestMsg> = Substrate::new(4, 128).unwrap();
        assert_eq!(galaxy.planet_count, 4);
        assert_eq!(galaxy.registered, 0);
        assert_eq!(galaxy.time.terminal, u64::MAX);
        assert_eq!(galaxy.time.gvt, 0);
        assert_eq!(galaxy.max_block_dur, 64);
    }

    #[test]
    fn test_galaxy_configuration() {
        let mut galaxy: Substrate<8, 16, TestMsg> = Substrate::new(2, 64).unwrap();

        galaxy.set_time_scale(1000);
        assert_eq!(galaxy.time.terminal, 1000);

        galaxy.checkpoints(50);
        assert_eq!(galaxy.time.cp_hz, 50);

        galaxy.with_block_duration(25);
        assert_eq!(galaxy.max_block_dur, 25);
    }

    #[test]
    fn test_planet_spawning() {
        let mut galaxy: Substrate<8, 16, TestMsg> = Substrate::new(3, 64).unwrap();

        let planet1 = galaxy.spawn_cluster::<32, 2>(Stateless).unwrap();
        assert_eq!(galaxy.registered, 1);
        assert_eq!(planet1.context.cluster_id, 0);

        let planet2 = galaxy.spawn_cluster::<32, 2>(Stateless).unwrap();
        assert_eq!(galaxy.registered, 2);
        assert_eq!(planet2.context.cluster_id, 1);

        let planet3 = galaxy.spawn_cluster::<32, 2>(Stateless).unwrap();
        assert_eq!(galaxy.registered, 3);
        assert_eq!(planet3.context.cluster_id, 2);

        let result = galaxy.spawn_cluster::<32, 2>(Stateless);
        assert!(matches!(result, Err(AikaError::MaximumClustersAllowed)));
    }

    #[test]
    fn test_galaxy_terminal_time_requirement() {
        let substrate: Substrate<8, 16, TestMsg> = Substrate::new(2, 64).unwrap();
        let (galaxy, _) = substrate.split_substrate().unwrap();
        let result = galaxy.master();
        assert!(matches!(result, Err(AikaError::MustSetTerminalTime)));
    }

    #[test]
    fn test_message_delivery() {
        let mut galaxy: Substrate<8, 16, TestMsg> = Substrate::new(2, 64).unwrap();
        galaxy.set_time_scale(100);

        let mut planet1 = galaxy.spawn_cluster::<32, 2>(Stateless).unwrap();
        let mut planet2 = galaxy.spawn_cluster::<32, 2>(Stateless).unwrap();

        let msg = Msg::new(TestMsg, 0, 10, 0, Some(0));
        planet1.context.send_mail(msg, 1).unwrap();

        let sends = std::mem::take(&mut planet1.context.outbox);

        let (_, mut bus) = galaxy.split_substrate().unwrap();

        for mail in sends {
            planet1.message_user.send(mail).unwrap();
        }

        bus.deliver_the_mail().unwrap();

        let received = planet2.message_user.poll();
        assert!(received.is_some());
    }

    #[test]
    fn test_check_all_terminal() {
        let mut galaxy: Substrate<8, 16, TestMsg> = Substrate::new(2, 64).unwrap();
        galaxy.set_time_scale(100);
        galaxy.with_block_duration(50);

        let mut planet1 = galaxy.spawn_cluster::<32, 2>(Stateless).unwrap();
        let mut planet2 = galaxy.spawn_cluster::<32, 2>(Stateless).unwrap();

        let (mut galaxy, _) = galaxy.split_substrate().unwrap();
        assert!(!galaxy.check_all_terminal().unwrap());

        planet1.blocks.block.start = 100;
        planet1.blocks.block.dur = 0;
        planet2.blocks.block.start = 100;
        planet2.blocks.block.dur = 0;

        planet1
            .blocks
            .submitter
            .write(planet1.blocks.block)
            .unwrap();
        planet2
            .blocks
            .submitter
            .write(planet2.blocks.block)
            .unwrap();

        galaxy.consensus.poll_n_slot().unwrap();

        galaxy.time.gvt = 100;
        assert!(galaxy.check_all_terminal().unwrap());
    }
}
