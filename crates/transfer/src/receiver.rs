//! Receiver-side of a file transfer.
//!
//! Lifecycle:
//!   1. `TransferReceiver::start(manifest, staging_root)` validates
//!      paths, creates `<staging_root>/<transfer_id>/`, and
//!      pre-creates all manifest directories so the first chunk for a
//!      nested file has a parent to land in.
//!   2. `on_chunk(entry_index, data)` appends `data` to the file at
//!      `entries[entry_index]`, enforcing per-file sizes from the
//!      manifest.
//!   3. `finish(drop_dir)` closes any open file, atomically renames
//!      staging into `<drop_dir>/<root-name>/`, and returns the list
//!      of final paths.
//!   4. `cancel(reason)` is called on abort; the staging directory is
//!      removed via a `Drop` guard so partial writes never escape.

use std::path::{Path, PathBuf};

use hop_common::{FileManifest, TransferCancelReason, TransferId};
use tokio::io::AsyncWriteExt;
use tracing::{info, warn};

use crate::error::TransferError;
use crate::path_guard::validate_rel_path;

/// Describes a file that actually landed in the drop directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedFile {
    /// Absolute path where the file now lives.
    pub path: PathBuf,
    /// Byte count that was written.
    pub size: u64,
}

/// Transfer-receiver state machine.
#[must_use = "call finish() or cancel() explicitly to finalize the transfer"]
pub struct TransferReceiver {
    transfer_id: TransferId,
    staging: PathBuf,
    manifest: FileManifest,
    absolute_paths: Vec<PathBuf>,
    bytes_per_entry: Vec<u64>,
    current_file: Option<(u32, tokio::fs::File)>,
    /// Most-recent `entry_index` we accepted a chunk for. Used to
    /// enforce strict monotonic ordering across entries — the sender
    /// is required to finish each file before advancing to the next.
    last_entry: Option<u32>,
    /// When `Some`, the staging dir is cleaned up on Drop. Cleared by
    /// [`finish`](Self::finish) on a successful finalise.
    cleanup: Option<PathBuf>,
}

impl std::fmt::Debug for TransferReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransferReceiver")
            .field("transfer_id", &self.transfer_id)
            .field("staging", &self.staging)
            .field("entry_count", &self.manifest.entries.len())
            .finish_non_exhaustive()
    }
}

