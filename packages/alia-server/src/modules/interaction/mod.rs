//! Conversation use cases and bounded turn supervision. Channels supply I/O contracts;
//! this module owns generation, cancellation, context and response orchestration.
pub mod cache;
mod context;
mod debug;
mod delivery;
mod dialogue_policy;
mod error_notice;
mod expression;
mod generation;
pub mod initiative;
#[cfg(test)]
mod integration_tests;
pub mod llm;
pub mod persistence;
pub mod policy;
pub mod prompt;
mod runtime;
mod services;
pub mod streaming;
mod supervision;
pub mod tools;
mod trace;
mod turn;
pub mod turn_detector;
mod voice;
pub mod voice_input;

pub use policy::{InteractionPolicies, SharedInteractionPolicies};
pub use runtime::spawn_interaction;
pub use services::InteractionServices;

pub mod conversation;
pub mod persona;
pub mod routing;
pub mod skills;
