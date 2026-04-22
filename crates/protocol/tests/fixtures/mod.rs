//! Shared fixtures for protocol tests.
//!
//! Holds proptest strategies for random generation and a set of
//! canonical [`Message`] instances used by snapshot tests.

#![allow(dead_code, reason = "different test binaries use different subsets")]

use std::path::PathBuf;

use bytes::Bytes;
use input_leap_common::{
    ButtonId, ClipboardFormat, ClipboardId, FileManifest, FileManifestEntry, KeyId, ModifierMask,
    TransferCancelReason,
};
use input_leap_protocol::{
    Capability, DeviceInfoPayload, DisconnectReason, HelloPayload, Message, PROTOCOL_VERSION,
};
use proptest::prelude::*;
use proptest::strategy::Union;

// ----- atom strategies ---------------------------------------------------

pub fn arb_key_id() -> impl Strategy<Value = KeyId> {
    any::<u32>().prop_map(KeyId::new)
}

pub fn arb_button_id() -> impl Strategy<Value = ButtonId> {
    any::<u8>().prop_map(ButtonId::new)
}

pub fn arb_modifier_mask() -> impl Strategy<Value = ModifierMask> {
    // `from_bits_retain` keeps unknown bits so the round-trip property
    // covers forward-compatible flags the peer might introduce later.
    any::<u32>().prop_map(ModifierMask::from_bits_retain)
}

pub fn arb_clipboard_id() -> impl Strategy<Value = ClipboardId> {
    prop_oneof![Just(ClipboardId::Clipboard), Just(ClipboardId::Primary)]
}

/// `ClipboardFormat::Unknown` is intentionally omitted: the wire format
/// serializes `Unknown` to the literal string `"unknown"`, so a
/// round-trip would turn any originally-"unknown" string into just
/// `Unknown` — which is the forward-compat behavior we want in prod,
/// but it violates the round-trip invariant we test here.
pub fn arb_clipboard_format() -> impl Strategy<Value = ClipboardFormat> {
    prop_oneof![
        Just(ClipboardFormat::Text),
        Just(ClipboardFormat::Html),
        Just(ClipboardFormat::Bitmap),
        Just(ClipboardFormat::Files),
    ]
}

/// See comment on [`arb_clipboard_format`] — same reasoning applies.
pub fn arb_capability() -> impl Strategy<Value = Capability> {
    prop_oneof![
        Just(Capability::FileClipboard),
        Just(Capability::UnicodeClipboard),
        Just(Capability::ClipboardHtml),
    ]
}

pub fn arb_disconnect_reason() -> impl Strategy<Value = DisconnectReason> {
    prop_oneof![
        Just(DisconnectReason::ProtocolVersionMismatch),
        Just(DisconnectReason::KeepAliveTimeout),
        Just(DisconnectReason::UnknownPeer),
        Just(DisconnectReason::MalformedMessage),
        Just(DisconnectReason::FrameTooLarge),
        Just(DisconnectReason::UserInitiated),
        Just(DisconnectReason::InternalError),
    ]
}

/// Transfer cancel reasons, skipping the `Unknown` catch-all for the
/// same round-trip reason as `Capability::Unknown`.
pub fn arb_cancel_reason() -> impl Strategy<Value = TransferCancelReason> {
    prop_oneof![
        Just(TransferCancelReason::UserCancelled),
        Just(TransferCancelReason::DiskFull),
        Just(TransferCancelReason::SizeMismatch),
        Just(TransferCancelReason::PeerError),
        Just(TransferCancelReason::PathTraversal),
        Just(TransferCancelReason::TooLarge),
    ]
}

pub fn arb_manifest_entry() -> impl Strategy<Value = FileManifestEntry> {
    // Keep sizes modest so a small Vec of entries never overflows the
    // manifest's `total_bytes` (u64) when we sum them below.
    ("[a-z]{1,8}/?[a-z]{1,8}", 0u64..1_000_000, any::<bool>()).prop_map(|(rel, size, is_dir)| {
        FileManifestEntry {
            rel_path: PathBuf::from(rel),
            size: if is_dir { 0 } else { size },
            is_dir,
        }
    })
}

