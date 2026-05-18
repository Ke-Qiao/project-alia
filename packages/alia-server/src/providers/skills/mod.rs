//! Skills module: extend what the LLM **knows how to do well**.
//!
//! Skills provide domain knowledge and routing context. Tool bindings are
//! injected into the LLM separately; skills are never executable functions.
//!
//! This module provides:
//! - `BuiltinSkills`: Full DB-backed implementation with markdown file support

pub mod builtin;
pub mod markdown;

pub use builtin::BuiltinSkills;
