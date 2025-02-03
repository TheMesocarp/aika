use std::collections::BTreeMap;

/// A snapshot of the world at a given time.
#[derive(Clone)]
pub struct Snapshot {
    pub timestamp: f64,
    pub shared_state: Option<Vec<u8>>,
    pub agent_states: BTreeMap<usize, Vec<u8>>,
}

impl PartialEq for Snapshot {
    fn eq(&self, other: &Self) -> bool {
        self.timestamp == other.timestamp
    }
}

impl Eq for Snapshot {}

impl PartialOrd for Snapshot {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Snapshot {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.timestamp.partial_cmp(&other.timestamp).unwrap()
    }
}
