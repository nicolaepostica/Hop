//! Wire-level message types and their payloads.

use bytes::Bytes;
use input_leap_common::{
    ButtonId, ClipboardFormat, ClipboardId, FileManifest, KeyId, ModifierMask,
    TransferCancelReason, TransferId,
};
use serde::{Deserialize, Serialize};

/// Top-level wire message.
///
/// Uses adjacently-tagged CBOR — every variant is serialized as a map
/// with a `"type"` key naming the variant plus its fields inlined at
/// the same level. See `tests/snapshots.rs` for golden byte layouts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    /// Initial greeting exchanged once in each direction right after
    /// the TLS handshake.
    Hello(HelloPayload),

    /// Requests the peer's [`DeviceInfo`](Self::DeviceInfo).
    DeviceInfoRequest,

    /// Describes the sender's screen geometry and cursor state.
    DeviceInfo(DeviceInfoPayload),

    /// A key was pressed down.
    KeyDown {
        /// Key identifier.
        key: KeyId,
        /// Modifier keys active at the time of the press.
        mods: ModifierMask,
    },

    /// A key was released.
    KeyUp {
        /// Key identifier.
        key: KeyId,
        /// Modifier keys active at the time of the release.
        mods: ModifierMask,
    },

    /// A held key auto-repeated.
    KeyRepeat {
        /// Key identifier.
        key: KeyId,
        /// Modifier keys active at the time of the repeat burst.
        mods: ModifierMask,
        /// Number of auto-repeat events collapsed into this message.
        count: u16,
    },

    /// Absolute cursor position on the current screen.
    MouseMove {
        /// X coordinate in pixels.
        x: i32,
        /// Y coordinate in pixels.
        y: i32,
    },

    /// Relative cursor motion (for raw-input scenarios).
    MouseRelMove {
        /// X delta in pixels.
        dx: i32,
        /// Y delta in pixels.
        dy: i32,
    },

    /// A mouse button changed state.
    MouseButton {
        /// Which button.
        button: ButtonId,
        /// `true` on press, `false` on release.
        down: bool,
    },

    /// Mouse wheel scrolled.
    MouseWheel {
        /// Horizontal scroll in protocol units (120 = one logical tick).
        dx: i32,
        /// Vertical scroll in protocol units.
        dy: i32,
    },

    /// The server is transferring control to this client.
    ScreenEnter {
        /// Entry cursor X in client-local coordinates.
        x: i32,
        /// Entry cursor Y in client-local coordinates.
        y: i32,
        /// Monotonic sequence number used to correlate subsequent
        /// clipboard grabs with this screen entry.
        seq: u32,
        /// Modifier keys held at the moment of entry.
        mask: ModifierMask,
    },

    /// The server has taken control back from this client.
    ScreenLeave,

    /// Announces the sender now owns the given clipboard.
    ClipboardGrab {
        /// Which clipboard was grabbed.
        id: ClipboardId,
        /// Sequence number matching the most recent [`ScreenEnter`](Self::ScreenEnter).
        seq: u32,
    },

    /// Clipboard contents requested by the peer.
    ClipboardRequest {
        /// Which clipboard to fetch.
        id: ClipboardId,
        /// Sequence number to correlate with the originating grab.
        seq: u32,
    },

    /// Clipboard payload in a specific format.
    ClipboardData {
        /// Which clipboard this data belongs to.
        id: ClipboardId,
        /// Concrete format of the payload.
        format: ClipboardFormat,
        /// Raw bytes.
        data: Bytes,
    },

    /// Announces a new file-clipboard transfer; the manifest arrives
    /// before any chunk so the receiver can pre-validate paths and
    /// allocate staging space. See `specs/file-clipboard.md`.
    FileTransferStart {
        /// Unique identifier for this transfer.
        transfer_id: TransferId,
        /// Clipboard-grab sequence number the transfer is associated with.
        clipboard_seq: u32,
        /// Manifest of files and directories.
        manifest: FileManifest,
    },

    /// One chunk of one file within an active transfer.
    ///
    /// `offset` is the byte position within `entries[entry_index]`
    /// where `data` starts. The receiver rejects any chunk whose
    /// `offset` does not equal the bytes it has already written for
    /// that entry — this catches duplicate deliveries, out-of-order
    /// streams, and leaves room to resume after a reconnect by letting
    /// the sender jump directly to the first unreceived byte.
    FileChunk {
        /// Transfer this chunk belongs to.
        transfer_id: TransferId,
        /// Index into the manifest's `entries` that this chunk targets.
        entry_index: u32,
        /// Byte offset within the target entry where `data` begins.
        offset: u64,
        /// Raw bytes to write at `offset` in `entries[entry_index]`.
        data: Bytes,
    },

    /// All chunks for a transfer have been sent; receiver may finalise.
    FileTransferEnd {
        /// Transfer that just completed.
        transfer_id: TransferId,
    },

    /// Abort a transfer; receiver cleans up staging.
    FileTransferCancel {
        /// Transfer being cancelled.
        transfer_id: TransferId,
        /// Why.
        reason: TransferCancelReason,
    },

    /// Periodic liveness probe; peers exchange these every few seconds.
    KeepAlive,

    /// Orderly connection shutdown.
    Disconnect {
        /// Machine-readable reason code.
        reason: DisconnectReason,
    },
}