pub fn arb_manifest() -> impl Strategy<Value = FileManifest> {
    prop::collection::vec(arb_manifest_entry(), 0..4).prop_map(|entries| {
        let total_bytes = entries.iter().map(|e| e.size).sum();
        FileManifest {
            entries,
            total_bytes,
        }
    })
}

pub fn arb_hello() -> impl Strategy<Value = HelloPayload> {
    (
        any::<u16>(),
        "[a-zA-Z0-9._ -]{0,32}",
        prop::collection::vec(arb_capability(), 0..4),
    )
        .prop_map(
            |(protocol_version, display_name, capabilities)| HelloPayload {
                protocol_version,
                display_name,
                capabilities,
            },
        )
}

pub fn arb_device_info() -> impl Strategy<Value = DeviceInfoPayload> {
    (
        any::<u32>(),
        any::<u32>(),
        any::<i32>(),
        any::<i32>(),
        any::<u16>(),
    )
        .prop_map(
            |(width, height, cursor_x, cursor_y, scale_factor_pct)| DeviceInfoPayload {
                width,
                height,
                cursor_x,
                cursor_y,
                scale_factor_pct,
            },
        )
}

pub fn arb_bytes() -> impl Strategy<Value = Bytes> {
    prop::collection::vec(any::<u8>(), 0..1024).prop_map(Bytes::from)
}

// ----- top-level strategy -----------------------------------------------

pub fn arb_message() -> BoxedStrategy<Message> {
    Union::new(vec![
        arb_hello().prop_map(Message::Hello).boxed(),
        Just(Message::DeviceInfoRequest).boxed(),
        arb_device_info().prop_map(Message::DeviceInfo).boxed(),
        (arb_key_id(), arb_modifier_mask())
            .prop_map(|(key, mods)| Message::KeyDown { key, mods })
            .boxed(),
        (arb_key_id(), arb_modifier_mask())
            .prop_map(|(key, mods)| Message::KeyUp { key, mods })
            .boxed(),
        (arb_key_id(), arb_modifier_mask(), any::<u16>())
            .prop_map(|(key, mods, count)| Message::KeyRepeat { key, mods, count })
            .boxed(),
        (any::<i32>(), any::<i32>())
            .prop_map(|(x, y)| Message::MouseMove { x, y })
            .boxed(),
        (any::<i32>(), any::<i32>())
            .prop_map(|(dx, dy)| Message::MouseRelMove { dx, dy })
            .boxed(),
        (arb_button_id(), any::<bool>())
            .prop_map(|(button, down)| Message::MouseButton { button, down })
            .boxed(),
        (any::<i32>(), any::<i32>())
            .prop_map(|(dx, dy)| Message::MouseWheel { dx, dy })
            .boxed(),
        (
            any::<i32>(),
            any::<i32>(),
            any::<u32>(),
            arb_modifier_mask(),
        )
            .prop_map(|(x, y, seq, mask)| Message::ScreenEnter { x, y, seq, mask })
            .boxed(),
        Just(Message::ScreenLeave).boxed(),
        (arb_clipboard_id(), any::<u32>())
            .prop_map(|(id, seq)| Message::ClipboardGrab { id, seq })
            .boxed(),
        (arb_clipboard_id(), any::<u32>())
            .prop_map(|(id, seq)| Message::ClipboardRequest { id, seq })
            .boxed(),
        (arb_clipboard_id(), arb_clipboard_format(), arb_bytes())
            .prop_map(|(id, format, data)| Message::ClipboardData { id, format, data })
            .boxed(),
        Just(Message::KeepAlive).boxed(),
        arb_disconnect_reason()
            .prop_map(|reason| Message::Disconnect { reason })
            .boxed(),
        (any::<u64>(), any::<u32>(), arb_manifest())
            .prop_map(
                |(transfer_id, clipboard_seq, manifest)| Message::FileTransferStart {
                    transfer_id,
                    clipboard_seq,
                    manifest,
                },
            )
            .boxed(),
        (any::<u64>(), any::<u32>(), arb_bytes())
            .prop_map(|(transfer_id, entry_index, data)| Message::FileChunk {
                transfer_id,
                entry_index,
                data,
            })
            .boxed(),
        any::<u64>()
            .prop_map(|transfer_id| Message::FileTransferEnd { transfer_id })
            .boxed(),
        (any::<u64>(), arb_cancel_reason())
            .prop_map(|(transfer_id, reason)| Message::FileTransferCancel {
                transfer_id,
                reason,
            })
            .boxed(),
    ])
    .boxed()
}

