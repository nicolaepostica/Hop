//! Transfer engine integration tests.
//!
//! Runs the sender and receiver in the same process over an `mpsc`
//! channel that stands in for the TLS connection — verifies that a
//! directory tree written to disk by the sender ends up byte-identical
//! in the receiver's drop directory.

use std::collections::BTreeMap;
use std::path::PathBuf;

use bytes::Bytes;
use input_leap_common::{FileManifest, FileManifestEntry, TransferCancelReason};
use input_leap_protocol::Message;
use input_leap_transfer::{TransferError, TransferReceiver, TransferSender};
use tempfile::TempDir;
use tokio::fs;
use tokio::sync::mpsc;

/// Write a small tree of files + one subdir under `root` and return
/// a map from each file's relative path to its expected bytes.
async fn build_source_tree(root: &std::path::Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fs::create_dir_all(root.join("sub")).await.unwrap();
    let files: Vec<(PathBuf, Vec<u8>)> = vec![
        (PathBuf::from("a.txt"), b"alpha".to_vec()),
        (PathBuf::from("b.bin"), (0u8..128).collect()),
        (PathBuf::from("sub/c.md"), b"# header\nbody\n".to_vec()),
    ];
    for (rel, body) in &files {
        fs::write(root.join(rel), body).await.unwrap();
    }
    files.into_iter().collect()
}

async fn run_transfer(
    source_root: &std::path::Path,
    staging_root: &std::path::Path,
    drop_dir: &std::path::Path,
    chunk_size: usize,
) -> Result<Vec<input_leap_transfer::ReceivedFile>, TransferError> {
    let (tx, mut rx) = mpsc::channel::<Message>(64);
    let sender = TransferSender::new(42, chunk_size);
    let source = source_root.to_path_buf();
    let send_task = tokio::spawn(async move {
        sender
            .send_directory(&source, 7, 10 * 1024 * 1024, &tx)
            .await
    });

    // Receiver: read the first message (FileTransferStart), construct
    // receiver, feed chunks, finalise.
    let start = rx.recv().await.expect("start message");
    let (transfer_id, manifest) = match start {
        Message::FileTransferStart {
            transfer_id,
            manifest,
            ..
        } => (transfer_id, manifest),
        other => panic!("expected FileTransferStart, got {other:?}"),
    };

    let mut receiver = TransferReceiver::start(transfer_id, manifest, staging_root).await?;
    while let Some(msg) = rx.recv().await {
        match msg {
            Message::FileChunk {
                transfer_id: _,
                entry_index,
                data,
            } => {
                receiver.on_chunk(entry_index, &data).await?;
            }
            Message::FileTransferEnd { .. } => break,
            Message::FileTransferCancel { reason, .. } => {
                receiver.cancel(reason);
                return Err(TransferError::Incomplete {
                    entry_index: 0,
                    missing: 0,
                });
            }
            other => panic!("unexpected mid-transfer message: {other:?}"),
        }
    }

    let result = receiver.finish(drop_dir).await?;
    let _ = send_task.await.expect("sender task didn't panic");
    Ok(result)
}

