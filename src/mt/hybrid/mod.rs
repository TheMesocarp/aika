//! `aika::mt::hybrid` contains the infrastructure for running hybrid synchronization

use bytemuck::{Pod, Zeroable};

use crate::{
    agents::ThreadedAgent,
    mt::hybrid::{config::HybridConfig, galaxy::Galaxy, planet::Planet},
    SimError,
};

pub mod config;
pub mod galaxy;
pub mod planet;

pub struct HybridEngine<
    const INTER_SLOTS: usize,
    const CLOCK_SLOTS: usize,
    const CLOCK_HEIGHT: usize,
    MessageType: Pod + Zeroable + Clone,
> {
    pub galaxy: Galaxy<INTER_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>,
    pub planets: Vec<Planet<INTER_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>>,
    pub config: HybridConfig,
}

impl<
        const INTER_SLOTS: usize,
        const CLOCK_SLOTS: usize,
        const CLOCK_HEIGHT: usize,
        MessageType: Pod + Zeroable + Clone,
    > HybridEngine<INTER_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
    pub fn create(config: HybridConfig) -> Result<Self, SimError> {
        let mut galaxy = Galaxy::new(
            config.number_of_worlds,
            config.throttle_horizon,
            config.checkpoint_frequency,
            config.terminal,
            config.timestep,
        )?;
        let mut planets = Vec::new();
        for i in 0..config.number_of_worlds {
            let registry = galaxy.spawn_world()?;
            let planet = Planet::from_config(
                config.world_config(i)?,
                config.terminal,
                config.timestep,
                config.throttle_horizon,
                registry,
            )?;
            planets.push(planet);
        }
        Ok(Self {
            galaxy,
            planets,
            config,
        })
    }

    pub fn spawn_agent(
        &mut self,
        planet_id: usize,
        agent: Box<dyn ThreadedAgent<INTER_SLOTS, MessageType>>,
    ) -> Result<(), SimError> {
        if planet_id >= self.planets.len() {
            return Err(SimError::InvalidWorldId(planet_id));
        }
        self.planets[planet_id].spawn_agent_preconfigured(agent);
        Ok(())
    }

    pub fn spawn_agent_autobalance(
        &mut self,
        agent: Box<dyn ThreadedAgent<INTER_SLOTS, MessageType>>,
    ) -> Result<(), SimError> {
        let mut lowest = (usize::MAX, usize::MAX);
        for (i, planet) in self.planets.iter().enumerate() {
            let count = planet.agents.len();
            if count < lowest.1 {
                lowest = (i, count)
            }
        }
        self.planets[lowest.0].spawn_agent_preconfigured(agent);
        Ok(())
    }

    pub fn schedule(
        &mut self,
        planet_id: usize,
        agent_id: usize,
        time: u64,
    ) -> Result<(), SimError> {
        if planet_id >= self.planets.len() {
            return Err(SimError::InvalidWorldId(planet_id));
        }
        self.planets[planet_id].schedule(time, agent_id)
    }

    pub fn run(self) -> Result<Self, SimError> {
        let HybridEngine {
            galaxy,
            planets,
            config,
        } = self;
        let galaxy_handle = std::thread::spawn(move || {
            let mut galaxy = galaxy;
            galaxy.gvt_daemon().map(|_| galaxy)
        });

        let mut planet_handles = Vec::new();
        for planet in planets {
            let handle = std::thread::spawn(move || {
                let mut planet = planet;
                planet.run().map(|_| planet)
            });
            planet_handles.push(handle);
        }

        let mut final_planets = Vec::new();
        for handle in planet_handles {
            let planet = handle.join().map_err(|_| SimError::ThreadPanic)??;
            final_planets.push(planet);
        }

        let final_galaxy = galaxy_handle.join().map_err(|_| SimError::ThreadPanic)??;

        Ok(Self {
            galaxy: final_galaxy,
            planets: final_planets,
            config,
        })
    }
}
