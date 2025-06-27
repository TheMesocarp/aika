use mesocarp::{comms::mailbox::Message, scheduling::Scheduleable};

#[derive(Clone, Debug)]
pub struct Msg<T: Clone> {
    from_id: usize,
    to_id: Option<usize>,
    commit_time: u64,
    recv_time: u64,
    data: T
}

impl<T: Clone> Message for Msg<T> {
    fn to(&self) -> Option<usize> {
        self.to_id
    }

    fn from(&self) -> usize {
        self.from_id
    }

    fn broadcast(&self) -> bool {
        if self.to_id.is_none() {
            true
        } else {
            false
        }
    }
}

impl<T: Clone> Scheduleable for Msg<T> {
    fn time(&self) -> u64 {
        self.recv_time
    }

    fn commit_time(&self) -> u64 {
        self.commit_time
    }
}

impl<T: Clone> PartialOrd for Msg<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.recv_time.partial_cmp(&other.recv_time)
    }
}

impl<T: Clone> PartialEq for Msg<T> {
    fn eq(&self, other: &Self) -> bool {
        self.from_id == other.from_id && self.to_id == other.to_id && self.commit_time == other.commit_time && self.recv_time == other.recv_time
    }
}

impl<T: Clone> Eq for Msg<T> {}

impl<T: Clone> Ord for Msg<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.recv_time.cmp(&other.recv_time)
    }
}