//! Durable presentation facts; this module never replays recovered audio.
pub mod repository;
pub mod types;

mod service;
pub use service::{AudioDelivery, DeliveryService};
