/// Configuration for the world
#[derive(Clone)]
pub struct Config {
    pub timestep: f64,
    pub terminal: Option<f64>,
    pub buffer_size: usize,
    pub mailbox_size: usize,
    pub logs: bool,
}

impl Config {
    pub fn new(
        timestep: f64,
        terminal: Option<f64>,
        buffer_size: usize,
        mailbox_size: usize,
        logs: bool,
    ) -> Self {
        Config {
            timestep,
            terminal,
            buffer_size,
            mailbox_size,
            logs,
        }
    }
}
