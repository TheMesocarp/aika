use crate::worlds::Agent;
use crate::worlds::Event;
use crate::clock::Clock;

pub struct LP<const SLOTS: usize, const HEIGHT: usize> {
    pub agent: Box<dyn Agent>,
    pub scheduler: Clock<Event, SLOTS, HEIGHT>,

}