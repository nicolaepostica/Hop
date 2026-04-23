//! The [`Coordinator`] actor — pure state machine.
//!
//! Consumes [`CoordinatorEvent`]s (local platform input, client
//! lifecycle, inbound peer messages) and produces a sequence of
//! [`CoordinatorOutput`]s the driver turns into I/O. No tokio, no
//! channels, no sockets inside — that keeps the test matrix large and
//! cheap.
//!
//! See `specs/milestones/M11-coordinator.md` for the full design.

use std::collections::HashMap;

use hop_common::{ClipboardFormat, ClipboardId};
use hop_platform::InputEvent;
use hop_protocol::{Capability, Message};

use crate::coordinator::clipboard::ClipboardGrabState;
use crate::coordinator::held::HeldState;
use crate::coordinator::layout::{ScreenLayout, ScreenName, SharedLayout};

/// Metadata about a connected client. The actual outbound
/// `mpsc::Sender<Message>` is owned by the task driver — the
/// Coordinator only needs to know a client exists.
#[derive(Debug, Clone)]
pub struct ClientInfo {
    /// Capabilities the client advertised in its `Hello`.
    pub capabilities: Vec<Capability>,
}

/// Input to the coordinator.
#[derive(Debug, Clone)]
pub enum CoordinatorEvent {
    /// An event observed on the local (primary) platform screen.
    LocalInput(InputEvent),
    /// A peer finished handshake; its display name and capabilities.
    ClientConnected {
        /// Peer's display name (from `Hello`).
        name: ScreenName,
        /// Advertised capabilities.
        capabilities: Vec<Capability>,
    },
    /// A peer went away. Called once per client when the connection
    /// drops for any reason (clean close, timeout, error, forced drop
    /// due to backpressure).
    ClientDisconnected {
        /// Display name of the gone peer.
        name: ScreenName,
    },
    /// A peer sent us a protocol message. Only non-handshake,
    /// non-keep-alive, non-disconnect messages reach the coordinator.
    PeerMessage {
        /// Sender's display name.
        from: ScreenName,
        /// The decoded message.
        msg: Message,
    },
    /// Layout swap happened via `LayoutStore::reload()` — coordinator
    /// should re-read the snapshot on next event (no-op today; the
    /// `.load()` on every hot-path call already picks up changes).
    LayoutReloaded,
}

/// Effect the coordinator wants the driver to perform.
#[derive(Debug, Clone, PartialEq)]
pub enum CoordinatorOutput {
    /// Forward `msg` to a specific peer.
    Send {
        /// Target peer (looked up in the driver's sender table).
        to: ScreenName,
        /// Message to deliver.
        msg: Message,
    },
    /// Hand `msg` to the local platform dispatcher (clipboard writes,
    /// etc.). Only meaningful when the local screen is active.
    InjectLocal(Message),
    /// Surface a human-readable warning via `tracing::warn!`.
    Warn(String),
}

/// Single-actor core: layout, cursor, held state, clipboard-grab
/// table, client registry. Entirely synchronous.
#[derive(Debug)]
pub struct Coordinator {
    layout: SharedLayout,
    local_name: ScreenName,
    /// Whose inputs are currently being routed. Starts as the local
    /// primary and returns there whenever the active peer goes away.
    active: ScreenName,
    /// Virtual-coordinate cursor position.
    cursor: (i32, i32),
    /// Last platform-local absolute cursor we observed, used to turn
    /// absolute `MouseMove` events into virtual deltas without
    /// requiring pointer-grab on the primary.
    last_platform_pos: Option<(i32, i32)>,
    held: HeldState,
    grabs: ClipboardGrabState,
    /// Connected clients whose names match an entry in the layout.
    clients: HashMap<ScreenName, ClientInfo>,
    /// Connected clients whose names do **not** match the layout —
    /// they get the `Hello` but won't receive routed input.
    orphans: HashMap<ScreenName, ClientInfo>,
    /// Monotonic seq bumped on every screen transition. Used for
    /// `ScreenEnter.seq` and clipboard-grab-correlation.
    seq: u32,
}

