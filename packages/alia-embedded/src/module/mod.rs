pub mod builder;
pub mod context;
pub mod cpal_audio;
pub mod direct_response;
pub mod discovery;
pub mod factory;
pub mod lifecycle;
pub mod noop;
pub mod registry;
#[cfg(feature = "sherpa-onnx")]
pub mod sherpa;
pub mod speech;
pub mod traits;

pub use builder::EmbeddedBuilder;
