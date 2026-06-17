//! OS integration: external `ssh` processes, key tooling, and network probing.
//! These modules have no ratatui dependency.

pub mod askpass;
pub mod binaries;
pub mod connect;
pub mod keys;
pub mod known_hosts;
pub mod liveness;
pub mod vault;

pub use binaries::{ssh_dir, tools};
