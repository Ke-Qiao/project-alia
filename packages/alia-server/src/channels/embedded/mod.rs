mod audio;
mod auth;
mod channel;
mod direct_response;
mod sensor;
mod settings;

pub use audio::{normalize_embedded_playback_payload, pcm16_to_le_bytes, pcm16le_bytes_to_samples};
pub use channel::{EmbeddedChannel, EmbeddedChannelConfig};
pub use sensor::{
    embedded_sensor_public_id, embedded_sensor_remote_id, ingest_embedded_sensor_reading,
    mirror_embedded_sensor_definitions,
};
