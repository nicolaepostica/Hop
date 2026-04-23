//! Shared types for Hop.
//!
//! This crate holds primitive ID and enum types referenced by the protocol,
//! platform, and application layers. It has no async runtime dependencies
//! and stays small and synchronous on purpose.

pub mod clipboard;
pub mod ids;
pub mod modifier;
pub mod transfer;

pub use self::clipboard::{ClipboardFormat, ClipboardId};
pub use self::ids::{ButtonId, KeyId};
pub use self::modifier::ModifierMask;
pub use self::transfer::{FileManifest, FileManifestEntry, TransferCancelReason, TransferId};
