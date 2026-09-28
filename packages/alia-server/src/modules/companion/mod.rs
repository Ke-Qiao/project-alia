//! Source-bound initiative candidates. Shadow decisions are never output permits.
pub mod decision;
pub(crate) mod live;
pub mod repository;
mod runner;
mod service;
pub mod types;

pub(crate) use runner::LiveOutputs;
pub use service::CompanionService;
