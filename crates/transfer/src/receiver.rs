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

use input_leap_common::{FileManifest, TransferCancelReason, TransferId};
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
            cleanup: Some(staging),
        })
    }

    /// Absorb one chunk. Opens the target file lazily on first chunk.
    pub async fn on_chunk(&mut self, entry_index: u32, data: &[u8]) -> Result<(), TransferError> {
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

        let expected = entry.size;
        let written = self.bytes_per_entry[idx];
        let incoming = data.len() as u64;
        if written + incoming > expected {
            return Err(TransferError::SizeOverflow {
                entry_index,
                overflow: (written + incoming) - expected,
            });
        }

        let open = match self.current_file.take() {
            Some((open_index, file)) if open_index == entry_index => Some(file),
            Some((open_index, _old)) => {
                // Moving on to a new file: the old one drops here,
                // closing its handle. Chunks for a previous entry
                // would be a protocol violation caught above, so this
                // arm only fires on a legitimate advance.
                let _ = open_index;
                None
            }
            None => None,
        };
        let mut file = match open {
            Some(f) => f,
            None => {
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
