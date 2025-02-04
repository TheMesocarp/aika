/// Configuration for the world
#[derive(Clone)]
pub struct Config {
    pub timestep: f64,
    pub terminal: Option<f64>,
    pub buffer_size: usize,
    pub mailbox_size: usize,
    pub live: bool,
    pub logs: bool,
    pub mail: bool,
}

impl Config {
    pub fn new(
        timestep: f64,
        terminal: Option<f64>,
        buffer_size: usize,
        mailbox_size: usize,
        live: bool,
        logs: bool,
        mail: bool,
    ) -> Self {
        Config {
            timestep,
            terminal,
            buffer_size,
            mailbox_size,
            live,
            logs,
            mail,
        }
    }
}