// ----- canonical instances for snapshot tests ---------------------------

/// One representative instance of each [`Message`] variant. Ordered so a
/// snapshot file read top-to-bottom walks through the whole wire vocabulary.
#[allow(clippy::too_many_lines, reason = "intentional: one entry per variant")]
pub fn canonical_messages() -> Vec<(&'static str, Message)> {
    vec![
        (
            "hello",
            Message::Hello(HelloPayload {
                protocol_version: PROTOCOL_VERSION,
                display_name: "laptop".into(),
                capabilities: vec![Capability::UnicodeClipboard, Capability::ClipboardHtml],
            }),
        ),
        ("device_info_request", Message::DeviceInfoRequest),
        (
            "device_info",
            Message::DeviceInfo(DeviceInfoPayload {
                width: 2560,
                height: 1440,
                cursor_x: 100,
                cursor_y: 200,
                scale_factor_pct: 100,
            }),
        ),
        (
            "key_down",
            Message::KeyDown {
                key: KeyId::new(0x61), // 'a'
                mods: ModifierMask::SHIFT,
            },
        ),
        (
            "key_up",
            Message::KeyUp {
                key: KeyId::new(0x61),
                mods: ModifierMask::empty(),
            },
        ),
        (
            "key_repeat",
            Message::KeyRepeat {
                key: KeyId::new(0xff08), // backspace
                mods: ModifierMask::empty(),
                count: 3,
            },
        ),
        ("mouse_move", Message::MouseMove { x: 1024, y: 768 }),
        ("mouse_rel_move", Message::MouseRelMove { dx: -5, dy: 12 }),
        (
            "mouse_button",
            Message::MouseButton {
                button: ButtonId::LEFT,
                down: true,
            },
        ),
        ("mouse_wheel", Message::MouseWheel { dx: 0, dy: 120 }),
        (
            "screen_enter",
            Message::ScreenEnter {
                x: 0,
                y: 0,
                seq: 42,
                mask: ModifierMask::CTRL | ModifierMask::ALT,
            },
        ),
        ("screen_leave", Message::ScreenLeave),
        (
            "clipboard_grab",
            Message::ClipboardGrab {
                id: ClipboardId::Clipboard,
                seq: 42,
            },
        ),
        (
            "clipboard_request",
            Message::ClipboardRequest {
                id: ClipboardId::Clipboard,
                seq: 42,
            },
        ),
        (
            "clipboard_data",
            Message::ClipboardData {
                id: ClipboardId::Clipboard,
                format: ClipboardFormat::Text,
                data: Bytes::from_static(b"hello"),
            },
        ),
        ("keep_alive", Message::KeepAlive),
        (
            "disconnect",
            Message::Disconnect {
                reason: DisconnectReason::UserInitiated,
            },
        ),
        (
            "file_transfer_start",
            Message::FileTransferStart {
                transfer_id: 1,
                clipboard_seq: 42,
                manifest: FileManifest {
                    entries: vec![FileManifestEntry {
                        rel_path: PathBuf::from("notes.txt"),
                        size: 12,
                        is_dir: false,
                    }],
                    total_bytes: 12,
                },
            },
        ),
        (
            "file_chunk",
            Message::FileChunk {
                transfer_id: 1,
                entry_index: 0,
                data: Bytes::from_static(b"hello world!"),
            },
        ),
        (
            "file_transfer_end",
            Message::FileTransferEnd { transfer_id: 1 },
        ),
        (
            "file_transfer_cancel",
            Message::FileTransferCancel {
                transfer_id: 1,
                reason: TransferCancelReason::UserCancelled,
            },
        ),
    ]
}
