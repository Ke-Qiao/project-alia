//! Cache backends for response caching.
//!
//! Cache is a fixed core service. Runtime policy decides whether entries are
//! used, while this module provides the database-backed implementation.

pub mod db;

pub use db::DbCache;
