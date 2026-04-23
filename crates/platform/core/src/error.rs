//! Errors produced by platform backends.

use input_leap_common::ClipboardFormat;
use thiserror::Error;

/// Errors a backend may return from trait methods.
///
/// The variants are deliberately coarse — callers rarely need more than
/// "can we retry?" vs "give up and fall back to `MockScreen`". When in
/// doubt, prefer a structured variant over [`Self::Other`]; the latter
/// exists only as a safety net for truly unclassifiable failures.
#[derive(Debug, Error)]
pub enum PlatformError {
    /// The backend is unavailable on this host (no display, missing
    /// portal, scaffold implementation). Callers typically try the
    /// next backend in their fallback chain.
    #[error("platform backend unavailable: {0}")]
    Unavailable(String),

    /// An OS-level permission is missing (macOS Accessibility, uinput
    /// access, corporate policy). Not recoverable without user action.
    #[error("permission denied for operation `{operation}`")]
    PermissionDenied {
        /// Name of the trait method that needed the permission.
        operation: &'static str,
    },

    /// Lost the connection to the underlying display / system server
    /// (X connection died, libei socket closed, ...). A fresh
    /// `open`/`try_open` is usually needed.
    #[error("display connection lost: {source}")]
    ConnectionLost {
        /// Underlying backend-specific error; boxed to keep this enum
        /// object-size small and avoid a generic type parameter.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The backend does not support the requested clipboard format at
    /// all (as opposed to "the current selection has no such format",
    /// which is an empty [`bytes::Bytes`] return).
    #[error("clipboard format {format:?} not supported by this backend")]
    UnsupportedFormat {
        /// The format the caller asked for.
        format: ClipboardFormat,
    },

    /// Clipboard read/write timed out waiting for the selection owner
    /// to respond.
    #[error("clipboard operation timed out")]
    ClipboardTimeout,

    /// Fallback for failures that do not fit another variant. Prefer a
    /// structured variant when you can — `Other` is opaque to callers.
    #[error("platform error: {0}")]
    Other(String),
}

impl PlatformError {
    /// Build a [`Self::ConnectionLost`] from any error that implements
    /// the standard error traits. Convenience for backends that wrap
    /// third-party errors (x11rb, reis, ...).
    pub fn connection_lost<E>(err: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::ConnectionLost {
            source: Box::new(err),
        }
    }
}
