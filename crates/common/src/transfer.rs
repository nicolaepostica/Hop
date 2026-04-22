//! Types describing a file-clipboard transfer (M9).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Unique identifier for a single transfer, monotonic per connection.
pub type TransferId = u64;

/// One file or directory in a manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileManifestEntry {
    /// Path relative to the manifest's logical root. Must not contain
    /// `..`, absolute-path markers, or empty components; the receiver
    /// rejects otherwise.
    pub rel_path: PathBuf,
    /// Size in bytes. `0` for directories.
    pub size: u64,
    /// `true` if this entry is a directory (no bytes, create the dir).
    pub is_dir: bool,
}

/// Ordered list of entries in a single transfer.
///
/// The receiver pre-allocates staging directories from this manifest
/// and validates every `rel_path` *before* reading any chunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileManifest {
    /// One entry per file or directory, in the order the sender will
    /// stream them.
    pub entries: Vec<FileManifestEntry>,
    /// Total payload byte count (sum of file sizes).
    pub total_bytes: u64,
}

/// Reason a transfer was aborted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferCancelReason {
    /// The local user cancelled (GUI button, config change).
    UserCancelled,
    /// Receiver ran out of disk space.
    DiskFull,
    /// Source file changed size while we were reading it.
    SizeMismatch,
    /// Peer reported an error of its own.
    PeerError,
    /// A `rel_path` escaped the staging directory.
    PathTraversal,
    /// Transfer exceeded `max_transfer_bytes` from config.
    TooLarge,
    /// Catch-all for reasons a future peer might invent.
    #[serde(other)]
    Unknown,
}
