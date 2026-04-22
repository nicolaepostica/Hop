//! Errors the transfer engine can surface to callers.

use std::path::PathBuf;

use input_leap_common::TransferCancelReason;
use thiserror::Error;

/// Everything that can go wrong during a transfer.
#[derive(Debug, Error)]
pub enum TransferError {
    /// Filesystem I/O failure (read source / write destination).
    #[error("io error at {path}: {source}")]
    Io {
        /// Path where the error occurred.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// A manifest entry tried to escape the staging directory (`..`,
    /// absolute path, or Windows drive letter).
    #[error("path traversal rejected: {rel_path}")]
    PathTraversal {
        /// The offending `rel_path` from the manifest.
        rel_path: PathBuf,
    },

    /// Wrong chunk targeted for the current receiver state.
    #[error("unexpected chunk for entry {entry_index} (manifest has {total})")]
    UnexpectedChunk {
        /// Index the sender supplied.
        entry_index: u32,
        /// Total number of entries in the manifest.
        total: u32,
    },

    /// A chunk would push a file past its manifest-declared size.
    #[error("chunk for entry {entry_index} overflows manifest size by {overflow} bytes")]
    SizeOverflow {
        /// Which entry.
        entry_index: u32,
        /// How many bytes beyond the declared size this chunk would add.
        overflow: u64,
    },

    /// Caller tried to finalise with missing bytes.
    #[error("entry {entry_index} short by {missing} bytes at end of transfer")]
    Incomplete {
        /// Which entry is short.
        entry_index: u32,
        /// How many bytes we still expected.
        missing: u64,
    },

    /// Transfer exceeded the configured `max_transfer_bytes`.
    #[error("transfer would send {size} bytes, over limit {max}")]
    TooLarge {
        /// Total manifest bytes (what the sender wants to send).
        size: u64,
        /// Limit from config.
        max: u64,
    },

    /// Sender observed that a source file changed size mid-read.
    #[error("source file {path} changed size mid-read (manifest said {expected}, read {got})")]
    SourceChanged {
        /// Path being read.
        path: PathBuf,
        /// Size the manifest announced.
        expected: u64,
        /// Size actually read.
        got: u64,
    },
}

impl TransferError {
    /// Best-effort mapping to a `TransferCancelReason` for the
    /// wire-level cancel message.
    #[must_use]
    pub fn cancel_reason(&self) -> TransferCancelReason {
        match self {
            Self::PathTraversal { .. } => TransferCancelReason::PathTraversal,
            Self::SourceChanged { .. } => TransferCancelReason::SizeMismatch,
            Self::TooLarge { .. } => TransferCancelReason::TooLarge,
            Self::Io { source, .. } if is_storage_full(source) => TransferCancelReason::DiskFull,
            _ => TransferCancelReason::PeerError,
        }
    }
}

/// `io::ErrorKind::StorageFull` was only stabilised in Rust 1.83; our
/// MSRV is 1.75. Match on the raw OS errno instead (`ENOSPC = 28` on
/// Linux) so callers get the right cancel reason without bumping the
/// minimum toolchain.
fn is_storage_full(err: &std::io::Error) -> bool {
    err.raw_os_error() == Some(28)
}
