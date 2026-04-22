//! Errors produced by platform backends.

use thiserror::Error;

/// Errors a backend may return from trait methods.
#[derive(Debug, Error)]
pub enum PlatformError {
    /// The backend is unavailable (e.g. no display, permission denied).
    #[error("platform backend unavailable: {0}")]
    Unavailable(String),

    /// A backend-specific failure that does not fit the other variants.
    /// Wrapped so the caller can log it but does not need to interpret it.
    #[error("platform error: {0}")]
    Other(String),
}
