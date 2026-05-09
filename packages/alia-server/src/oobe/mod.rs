//! Out-of-Box Experience (OOBE) — first-run setup flow.
//!
//! When no config file is found, the server enters OOBE mode:
//! - Starts a minimal HTTP server on port 3000 with setup API
//! - Prints CLI instructions (press `c` to release example config)
//! - Waits for either frontend or CLI completion before normal startup

pub mod bootstrap;
pub mod routes;