impl Coordinator {
    /// Build a fresh coordinator pointing at the primary.
    #[must_use]
    pub fn new(layout: SharedLayout, local_name: ScreenName) -> Self {
        // Park the virtual cursor at the primary's centre so the first
        // MouseMove has a defined delta to work against even if the
        // layout is swapped before anything else happens.
        let snap = layout.load_full();
        let cursor = snap.screen_by_name(&local_name).map_or((0, 0), |s| {
            (
                s.origin_x
                    .saturating_add(i32::try_from(s.width / 2).unwrap_or(0)),
                s.origin_y
                    .saturating_add(i32::try_from(s.height / 2).unwrap_or(0)),
            )
        });

        Self {
            layout,
            local_name: local_name.clone(),
            active: local_name,
            cursor,
            last_platform_pos: None,
            held: HeldState::default(),
            grabs: ClipboardGrabState::default(),
            clients: HashMap::new(),
            orphans: HashMap::new(),
            seq: 0,
        }
    }

    /// Current virtual-coordinate cursor — exposed for tests.
    #[must_use]
    pub fn cursor(&self) -> (i32, i32) {
        self.cursor
    }

    /// Name of the currently active screen — exposed for tests.
    #[must_use]
    pub fn active(&self) -> &str {
        &self.active
    }

    /// Global seq — exposed for tests.
    #[must_use]
    pub fn seq(&self) -> u32 {
        self.seq
    }

    /// Single entry point. Push outputs into `buf` (caller reuses it
    /// across calls to avoid per-event allocation).
    pub fn on_event(&mut self, event: CoordinatorEvent, buf: &mut Vec<CoordinatorOutput>) {
        match event {
            CoordinatorEvent::LocalInput(ev) => self.on_local_input(ev, buf),
            CoordinatorEvent::ClientConnected { name, capabilities } => {
                self.on_client_connected(name, capabilities, buf);
            }
            CoordinatorEvent::ClientDisconnected { name } => {
                self.on_client_disconnected(&name, buf);
            }
            CoordinatorEvent::PeerMessage { from, msg } => {
                self.on_peer_message(from, msg, buf);
            }
            CoordinatorEvent::LayoutReloaded => {
                // The layout is read through `ArcSwap::load_full()` on
                // every hot path so there is nothing to do here beyond
                // record that something changed for log correlation.
            }
        }
    }

    // ---- local input ----

    #[allow(clippy::needless_pass_by_value, reason = "consumed into forwarded Message")]
    fn on_local_input(&mut self, event: InputEvent, buf: &mut Vec<CoordinatorOutput>) {
        match event {
            InputEvent::MouseMove { x, y } => self.on_mouse_move(x, y, buf),
            InputEvent::MouseWheel { dx, dy } => {
                self.held.apply(&InputEvent::MouseWheel { dx, dy });
                self.forward_if_remote(Message::MouseWheel { dx, dy }, buf);
            }
            InputEvent::KeyDown { key, mods } => {
                self.held.apply(&InputEvent::KeyDown { key, mods });
                self.forward_if_remote(Message::KeyDown { key, mods }, buf);
            }
            InputEvent::KeyUp { key, mods } => {
                self.held.apply(&InputEvent::KeyUp { key, mods });
                self.forward_if_remote(Message::KeyUp { key, mods }, buf);
            }
            InputEvent::MouseButton { button, down } => {
                self.held.apply(&InputEvent::MouseButton { button, down });
                self.forward_if_remote(Message::MouseButton { button, down }, buf);
            }
        }
    }

    #[allow(clippy::needless_pass_by_value, reason = "consumed into Send variant")]
    fn forward_if_remote(&self, msg: Message, buf: &mut Vec<CoordinatorOutput>) {
        if self.active != self.local_name {
            buf.push(CoordinatorOutput::Send {
                to: self.active.clone(),
                msg,
            });
        }
    }

