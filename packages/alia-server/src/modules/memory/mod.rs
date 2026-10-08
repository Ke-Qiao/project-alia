//! Canonical memory, provenance, privacy transitions, and retrieval projections.
//! External model adapters supply embeddings or extraction; this module owns
//! what is stored and the transaction that authorizes each mutation.

pub mod capture;
pub mod context;
pub mod policy;
mod privacy;
mod service;
pub mod source;
mod types;

pub use service::MemoryService;
pub use types::*;

pub(crate) mod maintenance;
