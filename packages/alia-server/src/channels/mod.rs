pub(crate) mod adapter;
pub(crate) mod cli;
#[cfg(feature = "channel-embedded")]
pub(crate) mod embedded;
pub(crate) mod pipeline;
pub(crate) mod registry;

pub use adapter::ChannelAdapter;
pub use pipeline::spawn_pipeline;
pub use registry::ChannelRegistry;
