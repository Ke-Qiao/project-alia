//! Durable live admission. Only an acknowledged fresh commit creates a permit.
mod repository;
pub(crate) mod types;
pub(crate) use repository::LiveRepository;
pub(crate) use types::*;
#[cfg(test)]
pub(crate) mod tests;