    fn on_mouse_move(&mut self, local_x: i32, local_y: i32, buf: &mut Vec<CoordinatorOutput>) {
        let layout = self.layout.load_full();

        // Translate platform-local absolute coords to a virtual-space
        // delta. We can't just use `primary.origin + (x, y)` because
        // then crossing into a remote screen would keep snapping the
        // virtual cursor back onto the primary rect.
        let delta = match self.last_platform_pos {
            Some((px, py)) => (local_x.saturating_sub(px), local_y.saturating_sub(py)),
            None => (0, 0), // first event bootstraps position
        };
        self.last_platform_pos = Some((local_x, local_y));

        let new_cursor = (
            self.cursor.0.saturating_add(delta.0),
            self.cursor.1.saturating_add(delta.1),
        );

        // Drag mid-press: refuse to cross; clamp inside current active.
        if self.held.any_button_held() {
            self.clamp_and_forward(new_cursor, &layout, buf);
            return;
        }

        // Which screen does the new position belong to?
        let Some(target) = layout.screen_at(new_cursor.0, new_cursor.1) else {
            // Gap in layout — clamp to current active.
            self.clamp_and_forward(new_cursor, &layout, buf);
            return;
        };
        let target_name = target.name.clone();

        if target_name == self.active {
            // Staying put.
            self.cursor = new_cursor;
            if self.active != self.local_name {
                if let Some(screen) = layout.screen_by_name(&self.active) {
                    let (lx, ly) = to_local(new_cursor, screen.origin_x, screen.origin_y);
                    buf.push(CoordinatorOutput::Send {
                        to: self.active.clone(),
                        msg: Message::MouseMove { x: lx, y: ly },
                    });
                }
            }
            return;
        }

        // Crossing! Either target or current-active may be a client
        // we've never heard of (orphan / missing); `cross_to` warns
        // and routes as best it can.
        self.cross_to(&target_name, new_cursor, &layout, buf);
    }

    fn clamp_and_forward(
        &mut self,
        attempted: (i32, i32),
        layout: &ScreenLayout,
        buf: &mut Vec<CoordinatorOutput>,
    ) {
        let Some(current) = layout.screen_by_name(&self.active) else {
            self.cursor = attempted;
            return;
        };
        let clamped = current.clamp(attempted.0, attempted.1);
        self.cursor = clamped;
        if self.active != self.local_name {
            let (lx, ly) = to_local(clamped, current.origin_x, current.origin_y);
            buf.push(CoordinatorOutput::Send {
                to: self.active.clone(),
                msg: Message::MouseMove { x: lx, y: ly },
            });
        }
    }

    fn cross_to(
        &mut self,
        new_active: &ScreenName,
        new_cursor: (i32, i32),
        layout: &ScreenLayout,
        buf: &mut Vec<CoordinatorOutput>,
    ) {
        // 1. Release everything on the old active side.
        let leave_msgs = self.held.leave_messages();
        if self.active != self.local_name {
            for msg in leave_msgs {
                buf.push(CoordinatorOutput::Send {
                    to: self.active.clone(),
                    msg,
                });
            }
            buf.push(CoordinatorOutput::Send {
                to: self.active.clone(),
                msg: Message::ScreenLeave,
            });
        }

        // 2. Switch.
        let old_active = std::mem::replace(&mut self.active, new_active.clone());
        self.cursor = new_cursor;
        self.seq = self.seq.wrapping_add(1);

        // 3. Enter the new side.
        if new_active != &self.local_name {
            let Some(screen) = layout.screen_by_name(new_active) else {
                buf.push(CoordinatorOutput::Warn(format!(
                    "crossed into screen '{new_active}' which is not in the layout; \
                     falling back to '{}'",
                    self.local_name
                )));
                self.active = self.local_name.clone();
                return;
            };
            if !self.clients.contains_key(new_active) {
                buf.push(CoordinatorOutput::Warn(format!(
                    "crossed into '{new_active}' but no client is connected under that name; \
                     returning to '{}'",
                    self.local_name
                )));
                self.active = self.local_name.clone();
                return;
            }
            let (lx, ly) = to_local(new_cursor, screen.origin_x, screen.origin_y);
            buf.push(CoordinatorOutput::Send {
                to: new_active.clone(),
                msg: Message::ScreenEnter {
                    x: lx,
                    y: ly,
                    seq: self.seq,
                    mask: self.held.mods(),
                },
            });
            for msg in self.held.enter_messages() {
                buf.push(CoordinatorOutput::Send {
                    to: new_active.clone(),
                    msg,
                });
            }
        }
        let _ = old_active; // kept for future debug tracing.
    }

