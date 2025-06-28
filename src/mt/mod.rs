
pub mod optimistic;
pub mod conservative;

pub enum SyncMode {
    Conservative,
    Optimistic,
    Hybrid
}