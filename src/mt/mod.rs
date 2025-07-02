pub mod conservative;
pub mod optimistic;
pub mod optim;

pub enum SyncMode {
    Conservative,
    Optimistic,
    Hybrid,
}
