use std::cmp::Ordering;

#[derive(Debug, Clone)]
/// A message that can be sent between agents.
pub struct Message<'a> {
    pub data: &'a [u8],
    pub timestamp: f64,
    pub from: usize,
    pub to: usize,
}

impl<'a> Message<'a> {
    pub fn new(data: &'a [u8], timestamp: f64, from: usize, to: usize) -> Self {
        Message {
            data,
            timestamp,
            from,
            to,
        }
    }
}

impl<'a> PartialEq for Message<'a> {
    fn eq(&self, other: &Self) -> bool {
        self.timestamp == other.timestamp
    }
}

impl<'a> Eq for Message<'a> {}

impl<'a> PartialOrd for Message<'a> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<'a> Ord for Message<'a> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.timestamp.partial_cmp(&other.timestamp).unwrap()
    }
}
