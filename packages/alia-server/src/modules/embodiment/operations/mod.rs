//! Execution-authorized, persistent physical operations. No default device.
mod adapter;
mod repository;
mod service;
mod tools;

pub use adapter::{AdapterDescriptor, OperationRequest, PhysicalAdapter};
pub use alia_core::embodiment::operation::{
    OperationFeedback, OperationIdentity, OperationRecord, OperationStatus, StoppedEvidence,
};
pub use service::EmbodimentService;
