//! Error types for gamebus-presenced.

use thiserror::Error;

/// Main error type for the presence daemon.
#[derive(Debug, Error)]
pub enum Error {
    /// D-Bus connection error.
    #[error("D-Bus connection error: {0}")]
    Connection(#[from] zbus::Error),

    /// D-Bus interface error.
    #[error("D-Bus interface error: {0}")]
    Interface(#[from] zbus::fdo::Error),

    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result type for gamebus-presenced operations.
pub type Result<T> = std::result::Result<T, Error>;
