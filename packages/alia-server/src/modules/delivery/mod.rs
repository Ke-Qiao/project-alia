//! Durable presentation facts; this module never replays recovered audio.
pub mod audio;
pub mod repository;
pub mod transport;
pub mod types;

mod service;
pub use service::{AudioDelivery, DeliveryService, OutputProducer, TextDelivery, VoiceOutput};
