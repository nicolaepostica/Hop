// The X11 selection dance is stateful enough that the pedantic
// "match-same-arms" / "redundant-closure" lints get in the way more
// than they help; silence them at module scope.
#![allow(
    clippy::match_same_arms,
    clippy::needless_continue,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_closure,
    clippy::single_match_else,
    clippy::manual_let_else
)]

//! X11 selection-based clipboard worker.
//!
//! Runs on a dedicated OS thread with its own `RustConnection` so the
//! injection path in [`X11Screen`] is never blocked by a selection
//! round-trip. Communicates via a `std::sync::mpsc` channel; replies go
//! back over `tokio::sync::oneshot` so async callers can `.await` them.
//!
//! Covers the `CLIPBOARD` selection only for M4. Supports two formats:
//! plain UTF-8 text (`UTF8_STRING` target) and HTML (`text/html`).
//! `PRIMARY` selection and bitmap payloads are follow-ups.

use std::collections::HashMap;
use std::sync::mpsc;
use std::thread::{Builder, JoinHandle};
use std::time::{Duration, Instant};

use bytes::Bytes;
use input_leap_common::{ClipboardFormat, ClipboardId};
use input_leap_platform::PlatformError;
use tokio::sync::oneshot;
use tracing::{debug, warn};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, Property,
    SelectionNotifyEvent, SelectionRequestEvent, Window, WindowClass, SELECTION_NOTIFY_EVENT,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT, CURRENT_TIME};

/// How long we wait for a `SelectionNotify` when reading the clipboard.
const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// Commands sent to the worker thread.
enum Cmd {
    Read {
        id: ClipboardId,
        format: ClipboardFormat,
        reply: oneshot::Sender<Result<Bytes, PlatformError>>,
    },
    Write {
        id: ClipboardId,
        format: ClipboardFormat,
        data: Bytes,
        reply: oneshot::Sender<Result<(), PlatformError>>,
    },
    Shutdown,
}

/// Async handle to the clipboard worker.
pub struct X11Clipboard {
    tx: mpsc::Sender<Cmd>,
    join: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for X11Clipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("X11Clipboard").finish_non_exhaustive()
    }
}

impl X11Clipboard {
    /// Spawn a worker thread bound to `display` (or `$DISPLAY` if
    /// `None`). The thread exits when this handle is dropped.
    pub fn spawn(display: Option<&str>) -> Result<Self, PlatformError> {
        let (tx, rx) = mpsc::channel();
        let display = display.map(ToOwned::to_owned);
        let join = Builder::new()
            .name("x11-clipboard".into())
            .spawn(move || {
                if let Err(err) = run_worker(display.as_deref(), &rx) {
                    warn!(error = %err, "X11 clipboard worker exited");
                }
            })
            .map_err(|e| PlatformError::Other(e.to_string()))?;
        Ok(Self {
            tx,
            join: Some(join),
        })
    }

    /// Read the current selection in the requested format. Returns an
    /// empty `Bytes` when the selection is empty or the format is not
    /// on offer.
    pub async fn read(
        &self,
        id: ClipboardId,
        format: ClipboardFormat,
    ) -> Result<Bytes, PlatformError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(Cmd::Read {
                id,
                format,
                reply: reply_tx,
            })
            .map_err(|_| PlatformError::Other("clipboard worker gone".into()))?;
        reply_rx
            .await
            .map_err(|_| PlatformError::Other("clipboard reply dropped".into()))?
    }

    /// Take ownership of the selection and serve this payload for
    /// subsequent read requests.
    pub async fn write(
        &self,
        id: ClipboardId,
        format: ClipboardFormat,
        data: Bytes,
    ) -> Result<(), PlatformError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(Cmd::Write {
                id,
                format,
                data,
                reply: reply_tx,
            })
            .map_err(|_| PlatformError::Other("clipboard worker gone".into()))?;
        reply_rx
            .await
            .map_err(|_| PlatformError::Other("clipboard reply dropped".into()))?
    }
}

impl Drop for X11Clipboard {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Shutdown);
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
}

// ----- worker -----------------------------------------------------------

/// Cached atoms the worker uses; interned once at startup.
struct Atoms {
    clipboard: Atom,
    primary: Atom,
    utf8_string: Atom,
    html: Atom,
    targets: Atom,
    reply_prop: Atom,
}

