use crate::SimError;

#[derive(Debug, Clone)]
pub struct HybridConfig {
    pub number_of_worlds: usize,
    pub world_state_asizes: Vec<usize>,
    pub agent_states_asizes: Vec<Vec<usize>>,
    pub anti_message_asize: usize,
    pub throttle_horizon: u64,
    pub checkpoint_frequency: u64,
    pub terminal: f64,
    pub timestep: f64,
}

impl HybridConfig {
    /// Create a new configuration with the specified number of worlds and anti-message arena size
    pub fn new(number_of_worlds: usize, anti_message_asize: usize) -> Self {
        Self {
            number_of_worlds,
            world_state_asizes: vec![0; number_of_worlds],
            agent_states_asizes: vec![Vec::new(); number_of_worlds],
            anti_message_asize,
            throttle_horizon: 0,
            checkpoint_frequency: 0,
            terminal: 0.0,
            timestep: 0.0,
        }
    }

    /// Configure simulation time bounds
    pub fn with_time_bounds(mut self, terminal: f64, timestep: f64) -> Self {
        self.terminal = terminal;
        self.timestep = timestep;
        self
    }

    /// Configure optimistic synchronization parameters
    pub fn with_optimistic_sync(
        mut self,
        throttle_horizon: u64,
        checkpoint_frequency: u64,
    ) -> Self {
        self.throttle_horizon = throttle_horizon;
        self.checkpoint_frequency = checkpoint_frequency;
        self
    }

    /// Configure a specific world's state and agent arena sizes
    pub fn with_world(
        mut self,
        world_id: usize,
        world_state_size: usize,
        agent_state_sizes: Vec<usize>,
    ) -> Result<Self, SimError> {
        if world_id >= self.number_of_worlds {
            return Err(SimError::InvalidWorldId(world_id));
        }

        self.world_state_asizes[world_id] = world_state_size;
        self.agent_states_asizes[world_id] = agent_state_sizes;
        Ok(self)
    }

    pub fn with_uniform_worlds(
        mut self,
        world_state_size: usize,
        agents_per_world: usize,
        agent_state_size: usize,
    ) -> Self {
        for i in 0..self.number_of_worlds {
            self.world_state_asizes[i] = world_state_size;
            self.agent_states_asizes[i] = vec![agent_state_size; agents_per_world];
        }
        self
    }

    pub fn add_agent_to_world(
        mut self,
        world_id: usize,
        agent_state_size: usize,
    ) -> Result<Self, SimError> {
        if world_id >= self.number_of_worlds {
            return Err(SimError::InvalidWorldId(world_id));
        }

        self.agent_states_asizes[world_id].push(agent_state_size);
        Ok(self)
    }

    pub fn total_agents(&self) -> usize {
        self.agent_states_asizes
            .iter()
            .map(|agents| agents.len())
            .sum()
    }

    /// Validate that all required fields have been configured
    pub fn validate(&self) -> Result<(), SimError> {
        if self.terminal <= 0.0 {
            return Err(SimError::ConfigError(
                "Terminal time must be positive".to_string(),
            ));
        }

        if self.timestep <= 0.0 {
            return Err(SimError::ConfigError(
                "Timestep must be positive".to_string(),
            ));
        }

        if self.throttle_horizon == 0 {
            return Err(SimError::ConfigError(
                "Throttle horizon must be set".to_string(),
            ));
        }

        if self.checkpoint_frequency == 0 {
            return Err(SimError::ConfigError(
                "Checkpoint frequency must be set".to_string(),
            ));
        }

        // Check that all worlds have been configured
        for (i, world_size) in self.world_state_asizes.iter().enumerate() {
            if *world_size == 0 {
                return Err(SimError::ConfigError(format!(
                    "World {i} state size not configured"
                )));
            }
        }

        Ok(())
    }

    /// Get configuration for a specific world
    pub fn world_config(&self, world_id: usize) -> Result<(usize, usize, &Vec<usize>), SimError> {
        if world_id >= self.number_of_worlds {
            return Err(SimError::InvalidWorldId(world_id));
        }
        Ok((
            self.world_state_asizes[world_id],
            self.anti_message_asize,
            &self.agent_states_asizes[world_id],
        ))
    }
}
