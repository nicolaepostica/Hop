//! Clipboard identifiers and payload formats.

use serde::{Deserialize, Serialize};

/// Identifies which logical clipboard a message refers to.
///
/// Hop exposes two clipboards to match X11's selection model:
/// the primary selection (mouse-driven) and the regular clipboard
/// (Ctrl+C / Ctrl+V). Platforms without this distinction map both to
/// the OS clipboard.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardId {
    /// The system clipboard (Ctrl+C / Ctrl+V).
    #[default]
    Clipboard,
    /// The X11-style primary selection (mouse highlight).
    Primary,
}

/// Format descriptor for clipboard payloads.
///
/// Each [`ClipboardId`] slot may contain values in multiple formats at
/// once; peers negotiate which ones they actually transfer based on
/// capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardFormat {
    /// UTF-8 plain text.
    Text,
    /// HTML fragment (UTF-8).
    Html,
    /// Raw RGBA bitmap (implementation-defined header).
    Bitmap,
    /// File list (see `specs/architecture.md`, M9).
    Files,
    /// An unrecognized format; present so old peers can discard unknown
    /// payloads sent by newer peers instead of dropping the connection.
    #[serde(other)]
    Unknown,
}