impl Atoms {
    fn intern(conn: &RustConnection) -> Result<Self, PlatformError> {
        fn intern_one(conn: &RustConnection, name: &[u8]) -> Result<Atom, PlatformError> {
            conn.intern_atom(false, name)
                .map_err(wrap)?
                .reply()
                .map(|r| r.atom)
                .map_err(wrap)
        }
        Ok(Self {
            clipboard: intern_one(conn, b"CLIPBOARD")?,
            primary: intern_one(conn, b"PRIMARY")?,
            utf8_string: intern_one(conn, b"UTF8_STRING")?,
            html: intern_one(conn, b"text/html")?,
            targets: intern_one(conn, b"TARGETS")?,
            reply_prop: intern_one(conn, b"INPUT_LEAP_CLIPBOARD")?,
        })
    }

    fn selection_atom(&self, id: ClipboardId) -> Atom {
        match id {
            ClipboardId::Clipboard => self.clipboard,
            ClipboardId::Primary => self.primary,
        }
    }

    fn format_target(&self, format: ClipboardFormat) -> Option<Atom> {
        match format {
            ClipboardFormat::Text => Some(self.utf8_string),
            ClipboardFormat::Html => Some(self.html),
            _ => None,
        }
    }
}

/// In-memory store of owned selections: `(selection, target) -> bytes`.
#[derive(Default)]
struct OwnedStore {
    entries: HashMap<(Atom, Atom), Bytes>,
}

impl OwnedStore {
    fn set(&mut self, selection: Atom, target: Atom, data: Bytes) {
        self.entries.insert((selection, target), data);
    }

    fn get(&self, selection: Atom, target: Atom) -> Option<&Bytes> {
        self.entries.get(&(selection, target))
    }

    fn targets_for(&self, selection: Atom) -> Vec<Atom> {
        self.entries
            .keys()
            .filter(|(sel, _)| *sel == selection)
            .map(|(_, t)| *t)
            .collect()
    }
}

fn run_worker(display: Option<&str>, rx: &mpsc::Receiver<Cmd>) -> Result<(), PlatformError> {
    let (conn, screen_num) = x11rb::connect(display).map_err(wrap)?;
    let root = conn.setup().roots[screen_num].root;

    // Create an invisible owner window to attach selections to and
    // receive SelectionRequest events on.
    let window = conn.generate_id().map_err(wrap)?;
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        window,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        COPY_FROM_PARENT,
        &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )
    .map_err(wrap)?
    .check()
    .map_err(wrap)?;

    let atoms = Atoms::intern(&conn)?;
    let mut owned = OwnedStore::default();

    loop {
        // Drain any X events that are already pending before blocking
        // on the command channel.
        drain_events(&conn, window, &atoms, &owned);

        match rx.recv_timeout(Duration::from_millis(50)) {
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(Cmd::Shutdown) => break,
            Ok(Cmd::Read { id, format, reply }) => {
                let result = read_selection(&conn, window, &atoms, &owned, id, format);
                let _ = reply.send(result);
            }
            Ok(Cmd::Write {
                id,
                format,
                data,
                reply,
            }) => {
                let result = write_selection(&conn, window, &atoms, &mut owned, id, format, data);
                let _ = reply.send(result);
            }
        }
    }

    let _ = conn.destroy_window(window).map(|c| c.check());
    Ok(())
}

fn drain_events(conn: &RustConnection, window: Window, atoms: &Atoms, owned: &OwnedStore) {
    while let Ok(Some(event)) = conn.poll_for_event() {
        match event {
            Event::SelectionRequest(req) => {
                if let Err(err) = handle_selection_request(conn, window, atoms, owned, req) {
                    warn!(error = %err, "failed to service SelectionRequest");
                }
            }
            Event::SelectionClear(_) => {
                // Another app took ownership; we just stop responding
                // and keep our stored data for future writes.
                debug!("lost clipboard selection (another app took ownership)");
            }
            _ => {}
        }
    }
}