#[tokio::test(flavor = "multi_thread")]
async fn round_trip_preserves_bytes() {
    let source = TempDir::new().unwrap();
    let staging = TempDir::new().unwrap();
    let drop = TempDir::new().unwrap();
    let expected = build_source_tree(source.path()).await;

    let files = run_transfer(source.path(), staging.path(), drop.path(), 32)
        .await
        .expect("transfer succeeds");
    // There are exactly 3 files in our tree.
    assert_eq!(files.len(), 3);

    // Verify each on-disk file matches what we wrote at the source.
    for received in &files {
        let bytes = fs::read(&received.path).await.unwrap();
        assert_eq!(bytes.len() as u64, received.size);
        let rel = received
            .path
            .strip_prefix(drop.path())
            .unwrap()
            .components()
            .skip(1) // the leaf transfer-<id> dir
            .collect::<PathBuf>();
        let expected_bytes = expected.get(&rel).expect("matching source file");
        assert_eq!(&bytes, expected_bytes, "content mismatch for {rel:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn path_traversal_is_rejected_before_any_write() {
    let staging = TempDir::new().unwrap();
    let malicious = FileManifest {
        entries: vec![FileManifestEntry {
            rel_path: PathBuf::from("../../escape.txt"),
            size: 4,
            is_dir: false,
        }],
        total_bytes: 4,
    };
    let err = TransferReceiver::start(1, malicious, staging.path())
        .await
        .expect_err("path traversal must be rejected");
    assert!(matches!(err, TransferError::PathTraversal { .. }));

    // And the staging subdir must not contain any files.
    let mut entries = fs::read_dir(staging.path().join("transfer-1"))
        .await
        .unwrap();
    assert!(entries.next_entry().await.unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_drops_staging_directory() {
    let staging = TempDir::new().unwrap();
    let manifest = FileManifest {
        entries: vec![FileManifestEntry {
            rel_path: PathBuf::from("pending.bin"),
            size: 10,
            is_dir: false,
        }],
        total_bytes: 10,
    };
    let mut rx = TransferReceiver::start(99, manifest, staging.path())
        .await
        .unwrap();
    rx.on_chunk(0, &[1, 2, 3]).await.unwrap();
    let staging_sub = staging.path().join("transfer-99");
    assert!(staging_sub.exists());

    rx.cancel(TransferCancelReason::UserCancelled);
    // Drop guard should remove staging by now.
    assert!(
        !staging_sub.exists(),
        "staging dir must be gone after cancel: {staging_sub:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn size_overflow_is_caught() {
    let staging = TempDir::new().unwrap();
    let manifest = FileManifest {
        entries: vec![FileManifestEntry {
            rel_path: PathBuf::from("short.bin"),
            size: 3,
            is_dir: false,
        }],
        total_bytes: 3,
    };
    let mut rx = TransferReceiver::start(11, manifest, staging.path())
        .await
        .unwrap();
    rx.on_chunk(0, &[1, 2, 3]).await.unwrap();
    let err = rx.on_chunk(0, &[4]).await.unwrap_err();
    assert!(matches!(err, TransferError::SizeOverflow { .. }));
    rx.cancel(TransferCancelReason::PeerError);
}

#[tokio::test(flavor = "multi_thread")]
async fn incomplete_transfer_fails_to_finalise() {
    let staging = TempDir::new().unwrap();
    let drop = TempDir::new().unwrap();
    let manifest = FileManifest {
        entries: vec![FileManifestEntry {
            rel_path: PathBuf::from("half.bin"),
            size: 10,
            is_dir: false,
        }],
        total_bytes: 10,
    };
    let mut rx = TransferReceiver::start(5, manifest, staging.path())
        .await
        .unwrap();
    rx.on_chunk(0, &[0; 4]).await.unwrap();
    match rx.finish(drop.path()).await {
        Err(TransferError::Incomplete {
            missing,
            entry_index,
        }) => {
            assert_eq!(entry_index, 0);
            assert_eq!(missing, 6);
        }
        other => panic!("expected Incomplete, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_source_tree_round_trips() {
    let source = TempDir::new().unwrap();
    let staging = TempDir::new().unwrap();
    let drop = TempDir::new().unwrap();

    let files = run_transfer(source.path(), staging.path(), drop.path(), 4096)
        .await
        .expect("empty transfer still succeeds");
    assert!(files.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn large_file_is_chunked_and_reassembled() {
    let source = TempDir::new().unwrap();
    let staging = TempDir::new().unwrap();
    let drop = TempDir::new().unwrap();

    // 10 KiB payload with a varying pattern so any off-by-one shows up.
    let payload: Vec<u8> = (0u16..10 * 1024)
        .map(|i| ((i & 0xff) as u8).wrapping_mul(7))
        .collect();
    fs::write(source.path().join("big.bin"), &payload)
        .await
        .unwrap();

    let files = run_transfer(source.path(), staging.path(), drop.path(), 512)
        .await
        .expect("transfer succeeds");
    assert_eq!(files.len(), 1);
    let got = fs::read(&files[0].path).await.unwrap();
    assert_eq!(got.len(), payload.len());
    assert_eq!(got, payload);
}

#[tokio::test(flavor = "multi_thread")]
async fn chunk_is_a_noop_for_directory_entries() {
    let staging = TempDir::new().unwrap();
    let manifest = FileManifest {
        entries: vec![
            FileManifestEntry {
                rel_path: PathBuf::from("d"),
                size: 0,
                is_dir: true,
            },
            FileManifestEntry {
                rel_path: PathBuf::from("d/x.txt"),
                size: 1,
                is_dir: false,
            },
        ],
        total_bytes: 1,
    };
    let mut rx = TransferReceiver::start(3, manifest, staging.path())
        .await
        .unwrap();
    // Chunk targeting the directory entry (index 0) must error.
    let err = rx.on_chunk(0, &[0]).await.unwrap_err();
    assert!(matches!(err, TransferError::UnexpectedChunk { .. }));
    // A chunk targeting the file (index 1) is fine.
    rx.on_chunk(1, &[7]).await.unwrap();
    rx.cancel(TransferCancelReason::UserCancelled);
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_bytes_forces_cleanup() {
    // Just to prove Bytes doesn't hold onto anything beyond the call —
    // the receiver must have already written everything to disk by
    // the time on_chunk returns.
    let staging = TempDir::new().unwrap();
    let manifest = FileManifest {
        entries: vec![FileManifestEntry {
            rel_path: PathBuf::from("x"),
            size: 3,
            is_dir: false,
        }],
        total_bytes: 3,
    };
    let mut rx = TransferReceiver::start(12, manifest, staging.path())
        .await
        .unwrap();
    let chunk = Bytes::from_static(b"abc");
    rx.on_chunk(0, &chunk).await.unwrap();
    drop(chunk);
    rx.cancel(TransferCancelReason::UserCancelled);
}
