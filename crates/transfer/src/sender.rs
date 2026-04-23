//! Server-side of a file transfer: walk a directory tree, stream chunks.

use std::path::{Path, PathBuf};

use bytes::Bytes;
use input_leap_common::{FileManifest, FileManifestEntry, TransferId};
use input_leap_protocol::Message;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tracing::debug;

use crate::error::TransferError;

/// Default chunk size. Matches `FileTransferSettings::chunk_bytes`.
pub const DEFAULT_CHUNK_BYTES: usize = 64 * 1024;

/// Drives a single outbound file transfer.
#[derive(Debug)]
pub struct TransferSender {
    transfer_id: TransferId,
    chunk_size: usize,
}

impl TransferSender {
    /// Build a sender with the given `transfer_id` and chunk size.
    #[must_use]
    pub const fn new(transfer_id: TransferId, chunk_size: usize) -> Self {
        Self {
            transfer_id,
            chunk_size,
        }
    }

    /// Build a sender using [`DEFAULT_CHUNK_BYTES`].
    #[must_use]
    pub const fn with_default_chunking(transfer_id: TransferId) -> Self {
        Self::new(transfer_id, DEFAULT_CHUNK_BYTES)
    }

    /// Walk `root`, build a manifest, then stream start + chunks + end
    /// over `tx`. Returns the total byte count that was read.
    pub async fn send_directory(
        &self,
        root: &Path,
        clipboard_seq: u32,
        max_transfer_bytes: u64,
        tx: &mpsc::Sender<Message>,
    ) -> Result<u64, TransferError> {
        let (manifest, file_paths) = build_manifest(root).await?;

        if manifest.total_bytes > max_transfer_bytes {
            return Err(TransferError::TooLarge {
                size: manifest.total_bytes,
                max: max_transfer_bytes,
            });
        }

        send(
            tx,
            Message::FileTransferStart {
                transfer_id: self.transfer_id,
                clipboard_seq,
                manifest: manifest.clone(),
            },
        )
        .await?;

        let mut total_sent = 0u64;
        for (index, entry) in manifest.entries.iter().enumerate() {
            if entry.is_dir {
                continue;
            }
            let path = &file_paths[index];
            let index_u32 = u32::try_from(index).expect("manifest.entries fits in u32");
            total_sent += self
                .send_file_chunks(&manifest, index_u32, entry, path, tx)
                .await?;
        }

        send(
            tx,
            Message::FileTransferEnd {
                transfer_id: self.transfer_id,
            },
        )
        .await?;
        Ok(total_sent)
    }

    async fn send_file_chunks(
        &self,
        _manifest: &FileManifest,
        entry_index: u32,
        entry: &FileManifestEntry,
        path: &Path,
        tx: &mpsc::Sender<Message>,
    ) -> Result<u64, TransferError> {
        let mut file = tokio::fs::File::open(path)
            .await
            .map_err(|source| TransferError::Io {
                path: path.to_path_buf(),
                source,
            })?;

        let mut bytes_read = 0u64;
        let mut buf = vec![0u8; self.chunk_size];
        loop {
            let n = file
                .read(&mut buf)
                .await
                .map_err(|source| TransferError::Io {
                    path: path.to_path_buf(),
                    source,
                })?;
            if n == 0 {
                break;
            }
            let chunk = Bytes::copy_from_slice(&buf[..n]);
            // `offset` is the absolute position of the first byte in
            // `chunk` within the entry, so the receiver can validate
            // that chunks arrive contiguously and in order.
            send(
                tx,
                Message::FileChunk {
                    transfer_id: self.transfer_id,
                    entry_index,
                    offset: bytes_read,
                    data: chunk,
                },
            )
            .await?;
            bytes_read += n as u64;
            if bytes_read > entry.size {
                return Err(TransferError::SourceChanged {
                    path: path.to_path_buf(),
                    expected: entry.size,
                    got: bytes_read,
                });
            }
        }
        if bytes_read != entry.size {
            return Err(TransferError::SourceChanged {
                path: path.to_path_buf(),
                expected: entry.size,
                got: bytes_read,
            });
        }
        debug!(
            entry_index,
            path = %path.display(),
            bytes = bytes_read,
            "streamed file"
        );
        Ok(bytes_read)
    }
}

async fn send(tx: &mpsc::Sender<Message>, msg: Message) -> Result<(), TransferError> {
    // The receiver task hang-up is treated as an I/O failure on the
    // transport path we would have written.
    tx.send(msg).await.map_err(|_| TransferError::Io {
        path: PathBuf::new(),
        source: std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "transfer channel closed mid-stream",
        ),
    })
}

/// Walk `root` recursively, producing a manifest + parallel list of
/// absolute source paths (one per entry; empty `PathBuf` for dirs).
async fn build_manifest(root: &Path) -> Result<(FileManifest, Vec<PathBuf>), TransferError> {
    let mut entries = Vec::new();
    let mut paths = Vec::new();
    let mut stack: Vec<(PathBuf, PathBuf)> = vec![(root.to_path_buf(), PathBuf::new())];

    while let Some((abs, rel)) = stack.pop() {
        let meta = tokio::fs::metadata(&abs)
            .await
            .map_err(|source| TransferError::Io {
                path: abs.clone(),
                source,
            })?;
        if meta.is_dir() {
            if !rel.as_os_str().is_empty() {
                entries.push(FileManifestEntry {
                    rel_path: rel.clone(),
                    size: 0,
                    is_dir: true,
                });
                paths.push(PathBuf::new());
            }
            let mut reader =
                tokio::fs::read_dir(&abs)
                    .await
                    .map_err(|source| TransferError::Io {
                        path: abs.clone(),
                        source,
                    })?;
            let mut children: Vec<(PathBuf, PathBuf)> = Vec::new();
            while let Some(child) =
                reader
                    .next_entry()
                    .await
                    .map_err(|source| TransferError::Io {
                        path: abs.clone(),
                        source,
                    })?
            {
                let name = child.file_name();
                let child_abs = abs.join(&name);
                let child_rel = if rel.as_os_str().is_empty() {
                    PathBuf::from(&name)
                } else {
                    rel.join(&name)
                };
                children.push((child_abs, child_rel));
            }
            // Deterministic order: alphabetical by relative path. (The
            // receiver pre-creates every directory entry up front in
            // `TransferReceiver::start`, so mixing files and
            // directories in a single sort order doesn't affect
            // correctness — the receiver's parent dir always exists.)
            children.sort_by(|a, b| a.1.cmp(&b.1));
            for child in children.into_iter().rev() {
                stack.push(child);
            }
        } else if meta.is_file() {
            entries.push(FileManifestEntry {
                rel_path: rel.clone(),
                size: meta.len(),
                is_dir: false,
            });
            paths.push(abs.clone());
        }
        // Symlinks and other types are skipped for M9; see spec.
    }

    let total_bytes = entries.iter().map(|e| e.size).sum();
    Ok((
        FileManifest {
            entries,
            total_bytes,
        },
        paths,
    ))
}
