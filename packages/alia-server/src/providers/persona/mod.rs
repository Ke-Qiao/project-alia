//! Persona providers for AI personality and prompt management.
//!
//! Persona rendering is a fixed core service because prompts are user-facing
//! state, not a provider slot. Implementations should extend the template
//! renderer instead of replacing the whole service.

pub mod template;

pub use template::TemplatePersona;