    // ---- client lifecycle ----

    fn on_client_connected(
        &mut self,
        name: ScreenName,
        capabilities: Vec<Capability>,
        buf: &mut Vec<CoordinatorOutput>,
    ) {
        let info = ClientInfo { capabilities };
        let layout = self.layout.load_full();
        if layout.screen_by_name(&name).is_some() {
            self.clients.insert(name.clone(), info);
        } else {
            buf.push(CoordinatorOutput::Warn(format!(
                "client '{name}' connected but is not declared in the layout; \
                 inputs will not be routed to it"
            )));
            self.orphans.insert(name, info);
        }
    }

    fn on_client_disconnected(&mut self, name: &str, buf: &mut Vec<CoordinatorOutput>) {
        self.clients.remove(name);
        self.orphans.remove(name);
        self.grabs.drop_owner(name);
        if self.active == name {
            // Active peer vanished — snap back to local primary with
            // a fresh seq so any in-flight grab from the vanished
            // peer is guaranteed stale.
            self.active = self.local_name.clone();
            self.seq = self.seq.wrapping_add(1);
            buf.push(CoordinatorOutput::Warn(format!(
                "active client '{name}' disconnected; returning control to '{}'",
                self.local_name
            )));
        }
    }

    // ---- peer messages ----

    #[allow(clippy::needless_pass_by_value, reason = "consumed by match arms")]
    fn on_peer_message(
        &mut self,
        from: ScreenName,
        msg: Message,
        buf: &mut Vec<CoordinatorOutput>,
    ) {
        match msg {
            Message::ClipboardGrab { id, seq } => self.on_clipboard_grab(from, id, seq, buf),
            Message::ClipboardRequest { id, seq } => {
                self.on_clipboard_request(from, id, seq, buf);
            }
            Message::ClipboardData { id, format, data } => {
                self.on_clipboard_data(from, id, format, data, buf);
            }
            other => {
                // KeepAlive and Disconnect are filtered by ClientProxy
                // before they reach the coordinator. Anything else
                // from a peer (keyboard/mouse input from a client?) is
                // unexpected in the current server-pushes-inputs model.
                buf.push(CoordinatorOutput::Warn(format!(
                    "unexpected peer message from '{from}': {other:?}"
                )));
            }
        }
    }

    #[allow(clippy::needless_pass_by_value, reason = "stored into GrabRecord on accept")]
    fn on_clipboard_grab(
        &mut self,
        from: ScreenName,
        id: ClipboardId,
        seq: u32,
        buf: &mut Vec<CoordinatorOutput>,
    ) {
        if !self.grabs.on_grab(from.clone(), id, seq) {
            return; // stale
        }
        for name in self.clients.keys() {
            if name != &from {
                buf.push(CoordinatorOutput::Send {
                    to: name.clone(),
                    msg: Message::ClipboardGrab { id, seq },
                });
            }
        }
    }