fn handle_selection_request(
    conn: &RustConnection,
    _window: Window,
    atoms: &Atoms,
    owned: &OwnedStore,
    req: SelectionRequestEvent,
) -> Result<(), PlatformError> {
    // Property = 0 means the requestor wants the data in its own
    // well-known location — we must write to req.target instead.
    let target_property = if req.property == x11rb::NONE {
        req.target
    } else {
        req.property
    };

    let reply_property = if req.target == atoms.targets {
        let targets = owned.targets_for(req.selection);
        conn.change_property32(
            PropMode::REPLACE,
            req.requestor,
            target_property,
            AtomEnum::ATOM,
            &targets,
        )
        .map_err(wrap)?
        .check()
        .map_err(wrap)?;
        target_property
    } else if let Some(data) = owned.get(req.selection, req.target) {
        conn.change_property(
            PropMode::REPLACE,
            req.requestor,
            target_property,
            req.target,
            8,
            u32::try_from(data.len()).unwrap_or(u32::MAX),
            data.as_ref(),
        )
        .map_err(wrap)?
        .check()
        .map_err(wrap)?;
        target_property
    } else {
        // Tell the requestor we can't satisfy it.
        x11rb::NONE
    };

    let notify = SelectionNotifyEvent {
        response_type: SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time: req.time,
        requestor: req.requestor,
        selection: req.selection,
        target: req.target,
        property: reply_property,
    };
    conn.send_event(false, req.requestor, EventMask::NO_EVENT, notify)
        .map_err(wrap)?
        .check()
        .map_err(wrap)?;
    conn.flush().map_err(wrap)?;
    Ok(())
}

fn write_selection(
    conn: &RustConnection,
    window: Window,
    atoms: &Atoms,
    owned: &mut OwnedStore,
    id: ClipboardId,
    format: ClipboardFormat,
    data: Bytes,
) -> Result<(), PlatformError> {
    let Some(target) = atoms.format_target(format) else {
        return Err(PlatformError::Other(format!(
            "X11 clipboard: format {format:?} not supported yet"
        )));
    };
    let selection = atoms.selection_atom(id);
    owned.set(selection, target, data);
    conn.set_selection_owner(window, selection, CURRENT_TIME)
        .map_err(wrap)?
        .check()
        .map_err(wrap)?;
    conn.flush().map_err(wrap)?;
    Ok(())
}

fn read_selection(
    conn: &RustConnection,
    window: Window,
    atoms: &Atoms,
    owned: &OwnedStore,
    id: ClipboardId,
    format: ClipboardFormat,
) -> Result<Bytes, PlatformError> {
    let Some(target) = atoms.format_target(format) else {
        return Ok(Bytes::new());
    };
    let selection = atoms.selection_atom(id);

    // Clear any stale reply first so we don't read old data.
    let _ = conn
        .delete_property(window, atoms.reply_prop)
        .map(|c| c.check());

    conn.convert_selection(window, selection, target, atoms.reply_prop, CURRENT_TIME)
        .map_err(wrap)?
        .check()
        .map_err(wrap)?;
    conn.flush().map_err(wrap)?;

    let deadline = Instant::now() + READ_TIMEOUT;
    loop {
        if Instant::now() >= deadline {
            return Err(PlatformError::Other(
                "timeout waiting for selection owner".into(),
            ));
        }
        let event = match conn.poll_for_event().map_err(wrap)? {
            Some(e) => e,
            None => {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
        };
        match event {
            Event::SelectionNotify(ev) if ev.requestor == window => {
                if ev.property == x11rb::NONE {
                    // Owner couldn't give us this format.
                    return Ok(Bytes::new());
                }
                return fetch_property(conn, window, atoms.reply_prop, ev.target);
            }
            Event::SelectionRequest(req) => {
                // We are the owner — service our own request inline so
                // the SelectionNotify actually shows up in our queue.
                if let Err(err) = handle_selection_request(conn, window, atoms, owned, req) {
                    warn!(error = %err, "failed to service SelectionRequest (inline)");
                }
            }
            Event::SelectionClear(_) => {
                // Someone else took ownership while we were reading —
                // nothing for us to do here, keep polling.
            }
            _ => {}
        }
    }
}

fn fetch_property(
    conn: &RustConnection,
    window: Window,
    property: Atom,
    target: Atom,
) -> Result<Bytes, PlatformError> {
    // Reading in one shot; INCR handling for >256 KiB clipboards is TODO.
    let reply = conn
        .get_property(true, window, property, target, 0, u32::MAX / 4)
        .map_err(wrap)?
        .reply()
        .map_err(wrap)?;
    if reply.type_ == x11rb::NONE {
        return Ok(Bytes::new());
    }
    Ok(Bytes::from(reply.value))
}

fn wrap<E: std::fmt::Display>(err: E) -> PlatformError {
    PlatformError::Other(err.to_string())
}

// Re-export for the screen module to avoid an unused-import warning
// while this module's types live here.
#[allow(dead_code, reason = "exposed via X11Screen in a follow-up patch")]
pub(crate) fn _unused_force_compile(_: Property) {}
