//! Crate-wide error types.
//!
//! The ssh_config parse/write layer uses a typed [`ConfigError`]; the rest of
//! the app uses `anyhow::Result`. `ConfigError` converts into `anyhow::Error`
//! automatically (it implements `std::error::Error`).

use std::path::PathBuf;

/// Errors from the ssh_config parse/write layer.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot resolve home directory (~)")]
    NoHome,
    #[error("validation failed for field '{field}': {reason}")]
    Validation { field: String, reason: String },
    #[error("duplicate host alias '{0}'")]
    DuplicateAlias(String),
    #[error("read/write error on {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}