    #[allow(clippy::needless_pass_by_value, reason = "formatted into warn string")]
    fn on_clipboard_request(
        &mut self,
        from: ScreenName,
        id: ClipboardId,
        seq: u32,
        buf: &mut Vec<CoordinatorOutput>,
    ) {
        let Some(owner) = self.grabs.owner_of(id).cloned() else {
            buf.push(CoordinatorOutput::Warn(format!(
                "clipboard request from '{from}' but no owner recorded for {id:?}"
            )));
            return;
        };
        if owner == self.local_name {
            // Primary-owned clipboard: requires the platform layer to
            // satisfy the read. Deferred to M11.1 (lazy clipboard).
            buf.push(CoordinatorOutput::Warn(format!(
                "clipboard request for primary-owned slot {id:?} — lazy clipboard not implemented yet"
            )));
            return;
        }
        // Forward to the remote owner. They'll answer with ClipboardData.
        buf.push(CoordinatorOutput::Send {
            to: owner,
            msg: Message::ClipboardRequest { id, seq },
        });
    }

    #[allow(clippy::needless_pass_by_value, reason = "compared against keys in loop")]
    fn on_clipboard_data(
        &mut self,
        from: ScreenName,
        id: ClipboardId,
        format: ClipboardFormat,
        data: bytes::Bytes,
        buf: &mut Vec<CoordinatorOutput>,
    ) {
        // MVP: broadcast the payload to every other connected client.
        // Tracking per-requester routing properly needs an extra field
        // on ClipboardRequest/Data — left as future work.
        for name in self.clients.keys() {
            if name != &from {
                buf.push(CoordinatorOutput::Send {
                    to: name.clone(),
                    msg: Message::ClipboardData {
                        id,
                        format,
                        data: data.clone(),
                    },
                });
            }
        }
        // Also surface on the local primary (via InjectLocal) so a
        // future platform-side clipboard write can pick it up.
        buf.push(CoordinatorOutput::InjectLocal(Message::ClipboardData {
            id,
            format,
            data,
        }));
    }
}

