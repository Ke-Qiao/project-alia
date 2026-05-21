#[doc(hidden)]
pub mod adapter;
pub(crate) mod cli;
pub(crate) mod dialogue_control_tool;
#[cfg(feature = "channel-embedded")]
#[doc(hidden)]
pub mod embedded;
#[cfg(feature = "channel-embedded")]
pub(crate) mod embedded_volume_tool;
#[doc(hidden)]
pub mod pipeline;
pub(crate) mod registry;

pub use adapter::ChannelAdapter;
pub use pipeline::{PipelinePolicies, SharedPipelinePolicies, spawn_pipeline};
pub use registry::ChannelRegistry;
