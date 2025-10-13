//! `hlocal` contains the infrastructure for `aika`'s hybrid synchronization model, configured to operate with a GVT Master thread.
//!
//! `aika`'s hybrid model is inspired by [Local Time Warp](https://dl.acm.org/doi/abs/10.1145/158459.158474); where local "clusters",
//! or in our case `Planet`, operate conservatively on a partition of the global state, while inter-cluster coordination,
//! in otherwords `Substrate`, synchronizes clusters optimistically. The GVT is computed asynchronously using a block-based message
//! counting algorithm, defined [here](https://docs.rs/mesocarp/latest/mesocarp/sync/gvt/aika/index.html). The block-based nature
//! implies this is inherently a conservative GVT update: its accuracy is at a trade off with resource usage in the stack during runtime.
//! Additionally, the model supports a synchronization checkpointing system to help reduce the possibility and depth of rollback
//! cascades between clusters.
//!
//! This `hlocal` variant of aika's hybrid model is structured centrally around the `Substrate` thread, which manage block update processing,
//! GVT update broadcasts to clusters, and inter-cluster message passing via a message bus.
use std::{
    sync::{Arc, Barrier, Mutex},
    time::Instant,
};

use bytemuck::{Pod, Zeroable};

use crate::{
    actors::ConnectedActor,
    env::Environment,
    mt::{engines::hlocal::bus::MessageBus, logging::setup_hlocal_logging, RunMode},
    AikaError,
};

pub use crate::mt::engines::hlocal::{
    cluster::Planet,
    gvt::{Substrate, GVT},
};

mod bus;
mod cluster;
mod gvt;

#[derive(Debug, Copy, Clone)]
/// A `Config` packages all of the configuration information for the simulation's GVT/consensus algorithm.
pub struct Config {
    pub clusters: usize,
    pub message_bandwidth: usize,
    pub batch_size: usize,
    pub block_duration: u64,
    pub terminal: u64,
    pub checkpoint_frequency: u64,
}

impl Config {
    pub fn new(
        clusters: usize,
        message_bandwidth: usize,
        batch_size: usize,
        block_duration: u64,
        terminal: u64,
        checkpoint_frequency: u64,
    ) -> Self {
        Self {
            clusters,
            message_bandwidth,
            batch_size,
            block_duration,
            terminal,
            checkpoint_frequency,
        }
    }
}

/// A `Stager` is the main entry point to simulations. It provides a clean interface for staging and running `hlocal` simulations.
pub struct Stager<
    const BLOCK_BW: usize,
    const CLOCK_BW: usize,
    const CLOCK_SCALES: usize,
    MessageType: Pod + Zeroable + Clone,
> {
    pub substrate: Option<Substrate<BLOCK_BW, MessageType>>,
    pub clusters: Vec<Planet<BLOCK_BW, CLOCK_BW, CLOCK_SCALES, MessageType>>,
    configured: bool,
}

