pub mod conservative;
pub mod optimistic;

pub enum SyncMode {
    Conservative,
    Optimistic,
    Hybrid,
}
