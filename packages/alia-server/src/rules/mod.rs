//! Event-driven automation rule engine.
//!
//! Available implementations:
//! - `EventRuleEngine`: EventBus subscription + shared condition/action registries

pub mod engine;

pub use engine::EventRuleEngine;