/// Payload of a [`Message::Hello`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloPayload {
    /// Sender's protocol version.
    pub protocol_version: u16,
    /// Human-readable screen name; typically the hostname.
    pub display_name: String,
    /// Optional protocol features the sender understands.
    pub capabilities: Vec<Capability>,
}

/// Payload of a [`Message::DeviceInfo`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DeviceInfoPayload {
    /// Screen width in pixels.
    pub width: u32,
    /// Screen height in pixels.
    pub height: u32,
    /// Current cursor X coordinate.
    pub cursor_x: i32,
    /// Current cursor Y coordinate.
    pub cursor_y: i32,
    /// Integer DPI scale factor times 100 (100 means 1.0x, 150 means 1.5x).
    pub scale_factor_pct: u16,
}

/// Optional features a peer may support.
///
/// The `#[serde(other)]` `Unknown` variant ensures an older peer that
/// does not understand a capability name sent by a newer peer still
/// parses the surrounding [`HelloPayload`] successfully and simply
/// ignores the unrecognized entry.
///
/// # Equality hazard
///
/// `Unknown` is a unit variant (serde's `#[serde(other)]` only accepts
/// unit variants), so two distinct unknown capability names from the
/// wire both round-trip to `Unknown` and compare equal under
/// [`PartialEq`]. This is fine for the "is this capability supported"
/// check callers do — `vec.contains(&Capability::UnicodeClipboard)` is
/// never `true` just because `Unknown` is in the list — but callers
/// must not use `Unknown` as a key or rely on equality to tell unknown
/// capabilities apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Supports file transfers via the clipboard (see M9).
    FileClipboard,
    /// Supports full Unicode (non-BMP) clipboard payloads.
    UnicodeClipboard,
    /// Supports HTML clipboard payloads.
    ClipboardHtml,
    /// Catch-all for unrecognized capability strings.
    #[serde(other)]
    Unknown,
}

/// Machine-readable reason for a [`Message::Disconnect`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisconnectReason {
    /// Peer announced a protocol version we cannot speak.
    ProtocolVersionMismatch,
    /// Peer stopped sending keep-alives.
    KeepAliveTimeout,
    /// Peer's TLS fingerprint is not in our trust store.
    UnknownPeer,
    /// Peer sent a frame that failed to decode.
    MalformedMessage,
    /// Peer sent a frame larger than our configured limit.
    FrameTooLarge,
    /// Local operator requested shutdown (e.g. Ctrl+C, GUI action).
    UserInitiated,
    /// Unrecoverable error on the sending side.
    InternalError,
    /// Catch-all for unrecognized disconnect reasons.
    #[serde(other)]
    Unknown,
}
