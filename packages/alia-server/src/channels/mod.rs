#[doc(hidden)]
pub mod adapter;
pub(crate) mod cli;
#[cfg(feature = "channel-embedded")]
#[doc(hidden)]
pub mod embedded;
#[cfg(feature = "channel-embedded")]
pub(crate) mod embedded_volume_tool;
pub(crate) mod registry;

pub use adapter::ChannelAdapter;
pub use registry::ChannelRegistry;