fn to_local(virt: (i32, i32), origin_x: i32, origin_y: i32) -> (i32, i32) {
    (
        virt.0.saturating_sub(origin_x),
        virt.1.saturating_sub(origin_y),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinator::layout::{LayoutStore, ScreenEntry, ScreenLayout};
    use hop_common::{ButtonId, KeyId, ModifierMask};

    fn three_screen_layout() -> ScreenLayout {
        ScreenLayout {
            primary: "desk".into(),
            screens: vec![
                ScreenEntry {
                    name: "laptop".into(),
                    origin_x: -1440,
                    origin_y: 0,
                    width: 1440,
                    height: 900,
                },
                ScreenEntry {
                    name: "desk".into(),
                    origin_x: 0,
                    origin_y: 0,
                    width: 1920,
                    height: 1080,
                },
                ScreenEntry {
                    name: "monitor".into(),
                    origin_x: 1920,
                    origin_y: 0,
                    width: 2560,
                    height: 1440,
                },
            ],
        }
    }

    fn make_coord(layout: ScreenLayout) -> Coordinator {
        let store = LayoutStore::from_layout(layout);
        Coordinator::new(store.handle(), "desk".into())
    }

    fn connect(coord: &mut Coordinator, name: &str, buf: &mut Vec<CoordinatorOutput>) {
        coord.on_event(
            CoordinatorEvent::ClientConnected {
                name: name.into(),
                capabilities: vec![],
            },
            buf,
        );
    }

    #[test]
    fn starts_on_primary() {
        let coord = make_coord(three_screen_layout());
        assert_eq!(coord.active(), "desk");
        assert_eq!(coord.cursor(), (960, 540));
    }

    #[test]
    fn connected_client_in_layout_joins_clients_map() {
        let mut coord = make_coord(three_screen_layout());
        let mut buf = vec![];
        connect(&mut coord, "laptop", &mut buf);
        assert!(buf.is_empty(), "no warnings for an in-layout client");
        assert!(coord.clients.contains_key("laptop"));
    }

    #[test]
    fn orphan_client_is_recorded_separately_with_warning() {
        let mut coord = make_coord(three_screen_layout());
        let mut buf = vec![];
        connect(&mut coord, "stranger", &mut buf);
        assert!(coord.orphans.contains_key("stranger"));
        assert!(!coord.clients.contains_key("stranger"));
        assert!(matches!(buf[0], CoordinatorOutput::Warn(_)));
    }

    #[test]
    fn mouse_move_within_primary_produces_no_output() {
        let mut coord = make_coord(three_screen_layout());
        let mut buf = vec![];
        coord.on_event(
            CoordinatorEvent::LocalInput(InputEvent::MouseMove { x: 500, y: 500 }),
            &mut buf,
        );
        buf.clear();
        coord.on_event(
            CoordinatorEvent::LocalInput(InputEvent::MouseMove { x: 600, y: 500 }),
            &mut buf,
        );
        assert!(buf.is_empty(), "primary-local movement is a no-op: {buf:?}");
        assert_eq!(coord.active(), "desk");
    }

    #[test]
    fn crossing_right_edge_enters_monitor() {
        let mut coord = make_coord(three_screen_layout());
        let mut buf = vec![];
        connect(&mut coord, "monitor", &mut buf);
        buf.clear();

        // Parked at desk centre (960, 540). Bootstrap platform pos at
        // (960, 540), then move dx=+1100 → virtual cursor (2060, 540)
        // is inside the monitor rect (origin_x = 1920, width = 2560).
        coord.on_event(
            CoordinatorEvent::LocalInput(InputEvent::MouseMove { x: 960, y: 540 }),
            &mut buf,
        );
        buf.clear();
        coord.on_event(
            CoordinatorEvent::LocalInput(InputEvent::MouseMove { x: 2060, y: 540 }),
            &mut buf,
        );

        assert_eq!(coord.active(), "monitor");
        assert_eq!(coord.seq(), 1);
        match &buf[0] {
            CoordinatorOutput::Send {
                to,
                msg:
                    Message::ScreenEnter {
                        x, y, seq, mask, ..
                    },
            } => {
                assert_eq!(to, "monitor");
                assert_eq!(*x, 140, "2060 - 1920");
                assert_eq!(*y, 540);
                assert_eq!(*seq, 1);
                assert_eq!(*mask, ModifierMask::empty());
            }
            other => panic!("expected ScreenEnter, got {other:?}"),
        }
    }

    #[test]
    fn key_press_while_on_remote_is_forwarded() {
        let mut coord = make_coord(three_screen_layout());
        let mut buf = vec![];
        connect(&mut coord, "monitor", &mut buf);
        buf.clear();

        coord.on_event(
            CoordinatorEvent::LocalInput(InputEvent::MouseMove { x: 960, y: 540 }),
            &mut buf,
        );
        coord.on_event(
            CoordinatorEvent::LocalInput(InputEvent::MouseMove { x: 2060, y: 540 }),
            &mut buf,
        );
        buf.clear();

        coord.on_event(
            CoordinatorEvent::LocalInput(InputEvent::KeyDown {
                key: KeyId::new(0x61),
                mods: ModifierMask::empty(),
            }),
            &mut buf,
        );
        assert_eq!(buf.len(), 1);
        assert!(matches!(
            &buf[0],
            CoordinatorOutput::Send {
                to,
                msg: Message::KeyDown { .. },
            } if to == "monitor"
        ));
    }

    #[test]
    fn held_button_blocks_crossing() {
        let mut coord = make_coord(three_screen_layout());
        let mut buf = vec![];
        connect(&mut coord, "monitor", &mut buf);
        buf.clear();

        coord.on_event(
            CoordinatorEvent::LocalInput(InputEvent::MouseMove { x: 960, y: 540 }),
            &mut buf,
        );
        coord.on_event(
            CoordinatorEvent::LocalInput(InputEvent::MouseButton {
                button: ButtonId::LEFT,
                down: true,
            }),
            &mut buf,
        );
        buf.clear();

        coord.on_event(
            CoordinatorEvent::LocalInput(InputEvent::MouseMove { x: 3000, y: 540 }),
            &mut buf,
        );
        assert_eq!(coord.active(), "desk");
        assert_eq!(coord.cursor(), (1919, 540));
    }

    #[test]
    fn disconnect_of_active_falls_back_to_primary() {
        let mut coord = make_coord(three_screen_layout());
        let mut buf = vec![];
        connect(&mut coord, "monitor", &mut buf);
        buf.clear();
        coord.on_event(
            CoordinatorEvent::LocalInput(InputEvent::MouseMove { x: 960, y: 540 }),
            &mut buf,
        );
        coord.on_event(
            CoordinatorEvent::LocalInput(InputEvent::MouseMove { x: 2060, y: 540 }),
            &mut buf,
        );
        buf.clear();
        assert_eq!(coord.active(), "monitor");

        coord.on_event(
            CoordinatorEvent::ClientDisconnected {
                name: "monitor".into(),
            },
            &mut buf,
        );
        assert_eq!(coord.active(), "desk");
        assert!(buf.iter().any(|o| matches!(o, CoordinatorOutput::Warn(_))));
    }

    #[test]
    fn clipboard_grab_broadcasts_to_other_clients() {
        let mut coord = make_coord(three_screen_layout());
        let mut buf = vec![];
        connect(&mut coord, "laptop", &mut buf);
        connect(&mut coord, "monitor", &mut buf);
        buf.clear();

        coord.on_event(
            CoordinatorEvent::PeerMessage {
                from: "laptop".into(),
                msg: Message::ClipboardGrab {
                    id: ClipboardId::Clipboard,
                    seq: 1,
                },
            },
            &mut buf,
        );

        let sends: Vec<_> = buf
            .iter()
            .filter_map(|o| match o {
                CoordinatorOutput::Send {
                    to,
                    msg: Message::ClipboardGrab { .. },
                } => Some(to.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(sends, vec!["monitor"], "broadcast to others only");
    }

    #[test]
    fn clipboard_grab_stale_seq_is_dropped() {
        let mut coord = make_coord(three_screen_layout());
        let mut buf = vec![];
        connect(&mut coord, "laptop", &mut buf);
        connect(&mut coord, "monitor", &mut buf);

        coord.on_event(
            CoordinatorEvent::PeerMessage {
                from: "laptop".into(),
                msg: Message::ClipboardGrab {
                    id: ClipboardId::Clipboard,
                    seq: 5,
                },
            },
            &mut buf,
        );
        buf.clear();

        coord.on_event(
            CoordinatorEvent::PeerMessage {
                from: "monitor".into(),
                msg: Message::ClipboardGrab {
                    id: ClipboardId::Clipboard,
                    seq: 3,
                },
            },
            &mut buf,
        );
        assert!(buf.is_empty(), "stale grab dropped silently");
    }

    #[test]
    fn clipboard_request_forwards_to_owner() {
        let mut coord = make_coord(three_screen_layout());
        let mut buf = vec![];
        connect(&mut coord, "laptop", &mut buf);
        connect(&mut coord, "monitor", &mut buf);

        coord.on_event(
            CoordinatorEvent::PeerMessage {
                from: "laptop".into(),
                msg: Message::ClipboardGrab {
                    id: ClipboardId::Clipboard,
                    seq: 1,
                },
            },
            &mut buf,
        );
        buf.clear();

        coord.on_event(
            CoordinatorEvent::PeerMessage {
                from: "monitor".into(),
                msg: Message::ClipboardRequest {
                    id: ClipboardId::Clipboard,
                    seq: 1,
                },
            },
            &mut buf,
        );
        assert!(matches!(
            &buf[0],
            CoordinatorOutput::Send {
                to,
                msg: Message::ClipboardRequest { .. },
            } if to == "laptop"
        ));
    }
}
