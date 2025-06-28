pub struct LPConfig {
    pub horizon: Option<u64>,
    pub timestep: f64,
    pub terminal: f64,
    pub state_arena_size: usize,
    pub anti_msg_arena_size: usize,
}

impl LPConfig {
    pub fn new(
        state_arena_size: usize,
        anti_msg_arena_size: usize,
        horizon: Option<u64>,
        timestep: f64,
        terminal: f64,
    ) -> Self {
        Self {
            horizon,
            timestep,
            terminal,
            state_arena_size,
            anti_msg_arena_size,
        }
    }
}
