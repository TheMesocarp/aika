use std::{sync::Arc, thread, time::Duration};

use crate::{
    agents::ThreadedAgent,
    messages::Transfer,
    mt::optimistic::{
        config::LPConfig,
        gvt::{RegistryOutput, GVT},
        lp::LP,
    },
    SimError,
};

pub mod config;
pub mod gvt;
pub mod lp;

pub struct TimeWarpBuilder<const SLOTS: usize, MessageType: Clone> {
    agents: Vec<Box<dyn ThreadedAgent<SLOTS, Transfer<MessageType>>>>,
    registries: Vec<RegistryOutput<SLOTS, MessageType>>,
    configs: Vec<LPConfig>,
    gvt: GVT<SLOTS, MessageType>,
    num_agents: usize,
}

impl<const SLOTS: usize, MessageType: Clone> TimeWarpBuilder<SLOTS, MessageType> {
    pub fn new(num_agents: usize) -> Result<Self, SimError> {
        let gvt = GVT::new(num_agents)?;
        Ok(Self {
            agents: Vec::new(),
            registries: Vec::new(),
            configs: Vec::new(),
            gvt,
            num_agents,
        })
    }

    pub fn set_agent_config(&mut self, config: LPConfig) {
        let configs = vec![config; self.num_agents];
        self.configs = configs;
    }

    pub fn spawn(
        &mut self,
        agent: impl ThreadedAgent<SLOTS, Transfer<MessageType>> + 'static,
    ) -> Result<(), SimError> {
        if self.agents.len() == self.num_agents {
            return Err(SimError::MaximumAgentsAllowed);
        }
        let boxed = Box::new(agent);
        let registry = self.gvt.register_agent()?;
        self.agents.push(boxed);
        self.registries.push(registry);
        Ok(())
    }

    pub fn spawn_custom(
        &mut self,
        agent: impl ThreadedAgent<SLOTS, Transfer<MessageType>> + 'static,
        config: LPConfig,
    ) -> Result<(), SimError> {
        let idx = self.agents.len();
        if idx == self.num_agents {
            return Err(SimError::MaximumAgentsAllowed);
        }
        let boxed = Box::new(agent);
        let registry = self.gvt.register_agent()?;
        self.agents.push(boxed);
        self.registries.push(registry);
        if self.configs.len() == self.num_agents {
            self.configs[idx] = config;
        } else {
            self.configs.push(config);
        }
        Ok(())
    }

    fn check_ready(&self) -> bool {
        if self.agents.len() != self.num_agents
            || self.registries.len() != self.num_agents
            || self.configs.len() != self.num_agents
        {
            return false;
        }
        true
    }

    pub fn build<const CLOCK_SLOTS: usize, const CLOCK_HEIGHT: usize>(
        self,
    ) -> Result<TimeWarp<SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>, SimError> {
        if !self.check_ready() {
            return Err(SimError::NotAllAgentsRegistered);
        }
        let zipped = self
            .agents
            .into_iter()
            .zip(
                self.configs
                    .into_iter()
                    .zip(self.registries)
                    .collect::<Vec<_>>(),
            )
            .collect::<Vec<_>>();
        let mut lps = Vec::new();
        for (agent, config_and_registry) in zipped {
            let lp = LP::init(agent, config_and_registry.1, config_and_registry.0)?;
            lps.push(lp);
        }
        Ok(TimeWarp { gvt: self.gvt, lps })
    }
}

pub struct TimeWarp<
    const SLOTS: usize,
    const CLOCK_SLOTS: usize,
    const CLOCK_HEIGHT: usize,
    MessageType: Clone + 'static,
> {
    gvt: GVT<SLOTS, MessageType>,
    lps: Vec<LP<SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>>,
}