impl TransferReceiver {
    /// Start a receiver. Validates every `rel_path` against traversal
    /// before any I/O on incoming chunks.
    pub async fn start(
        transfer_id: TransferId,
        manifest: FileManifest,
        staging_root: &Path,
    ) -> Result<Self, TransferError> {
        let staging = staging_root.join(format!("transfer-{transfer_id}"));
        tokio::fs::create_dir_all(&staging)
            .await
            .map_err(|source| TransferError::Io {
                path: staging.clone(),
                source,
            })?;

        // Validate every path first, then pre-create directories.
        let mut absolute_paths = Vec::with_capacity(manifest.entries.len());
        for entry in &manifest.entries {
            let abs = validate_rel_path(&staging, &entry.rel_path)?;
            absolute_paths.push(abs);
        }
        for (entry, abs) in manifest.entries.iter().zip(absolute_paths.iter()) {
            if entry.is_dir {
                tokio::fs::create_dir_all(abs)
                    .await
                    .map_err(|source| TransferError::Io {
                        path: abs.clone(),
                        source,
                    })?;
            } else if let Some(parent) = abs.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|source| TransferError::Io {
                        path: parent.to_path_buf(),
                        source,
                    })?;
            }
        }

        let bytes_per_entry = vec![0u64; manifest.entries.len()];

        Ok(Self {
            transfer_id,
            staging: staging.clone(),
            manifest,
            absolute_paths,
            bytes_per_entry,
            current_file: None,
            last_entry: None,
            cleanup: Some(staging),
        })
    }

    /// Absorb one chunk. Opens the target file lazily on the first
    /// chunk for that entry.
    ///
    /// The receiver enforces two invariants:
    ///
    /// - **Strict entry ordering.** `entry_index` must never decrease,
    ///   and may only advance once the previous non-directory entry
    ///   has received every byte the manifest promised. A sender that
    ///   interleaves files or skips ahead is rejected with
    ///   [`TransferError::OutOfOrderChunk`].
    /// - **Contiguous offsets.** `offset` must equal the number of
    ///   bytes already written for that entry. Any mismatch —
    ///   duplicate chunk, gap, reordering — surfaces as
    ///   [`TransferError::OffsetMismatch`].
    pub async fn on_chunk(
        &mut self,
        entry_index: u32,
        offset: u64,
        data: &[u8],
    ) -> Result<(), TransferError> {
        let total = u32::try_from(self.manifest.entries.len()).unwrap_or(u32::MAX);
        let idx = entry_index as usize;
        let entry = self
            .manifest
            .entries
            .get(idx)
            .ok_or(TransferError::UnexpectedChunk { entry_index, total })?;
        if entry.is_dir {
            return Err(TransferError::UnexpectedChunk { entry_index, total });
        }

        // Strict monotonic entry ordering.
        if let Some(last) = self.last_entry {
            if entry_index < last {
                return Err(TransferError::OutOfOrderChunk {
                    entry_index,
                    last_seen: last,
                });
            }
            if entry_index > last {
                // Sender advanced to a new entry — the previous one
                // must have been completed byte-for-byte, and we need
                // to flush/close its open handle before opening the
                // next.
                let prev = last as usize;
                let prev_entry = &self.manifest.entries[prev];
                if !prev_entry.is_dir && self.bytes_per_entry[prev] != prev_entry.size {
                    return Err(TransferError::OutOfOrderChunk {
                        entry_index,
                        last_seen: last,
                    });
                }
                if let Some((_, mut old)) = self.current_file.take() {
                    old.flush()
                        .await
                        .map_err(|source| TransferError::Io {
                            path: self.absolute_paths[prev].clone(),
                            source,
                        })?;
                }
            }
        }

        // Contiguous offset within the entry.
        let written = self.bytes_per_entry[idx];
        if offset != written {
            return Err(TransferError::OffsetMismatch {
                entry_index,
                expected: written,
                got: offset,
            });
        }

        let expected = entry.size;
        let incoming = data.len() as u64;
        if written + incoming > expected {
            return Err(TransferError::SizeOverflow {
                entry_index,
                overflow: (written + incoming) - expected,
            });
        }

        let mut file = match self.current_file.take() {
            Some((open_index, f)) if open_index == entry_index => f,
            _ => {
                let path = &self.absolute_paths[idx];
                tokio::fs::File::create(path)
                    .await
                    .map_err(|source| TransferError::Io {
                        path: path.clone(),
                        source,
                    })?
            }
        };
        file.write_all(data)
            .await
            .map_err(|source| TransferError::Io {
                path: self.absolute_paths[idx].clone(),
                source,
            })?;
        self.bytes_per_entry[idx] = written + incoming;
        self.current_file = Some((entry_index, file));
        self.last_entry = Some(entry_index);
        Ok(())
    }

    /// Finalise the transfer: verify sizes, flush, move staging into
    /// the drop directory. Returns the final absolute paths in
    /// manifest order.
    pub async fn finish(mut self, drop_dir: &Path) -> Result<Vec<ReceivedFile>, TransferError> {
        // Flush whatever file is still open.
        if let Some((_, mut file)) = self.current_file.take() {
            file.flush().await.map_err(|source| TransferError::Io {
                path: self.staging.clone(),
                source,
            })?;
        }

        // Verify every entry got the bytes the manifest promised.
        for (idx, entry) in self.manifest.entries.iter().enumerate() {
            if entry.is_dir {
                continue;
            }
            let written = self.bytes_per_entry[idx];
            if written != entry.size {
                return Err(TransferError::Incomplete {
                    entry_index: u32::try_from(idx).unwrap_or(u32::MAX),
                    missing: entry.size - written,
                });
            }
        }

        // Move everything under drop_dir/<staging's leaf>/.
        tokio::fs::create_dir_all(drop_dir)
            .await
            .map_err(|source| TransferError::Io {
                path: drop_dir.to_path_buf(),
                source,
            })?;
        let leaf = self
            .staging
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("transfer"));
        let final_base = unique_name(drop_dir, leaf).await?;
        tokio::fs::rename(&self.staging, &final_base)
            .await
            .map_err(|source| TransferError::Io {
                path: final_base.clone(),
                source,
            })?;

        // Drop guard is no longer responsible for cleanup.
        self.cleanup = None;

        // Translate absolute_paths (under staging) to their new home.
        let mut final_files = Vec::with_capacity(self.manifest.entries.len());
        for (entry, old_abs) in self.manifest.entries.iter().zip(self.absolute_paths.iter()) {
            if entry.is_dir {
                continue;
            }
            let rel = old_abs.strip_prefix(&self.staging).unwrap_or(old_abs);
            final_files.push(ReceivedFile {
                path: final_base.join(rel),
                size: entry.size,
            });
        }
        info!(
            transfer_id = self.transfer_id,
            files = final_files.len(),
            base = %final_base.display(),
            "transfer finalised"
        );
        Ok(final_files)
    }

    /// Abort the transfer. Staging is wiped by the `Drop` guard.
    pub fn cancel(mut self, reason: TransferCancelReason) {
        warn!(
            transfer_id = self.transfer_id,
            ?reason,
            staging = %self.staging.display(),
            "transfer cancelled"
        );
        drop(self.current_file.take());
    }
}

impl Drop for TransferReceiver {
    fn drop(&mut self) {
        if let Some(path) = self.cleanup.take() {
            drop(self.current_file.take());
            // Synchronous cleanup on the current thread; running tokio
            // fs from Drop is unsafe because we don't know the runtime
            // state. `std::fs` is fine for a best-effort rm -rf.
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

async fn unique_name(dir: &Path, leaf: &std::ffi::OsStr) -> Result<PathBuf, TransferError> {
    let candidate = dir.join(leaf);
    if tokio::fs::try_exists(&candidate).await.unwrap_or(false) {
        for suffix in 1..=128 {
            let mut name = leaf.to_os_string();
            name.push(format!("_{suffix}"));
            let candidate = dir.join(&name);
            if !tokio::fs::try_exists(&candidate).await.unwrap_or(false) {
                return Ok(candidate);
            }
        }
        return Err(TransferError::Io {
            path: candidate,
            source: std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "drop directory contains too many same-named transfers",
            ),
        });
    }
    Ok(candidate)
}
