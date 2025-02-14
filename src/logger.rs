use crate::worlds::Event;

/// A logger for recording snapshots of the world.
pub struct Logger {
    pub astates: Vec<History>,
    pub gstates: History,
    events: Vec<Event>,
}

pub struct History(pub Vec<(Vec<u8>, u64)>);

impl History {
    pub fn update(&mut self, new: Option<Vec<u8>>, state: &mut Option<Vec<u8>>, step: &u64) {
        if new.is_none() {
            return;
        }
        let result = std::mem::replace(state, new);
        self.0.push((result.unwrap(), *step));
    }
}

impl Logger {
    pub fn new() -> Self {
        Logger {
            astates: Vec::new(),
            gstates: History(Vec::new()),
            events: Vec::new(),
        }
    }

    pub fn log_global(&mut self, state: Vec<u8>, step: u64) {
        self.gstates.0.push((state, step));
    }
    pub fn log_event(&mut self, event: Event) {
        self.events.push(event);
    }

    pub fn get_events(&self) -> Vec<Event> {
        self.events.clone()
    }

    pub fn latest(&self) -> u64 {
        let mut last = if self.gstates.0.last().is_none() {
            &(Vec::new(), 0)
        } else {
            self.gstates.0.last().unwrap()
        };
        for i in 0..self.astates.len() {
            let astates = &self.astates[i].0;

            let last_astate = astates.last();
            if last_astate.is_none() {
                continue;
            }
            if last_astate.unwrap().1 > last.1 {
                last = last_astate.unwrap();
            }
        }
        last.1
    }
}