impl<
        const BLOCK_BW: usize,
        const CLOCK_BW: usize,
        const CLOCK_SCALES: usize,
        MessageType: Pod + Zeroable + Clone,
    > Stager<BLOCK_BW, CLOCK_BW, CLOCK_SCALES, MessageType>
{
    pub fn new() -> Result<Self, AikaError> {
        Ok(Self {
            substrate: None,
            clusters: Vec::new(),
            configured: false,
        })
    }

    pub fn config(&mut self, config: Config) -> Result<(), AikaError> {
        let mut substrate = Substrate::new(config.clusters, config.batch_size, config.message_bandwidth)?;
        substrate.set_time_scale(config.terminal);
        substrate.with_block_duration(config.block_duration);
        substrate.checkpoints(config.checkpoint_frequency);
        self.substrate = Some(substrate);
        self.configured = true;
        Ok(())
    }

    pub fn create_cluster(&mut self, env: impl Environment + 'static) -> Result<(), AikaError> {
        if self.configured {
            let cluster = self.substrate.as_mut().unwrap().spawn_cluster(env)?;
            self.clusters.push(cluster);
            return Ok(());
        }
        Err(AikaError::UnconfiguredStager)
    }

    pub fn spawn_actor_on_cluster(
        &mut self,
        cluster: usize,
        actor: impl ConnectedActor<MessageType> + 'static,
    ) -> Result<(), AikaError> {
        let clusters = self.clusters.len();
        if clusters <= cluster {
            return Err(AikaError::InvalidClusterId(clusters, cluster));
        }
        self.clusters[cluster].spawn_actor(actor);
        Ok(())
    }

    pub fn schedule(&mut self, cluster: usize, actor: usize, time: u64) -> Result<(), AikaError> {
        let clusters = self.clusters.len();
        if clusters <= cluster {
            return Err(AikaError::InvalidClusterId(clusters, cluster));
        }
        let actors = self.clusters[cluster].actors.len();
        if actors <= actor {
            return Err(AikaError::InvalidActorId(actors, cluster, actor));
        }
        self.clusters[cluster].schedule(time, actor)?;
        Ok(())
    }

    pub fn schedule_cluster(&mut self, cluster: usize, time: u64) -> Result<(), AikaError> {
        let clusters = self.clusters.len();
        if clusters <= cluster {
            return Err(AikaError::InvalidClusterId(clusters, cluster));
        }
        let actors = self.clusters[cluster].actors.len();
        for i in 0..actors {
            self.clusters[cluster].schedule(time, i)?;
        }
        Ok(())
    }

    pub fn schedule_all(&mut self, time: u64) -> Result<(), AikaError> {
        let clusters = self.clusters.len();
        for cluster in 0..clusters {
            let actors = self.clusters[cluster].actors.len();
            for i in 0..actors {
                self.clusters[cluster].schedule(time, i)?;
            }
        }
        Ok(())
    }

    pub fn run(mut self, mode: RunMode) -> Result<Self, AikaError> {
        match self.check_ready() {
            Ok(_) => {}
            Err(err) => match err {
                None => {
                    return Err(AikaError::NotAllClustersRegistered);
                }
                Some(i) => return Err(AikaError::NoActors(i)),
            },
        }

        let num_threads = self.clusters.len() + 2;
        let barrier = Arc::new(Barrier::new(num_threads));
        let start_time = Arc::new(Mutex::new(None::<Instant>));

        let mut pfiles = vec![];
        if RunMode::Debug == mode {
            let mut files = setup_hlocal_logging(self.clusters.len())
                .map_err(|_| AikaError::LoggingSetupFailure)?;
            let busfile = files.pop().unwrap();
            let gvtfile = files.pop().unwrap();
            self.substrate.as_mut().unwrap().set_log((gvtfile, busfile));
            pfiles = files;
        }

        let (gvt, bus) = self.substrate.unwrap().split_substrate()?;

        let cluster = self.clusters;
        let phandles = match mode {
            RunMode::Fast => cluster
                .into_iter()
                .enumerate()
                .map(|(i, planet)| {
                    let barrier_clone = Arc::clone(&barrier);
                    std::thread::Builder::new()
                        .name(format!("Cluster {i}"))
                        .spawn(move || {
                            barrier_clone.wait();
                            let planet = planet.run()?;
                            Ok::<
                                Planet<BLOCK_BW, CLOCK_BW, CLOCK_SCALES, MessageType>,
                                AikaError,
                            >(planet)
                        })
                        .map_err(|_| AikaError::ThreadPanic)
                })
                .collect::<Result<Vec<_>, _>>(),
            RunMode::Debug => {
                let cluster = cluster.into_iter().zip(pfiles).collect::<Vec<_>>();
                cluster
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
                                    Planet<BLOCK_BW, CLOCK_BW, CLOCK_SCALES, MessageType>,
                                    AikaError,
                                >(planet)
                            })
                            .map_err(|_| AikaError::ThreadPanic)
                    })
                    .collect::<Result<Vec<_>, _>>()
            }
        }?;
        let barrier_clone = Arc::clone(&barrier);
        let ghandle = std::thread::Builder::new()
            .name("GVT Consensus".to_owned())
            .spawn(move || {
                let gvt = match mode {
                    RunMode::Fast => {
                        barrier_clone.wait();
                        gvt.master()?
                    }
                    RunMode::Debug => {
                        barrier_clone.wait();
                        gvt.master_debug()?
                    }
                };
                Ok::<GVT<BLOCK_BW>, AikaError>(gvt)
            })
            .map_err(|_| AikaError::ThreadPanic)?;

        let bhandle = std::thread::Builder::new()
            .name("GVT Consensus".to_owned())
            .spawn(move || {
                barrier.wait();
                let bus = bus.master()?;
                Ok::<MessageBus<MessageType>, AikaError>(bus)
            })
            .map_err(|_| AikaError::ThreadPanic)?;

        let mut clusters = Vec::new();
        for handle in phandles {
            let planet = handle.join().map_err(|_| AikaError::ThreadPanic)??;
            clusters.push(planet);
        }
        let gvt = ghandle.join().map_err(|_| AikaError::ThreadPanic)??;
        let bus = bhandle.join().map_err(|_| AikaError::ThreadPanic)??;

        if mode == RunMode::Debug {
            let execution_time = {
                let start = start_time.lock().unwrap();
                start.unwrap().elapsed()
            };
            println!("Runtime: {execution_time:?}")
        }
        let substrate = Some(Substrate::rejoin_substrate(gvt, bus));
        Ok(Self {
            substrate,
            clusters,
            configured: true,
        })
    }

    fn check_ready(&self) -> Result<(), Option<usize>> {
        if self.substrate.is_none() {
            return Err(None);
        }
        let substrate = self.substrate.as_ref().unwrap();
        if substrate.registered != substrate.planet_count {
            return Err(None);
        }
        for (i, planet) in self.clusters.iter().enumerate() {
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
        $crate::mt::engines::hlocal::Stager::<32, 128, 2, $msg_type>::new()
    };

    // With BLOCK_BW only
    ($msg_type:ty, BLOCK_BW = $block_bw:expr) => {
        $crate::mt::engines::hlocal::Stager::<$block_bw, 128, 2, $msg_type>::new()
    };

    // With CLOCK_BW only
    ($msg_type:ty, CLOCK_BW = $clock_bw:expr) => {
        $crate::mt::engines::hlocal::Stager::<32, $clock_bw, 2, $msg_type>::new()
    };

    // With CLOCK_SCALES only
    ($msg_type:ty, CLOCK_SCALES = $clock_scales:expr) => {
        $crate::mt::engines::hlocal::Stager::<32, 128, $clock_scales, $msg_type>::new()
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
    ($msg_type:ty, CLOCK_BW = $clock_bw:expr) => {
        $crate::mt::engines::hlocal::Stager::<32, $clock_bw, 2, $msg_type>::new()
    };

    // With MSG_BW and CLOCK_SCALES
    ($msg_type:ty, CLOCK_SCALES = $clock_scales:expr) => {
        $crate::mt::engines::hlocal::Stager::<32, $clock_scales, $msg_type>::new()
    };

    // With CLOCK_BW and CLOCK_SCALES
    ($msg_type:ty, CLOCK_BW = $clock_bw:expr, CLOCK_SCALES = $clock_scales:expr) => {
        $crate::mt::engines::hlocal::Stager::<32, $clock_bw, $clock_scales, $msg_type>::new()
    };

    // With BLOCK_BW, MSG_BW, and CLOCK_BW
    ($msg_type:ty, BLOCK_BW = $block_bw:expr, CLOCK_BW = $clock_bw:expr) => {
        $crate::mt::engines::hlocal::Stager::<$block_bw, $clock_bw, 2, $msg_type>::new()
    };

    // With BLOCK_BW, MSG_BW, and CLOCK_SCALES
    ($msg_type:ty, BLOCK_BW = $block_bw:expr, CLOCK_SCALES = $clock_scales:expr) => {
        $crate::mt::engines::hlocal::Stager::<$block_bw, 128, $clock_scales, $msg_type>::new()
    };

    // With BLOCK_BW, CLOCK_BW, and CLOCK_SCALES
    ($msg_type:ty, BLOCK_BW = $block_bw:expr, CLOCK_BW = $clock_bw:expr, CLOCK_SCALES = $clock_scales:expr) => {
        $crate::mt::engines::hlocal::Stager::<$block_bw, $clock_bw, $clock_scales, $msg_type>::new()
    };

    // With MSG_BW, CLOCK_BW, and CLOCK_SCALES
    ($msg_type:ty, CLOCK_BW = $clock_bw:expr, CLOCK_SCALES = $clock_scales:expr) => {
        $crate::mt::engines::hlocal::Stager::<32, $clock_bw, $clock_scales, $msg_type>::new()
    };

    // With all parameters
    ($msg_type:ty, BLOCK_BW = $block_bw:expr, CLOCK_BW = $clock_bw:expr, CLOCK_SCALES = $clock_scales:expr) => {
        $crate::mt::engines::hlocal::Stager::<$block_bw, $clock_bw, $clock_scales, $msg_type>::new()
    };
}

#[cfg(test)]
mod unit_tests {
    use std::thread;

    use super::*;
    use crate::actors::{Actor, ConnectedActor, Context};
    use crate::env::SimpleUnified;
    use crate::objects::{AntiMsg, Msg, SchedulingTask};
    use bytemuck::{Pod, Zeroable};
    use mesocarp::logging::Journal;
    use mesocarp::scheduling::Scheduleable;
    use mesocarp::MesoError;

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
            _actor_id: usize,
        ) -> Result<SchedulingTask, AikaError> {
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
            Ok(SchedulingTask::Timeout(1))
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
        ) -> Result<SchedulingTask, AikaError> {
            let id = actor_id;
            let time = context.time;
            context.send_mail(
                Msg::new(TestMessage, time, time + 1, id, (id + 1) % AGENTS),
                context.cluster_id,
            )?;
            Ok(SchedulingTask::Wait)
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
            Substrate<BLOCK_BANDWIDTH, MessageType>,
            Planet<BLOCK_BANDWIDTH, CLOCK_BW, CLOCK_SCALES, MessageType>,
        ),
        AikaError,
    >
    where
        TestAgent: ConnectedActor<MessageType>,
    {
        let mut substrate = Substrate::<BLOCK_BANDWIDTH, MessageType>::new(1, 12, MSG_BANDWIDTH)?;
        substrate.set_time_scale(terminal);
        substrate.with_block_duration(block_dur);
        let mut planet = substrate.spawn_cluster::<CLOCK_BW, CLOCK_SCALES>(SimpleUnified {
            inner: Journal::init(1024),
        })?;
        for j in 0..AGENTS {
            let actor = TestAgent::new(j);
            planet.spawn_actor(actor);
        }
        Ok((substrate, planet))
    }

    fn schedule_all<
        const CLOCK_BW: usize,
        const CLOCK_SCALES: usize,
        MessageType: Pod + Zeroable + Clone,
    >(
        cluster: &mut Planet<BLOCK_BANDWIDTH, CLOCK_BW, CLOCK_SCALES, MessageType>,
        time: u64,
    ) -> Result<(), AikaError> {
        for i in 0..AGENTS {
            cluster.schedule(time, i)?;
        }
        Ok(())
    }

    #[test]
    fn test_stager_macro() {
        stager!(TestMessage).unwrap();
        stager!(TestMessage, BLOCK_BW = 48).unwrap();
        stager!(TestMessage, BLOCK_BW = 48).unwrap();
        stager!(TestMessage, CLOCK_BW = 128).unwrap();
        stager!(TestMessage, CLOCK_BW = 128, CLOCK_SCALES = 1).unwrap();
    }

    #[test]
    fn test_simple_setup() {
        let (substrate, mut cluster) = create_setup::<64, 2, TestMessage>(1, 1).unwrap();
        let (gvt, bus) = substrate.split_substrate().unwrap();
        schedule_all(&mut cluster, 0).unwrap();
        let ghandle = thread::spawn(move || gvt.master());
        let bhandle = thread::spawn(move || bus.master());
        let phandle = thread::spawn(move || cluster.run());

        let substrate_result = ghandle.join().expect("Substrate thread panicked.");
        let bus_result = bhandle.join().expect("Bus panicked.");
        let planet_result = phandle.join().expect("Planet thread panicked.");

        // Ensure both threads completed successfully
        assert!(substrate_result.is_ok());
        assert!(
            planet_result.is_ok(),
            "Planet run loop failed: {:?}",
            planet_result.err().unwrap()
        );
        assert!(bus_result.is_ok())
    }

    #[test]
    fn test_multiplanet_setup() {
        const CLUSTERS: usize = 6;
        let mut stager =
            Stager::<BLOCK_BANDWIDTH, 64, 2, TestMessage>::new().unwrap();

        let config = Config {
            clusters: CLUSTERS,
            message_bandwidth: MSG_BANDWIDTH,
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
        let mut substrate: Substrate<8, TestMessage> = Substrate::new(1, 1, 8).unwrap();
        let mut planet = substrate
            .spawn_cluster::<16, 2>(SimpleUnified {
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

        let (_, mut cluster) = create_setup::<64, 2, TestMessage>(50, 1).unwrap();
        cluster.spawn_actor(TestAgent::new(0));
        cluster.spawn_actor(TestAgent::new(1));
        for i in 0..100 {
            cluster.commit_mail(Msg::new(TestMessage, i, i + 10, 0, 1));
            cluster
                .context
                .anti_msgs
                .write(AntiMsg::new(i, i + 10, (0, 0), (0, 1)), i, None);
        }
        for _ in 0..50 {
            cluster.step().unwrap();
        }
        cluster.rollback(25).unwrap();
        assert_eq!(cluster.context.anti_msgs.read_all::<AntiMsg>().len(), 25);
        assert_eq!(
            cluster
                .context
                .anti_msgs
                .read_state::<AntiMsg>()
                .unwrap()
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
        objects::{Msg, SchedulingTask},
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
        target: usize,
        cluster: usize,
        delay1: u64,
        delay2: u64,
    }

    impl MessagingActor {
        fn new(target: usize, cluster: usize, delay1: u64, delay2: u64) -> Self {
            Self {
                recieved: 0,
                sent: 0,
                target,
                cluster,
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
        ) -> Result<SchedulingTask, AikaError> {
            let time = env.time;
            let msg = Msg::new(Message, time, time + self.delay1, actor_id, self.target);
            env.send_mail(msg, self.cluster)?;
            self.sent += 1;
            Ok(SchedulingTask::Wait)
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
            if self.target != msg.from.1 {
                let from = msg.from.0;
                let msg = Msg::new(Message, time, time + self.delay2, actor_id, msg.from.1);
                env.send_mail(msg, from)?;
                self.sent += 1;
            } else {
                let _ = self.step(env, actor_id)?;
            }
            Ok(())
        }
    }

    #[test]
    fn test_local_messaging() {
        let mut stager: Stager<BLOCK_BW, CLOCK_BW, CLOCK_SCALES, Message> =
            Stager::new().unwrap();

        let config = Config::new(1, MSG_BW, 128, 20, 2048, 10000000);
        stager.config(config).unwrap();

        stager.create_cluster(Stateless).unwrap();

        stager
            .spawn_actor_on_cluster(0, MessagingActor::new(1, 0, 5, 1))
            .unwrap();
        stager
            .spawn_actor_on_cluster(0, MessagingActor::new(0, 0, 5, 1))
            .unwrap();

        stager.schedule_cluster(0, 1).unwrap();

        stager.run(RunMode::Debug).unwrap();
    }

    #[test]
    fn test_local_messaging_heavy() {
        let mut stager: Stager<BLOCK_BW, CLOCK_BW, CLOCK_SCALES, Message> =
            Stager::new().unwrap();

        let config = Config::new(1, MSG_BW, 128, 20, 204, 10000000);
        stager.config(config).unwrap();

        stager.create_cluster(Stateless).unwrap();
        for i in 0..900 {
            stager
                .spawn_actor_on_cluster(0, MessagingActor::new((i + 1) % 200, 0, 5, 1))
                .unwrap();
        }
        stager.schedule_cluster(0, 1).unwrap();

        stager.run(RunMode::Debug).unwrap();
    }

    #[test]
    fn test_intercluster_messaging() {
        let mut stager: Stager<216, CLOCK_BW, CLOCK_SCALES, Message> = Stager::new().unwrap();

        let config = Config::new(2, 1024, 128, 128, 2048, 10);
        stager.config(config).unwrap();

        stager.create_cluster(Stateless).unwrap();
        stager.create_cluster(Stateless).unwrap();

        stager
            .spawn_actor_on_cluster(0, MessagingActor::new(0, 1, 5, 1))
            .unwrap();
        stager
            .spawn_actor_on_cluster(1, MessagingActor::new(0, 0, 5, 1))
            .unwrap();

        stager.schedule_cluster(0, 1).unwrap();
        stager.schedule_cluster(1, 1).unwrap();

        stager.run(RunMode::Fast).unwrap();
    }

    #[test]
    fn test_intercluster_messaging_heavy() {
        let mut stager = stager!(Message, BLOCK_BW = 128).unwrap();
        let config = Config::new(4, 16 * 1024, 32, 48, 2048, 10);
        stager.config(config).unwrap();

        stager.create_cluster(Stateless).unwrap();
        stager.create_cluster(Stateless).unwrap();
        stager.create_cluster(Stateless).unwrap();
        stager.create_cluster(Stateless).unwrap();

        for i in 0..4 {
            for j in 0..10 {
                stager
                    .spawn_actor_on_cluster(
                        i,
                        MessagingActor::new((j + 1) % 10, (i + 1) % 4, 2, 2),
                    )
                    .unwrap();
            }
        }

        stager.schedule_all(1).unwrap();
        stager.run(RunMode::Fast).unwrap();
    }
}
