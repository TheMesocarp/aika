use std::cmp::Ordering;

#[derive(Debug, Clone)]
/// A message that can be sent between agents.
pub struct Message {
    pub data: Vec<u8>,
    pub timestamp: f64,
    pub from: usize,
    pub to: usize,
}

impl Message {
    pub fn new(data: Vec<u8>, timestamp: f64, from: usize, to: usize) -> Self {
        Message {
            data,
            timestamp,
            from,
            to,
        }
    }
}

impl PartialEq for Message {
    fn eq(&self, other: &Self) -> bool {
        self.timestamp == other.timestamp
    }
}

impl Eq for Message {}

impl PartialOrd for Message {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Message {
    fn cmp(&self, other: &Self) -> Ordering {
        self.timestamp.partial_cmp(&other.timestamp).unwrap()
    }
}
