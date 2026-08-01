//! OS integration: external `ssh` processes, key tooling, and network probing.
//! These modules have no ratatui dependency.

pub mod agent;
pub mod askpass;
pub mod binaries;
pub mod clipboard;
pub mod connect;
pub mod deploy;
pub mod history;
pub mod keys;
pub mod keyscan;
pub mod known_hosts;
pub mod liveness;
pub mod prefs;
pub mod resolve;
pub mod sftp;
pub mod vault;

pub use binaries::{ssh_dir, tools};