impl<
        const SLOTS: usize,
        const CLOCK_SLOTS: usize,
        const CLOCK_HEIGHT: usize,
        MessageType: Clone,
    > TimeWarp<SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
    pub fn run<F>(
        self,
    ) -> Result<TimeWarp<SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>, SimError> {
        let num_lps = self.lps.len();
        let mut handles = Vec::with_capacity(num_lps);

        let termination_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Start GVT polling thread
        let mut gvt_controller = self.gvt;
        let gvt_flag = Arc::clone(&termination_flag);
        let gvt_handle = thread::spawn(move || -> Result<GVT<SLOTS, MessageType>, SimError> {
            while !gvt_flag.load(std::sync::atomic::Ordering::Relaxed) {
                if let Err(e) = gvt_controller.poll() {
                    eprintln!("GVT polling error: {e:?}");
                    return Err(e);
                }
            }
            Ok(gvt_controller)
        });

        // Spawn threads for each logical process
        for lp in self.lps {
            let lp_flag = Arc::clone(&termination_flag);
            let handle = thread::spawn(
                move || -> Result<LP<SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>, SimError> {
                    let mut local_lp = lp;

                    local_lp.run(lp_flag)?;
                    Ok(local_lp)
                },
            );
            handles.push(handle);
        }

        termination_flag.store(true, std::sync::atomic::Ordering::Relaxed);
        let mut results_lps = Vec::new();
        // Wait for all threads to complete
        for (i, handle) in handles.into_iter().enumerate() {
            match handle.join() {
                Ok(result) => {
                    if let Err(e) = result {
                        eprintln!("LP {i} terminated with error: {e:?}");
                        return Err(e);
                    }
                    results_lps.push(result.unwrap());
                }
                Err(_) => {
                    eprintln!("LP {i} thread panicked");
                    return Err(SimError::ThreadPanic);
                }
            }
        }

        // Wait for GVT thread
        let gvt = match gvt_handle.join() {
            Ok(result) => {
                if let Err(e) = result {
                    eprintln!("GVT thread terminated with error: {e:?}");
                    return Err(e);
                }
                result.unwrap()
            }
            Err(_) => {
                eprintln!("GVT thread panicked");
                return Err(SimError::ThreadPanic);
            }
        };
        let timewarp = TimeWarp {
            gvt,
            lps: results_lps,
        };
        println!("Time Warp simulation completed with condition met");
        Ok(timewarp)
    }

    pub fn run_until<F>(
        self,
        mut condition: F,
    ) -> Result<TimeWarp<SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>, SimError>
    where
        F: FnMut() -> bool + Send + 'static,
    {
        let num_lps = self.lps.len();
        let mut handles = Vec::with_capacity(num_lps);

        let termination_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Start GVT polling thread
        let mut gvt_controller = self.gvt;
        let gvt_flag = Arc::clone(&termination_flag);
        let gvt_handle = thread::spawn(move || -> Result<GVT<SLOTS, MessageType>, SimError> {
            while !gvt_flag.load(std::sync::atomic::Ordering::Relaxed) {
                if let Err(e) = gvt_controller.poll() {
                    eprintln!("GVT polling error: {e:?}");
                    return Err(e);
                }
            }
            Ok(gvt_controller)
        });

        // Spawn threads for each logical process
        for lp in self.lps {
            let lp_flag = Arc::clone(&termination_flag);
            let handle = thread::spawn(
                move || -> Result<LP<SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>, SimError> {
                    let mut local_lp = lp;

                    local_lp.run(lp_flag)?;
                    Ok(local_lp)
                },
            );
            handles.push(handle);
        }

        // Monitor condition in main thread
        while !condition() {
            thread::sleep(Duration::from_nanos(100));
        }

        termination_flag.store(true, std::sync::atomic::Ordering::Relaxed);
        let mut results_lps = Vec::new();
        // Wait for all threads to complete
        for (i, handle) in handles.into_iter().enumerate() {
            match handle.join() {
                Ok(result) => {
                    if let Err(e) = result {
                        eprintln!("LP {i} terminated with error: {e:?}");
                        return Err(e);
                    }
                    results_lps.push(result.unwrap());
                }
                Err(_) => {
                    eprintln!("LP {i} thread panicked");
                    return Err(SimError::ThreadPanic);
                }
            }
        }

        // Wait for GVT thread
        let gvt = match gvt_handle.join() {
            Ok(result) => {
                if let Err(e) = result {
                    eprintln!("GVT thread terminated with error: {e:?}");
                    return Err(e);
                }
                result.unwrap()
            }
            Err(_) => {
                eprintln!("GVT thread panicked");
                return Err(SimError::ThreadPanic);
            }
        };
        let timewarp = TimeWarp {
            gvt,
            lps: results_lps,
        };
        println!("Time Warp simulation completed with condition met");
        Ok(timewarp)
    }

    pub fn num_lps(&self) -> usize {
        self.lps.len()
    }
}
