//! Source-bound initiative candidates. Shadow decisions are never output permits.
pub mod decision;
pub(crate) mod invitation;
pub(crate) mod live;
pub mod repository;
mod runner;
mod service;
pub(crate) mod source;
pub mod types;

pub(crate) use runner::LiveOutputs;
pub use service::CompanionService;
