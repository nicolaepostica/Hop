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
//! Architecture:
//!
//! - A dedicated **worker thread** owns a [`RustConnection`] and all
//!   mutable state (owned-selection store, pending reads). Both the
//!   [`X11Screen`](crate::X11Screen) injection path and other apps
//!   reach the worker through channels, never directly — the X
//!   connection is single-threaded.
//! - A second **X reader thread** sits in a blocking
//!   [`Connection::wait_for_event`] loop and forwards every event
//!   through a `crossbeam_channel`. This eliminates the polling
//!   `recv_timeout(50 ms)` + `sleep(10 ms)` busy-waits that the first
//!   implementation used; the worker now sleeps in
//!   `crossbeam_channel::select!` until there is either a command
//!   from an async caller or an X event to service.
//! - Clipboard **reads** are decoupled from the `read()` call. The
//!   worker sends `ConvertSelection`, pushes a [`PendingRead`] into a
//!   FIFO, and returns to the event loop. When the matching
//!   `SelectionNotify` arrives it pops the front, fetches the
//!   property, and fulfils the [`tokio::sync::oneshot`]. Timeouts are
//!   enforced on the async side via [`tokio::time::timeout`].
//!
//! Covers the `CLIPBOARD` selection only for M4. Supports two formats:
//! plain UTF-8 text (`UTF8_STRING` target) and HTML (`text/html`).
//! `PRIMARY` selection and bitmap payloads are follow-ups.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::thread::{Builder, JoinHandle};
use std::time::Duration;

use bytes::Bytes;
use crossbeam_channel::{select, unbounded, Receiver, Sender};
use input_leap_common::{ClipboardFormat, ClipboardId};
use input_leap_platform::PlatformError;
use tokio::sync::oneshot;
use tracing::{debug, warn};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask, PropMode,
    Property, SelectionNotifyEvent, SelectionRequestEvent, Window, WindowClass,
    CLIENT_MESSAGE_EVENT, SELECTION_NOTIFY_EVENT,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT, CURRENT_TIME};

/// How long an async caller waits for a clipboard read before giving up.
///
/// Enforced on the tokio side via `tokio::time::timeout`. The worker
/// itself has no timeout — it simply tracks pending reads in a FIFO.
pub const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// Commands sent to the worker thread from async callers.
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
    tx: Sender<Cmd>,
    worker: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for X11Clipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("X11Clipboard").finish_non_exhaustive()
    }
}

impl X11Clipboard {
    /// Spawn the worker + reader threads bound to `display` (or
    /// `$DISPLAY` if `None`). Both threads exit when this handle is
    /// dropped.
    pub fn spawn(display: Option<&str>) -> Result<Self, PlatformError> {
        let (tx, rx) = unbounded::<Cmd>();
        let display = display.map(ToOwned::to_owned);
        let worker = Builder::new()
            .name("x11-clipboard".into())
            .spawn(move || {
                if let Err(err) = run_worker(display.as_deref(), &rx) {
                    warn!(error = %err, "X11 clipboard worker exited");
                }
            })
            .map_err(|e| PlatformError::Other(format!("spawn clipboard thread: {e}")))?;
        Ok(Self {
            tx,
            worker: Some(worker),
        })
    }

    /// Read the current selection in the requested format. Returns an
    /// empty [`Bytes`] when the selection is empty or the format is
    /// not on offer.
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
        match tokio::time::timeout(READ_TIMEOUT, reply_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(PlatformError::Other("clipboard reply dropped".into())),
            Err(_) => Err(PlatformError::ClipboardTimeout),
        }
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
        if let Some(handle) = self.worker.take() {
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
    wakeup: Atom,
}

impl Atoms {
    fn intern(conn: &RustConnection) -> Result<Self, PlatformError> {
        fn intern_one(conn: &RustConnection, name: &[u8]) -> Result<Atom, PlatformError> {
            conn.intern_atom(false, name)
                .map_err(PlatformError::connection_lost)?
                .reply()
                .map(|r| r.atom)
                .map_err(PlatformError::connection_lost)
        }
        Ok(Self {
            clipboard: intern_one(conn, b"CLIPBOARD")?,
            primary: intern_one(conn, b"PRIMARY")?,
            utf8_string: intern_one(conn, b"UTF8_STRING")?,
            html: intern_one(conn, b"text/html")?,
            targets: intern_one(conn, b"TARGETS")?,
            reply_prop: intern_one(conn, b"INPUT_LEAP_CLIPBOARD")?,
            wakeup: intern_one(conn, b"INPUT_LEAP_WAKEUP")?,
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

/// A read waiting for its `SelectionNotify` to come back.
struct PendingRead {
    /// Selection atom the reader asked for; used only for logging.
    #[allow(dead_code, reason = "useful for future multi-selection debugging")]
    selection: Atom,
    /// Target atom (format) the reader asked for.
    target: Atom,
    /// Where to send the final payload.
    reply: oneshot::Sender<Result<Bytes, PlatformError>>,
}

fn run_worker(display: Option<&str>, cmd_rx: &Receiver<Cmd>) -> Result<(), PlatformError> {
    let (conn, screen_num) = x11rb::connect(display).map_err(|e| {
        PlatformError::Unavailable(format!("clipboard: cannot open X display: {e}"))
    })?;
    let conn = Arc::new(conn);
    let root = conn.setup().roots[screen_num].root;

    // Create an invisible owner window to attach selections to and
    // receive SelectionRequest events on.
    let window = conn.generate_id().map_err(PlatformError::connection_lost)?;
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
    .map_err(PlatformError::connection_lost)?
    .check()
    .map_err(PlatformError::connection_lost)?;

    let atoms = Atoms::intern(&conn)?;
    let mut owned = OwnedStore::default();
    let mut pending = VecDeque::<PendingRead>::new();

    // Spawn the X reader thread. It blocks in wait_for_event and
    // forwards every event back to us on a crossbeam channel.
    let (xevt_tx, xevt_rx) = unbounded::<Event>();
    let reader_conn = Arc::clone(&conn);
    let reader = Builder::new()
        .name("x11-clipboard-reader".into())
        .spawn(move || loop {
            match reader_conn.wait_for_event() {
                Ok(ev) => {
                    if xevt_tx.send(ev).is_err() {
                        break; // main worker dropped the receiver — we're done.
                    }
                }
                Err(err) => {
                    warn!(error = %err, "X clipboard reader connection lost");
                    break;
                }
            }
        })
        .map_err(|e| PlatformError::Other(format!("spawn clipboard reader thread: {e}")))?;

    'outer: loop {
        select! {
            recv(cmd_rx) -> msg => {
                match msg {
                    Err(_) | Ok(Cmd::Shutdown) => break 'outer,
                    Ok(Cmd::Read { id, format, reply }) => {
                        dispatch_read(&conn, window, &atoms, &mut pending, id, format, reply);
                    }
                    Ok(Cmd::Write { id, format, data, reply }) => {
                        let result =
                            write_selection(&conn, window, &atoms, &mut owned, id, format, data);
                        let _ = reply.send(result);
                    }
                }
            }
            recv(xevt_rx) -> ev => {
                let Ok(ev) = ev else { break 'outer; };
                handle_x_event(&conn, window, &atoms, &owned, &mut pending, ev);
            }
        }
    }

    // Graceful shutdown. Unblock any still-pending reads, wake the
    // reader with a self-sent ClientMessage so it can exit its
    // wait_for_event, then join.
    while let Some(pr) = pending.pop_front() {
        let _ = pr
            .reply
            .send(Err(PlatformError::Other(
                "clipboard worker shutting down".into(),
            )));
    }
    drop(xevt_rx); // reader's send() will start failing on next event.
    wake_reader(&conn, window, atoms.wakeup);
    let _ = reader.join();

    let _ = conn.destroy_window(window).map(|c| c.check());
    Ok(())
}

/// Issue a `ConvertSelection` and park a `PendingRead` in the queue.
fn dispatch_read(
    conn: &RustConnection,
    window: Window,
    atoms: &Atoms,
    pending: &mut VecDeque<PendingRead>,
    id: ClipboardId,
    format: ClipboardFormat,
    reply: oneshot::Sender<Result<Bytes, PlatformError>>,
) {
    let Some(target) = atoms.format_target(format) else {
        // Format we don't implement on this backend — return empty so
        // the caller can try another format from the peer's offer.
        let _ = reply.send(Ok(Bytes::new()));
        return;
    };
    let selection = atoms.selection_atom(id);

    // Clear any stale reply property so we don't read old data if the
    // owner flakes out and never responds.
    let _ = conn
        .delete_property(window, atoms.reply_prop)
        .map(|c| c.check());

    if let Err(err) =
        conn.convert_selection(window, selection, target, atoms.reply_prop, CURRENT_TIME)
    {
        let _ = reply.send(Err(PlatformError::connection_lost(err)));
        return;
    }
    if let Err(err) = conn.flush() {
        let _ = reply.send(Err(PlatformError::connection_lost(err)));
        return;
    }

    pending.push_back(PendingRead {
        selection,
        target,
        reply,
    });
}

#[allow(clippy::needless_pass_by_value, reason = "consumed by match bindings")]
fn handle_x_event(
    conn: &RustConnection,
    window: Window,
    atoms: &Atoms,
    owned: &OwnedStore,
    pending: &mut VecDeque<PendingRead>,
    ev: Event,
) {
    match ev {
        Event::SelectionRequest(req) => {
            if let Err(err) = handle_selection_request(conn, window, atoms, owned, req) {
                warn!(error = %err, "failed to service SelectionRequest");
            }
        }
        Event::SelectionNotify(ev) if ev.requestor == window => {
            let Some(pr) = pending.pop_front() else {
                debug!("received SelectionNotify with no pending read");
                return;
            };
            let result = if ev.property == x11rb::NONE {
                // Owner could not satisfy our request.
                Ok(Bytes::new())
            } else {
                fetch_property(conn, window, atoms.reply_prop, pr.target)
            };
            let _ = pr.reply.send(result);
        }
        Event::SelectionClear(_) => {
            debug!("lost clipboard selection (another app took ownership)");
        }
        Event::ClientMessage(msg) if msg.type_ == atoms.wakeup => {
            // Dummy event we sent to ourselves during shutdown.
        }
        _ => {}
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
        .map_err(PlatformError::connection_lost)?
        .check()
        .map_err(PlatformError::connection_lost)?;
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
        .map_err(PlatformError::connection_lost)?
        .check()
        .map_err(PlatformError::connection_lost)?;
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
        .map_err(PlatformError::connection_lost)?
        .check()
        .map_err(PlatformError::connection_lost)?;
    conn.flush().map_err(PlatformError::connection_lost)?;
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
        return Err(PlatformError::UnsupportedFormat { format });
    };
    let selection = atoms.selection_atom(id);
    owned.set(selection, target, data);
    conn.set_selection_owner(window, selection, CURRENT_TIME)
        .map_err(PlatformError::connection_lost)?
        .check()
        .map_err(PlatformError::connection_lost)?;
    conn.flush().map_err(PlatformError::connection_lost)?;
    Ok(())
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
        .map_err(PlatformError::connection_lost)?
        .reply()
        .map_err(PlatformError::connection_lost)?;
    if reply.type_ == x11rb::NONE {
        return Ok(Bytes::new());
    }
    Ok(Bytes::from(reply.value))
}

/// Send a `ClientMessage` to our own window so the reader thread's
/// `wait_for_event` returns and it can notice that its outbound channel
/// has been closed.
fn wake_reader(conn: &RustConnection, window: Window, wakeup_atom: Atom) {
    let msg = ClientMessageEvent {
        response_type: CLIENT_MESSAGE_EVENT,
        format: 32,
        sequence: 0,
        window,
        type_: wakeup_atom,
        data: [0u8; 20].into(),
    };
    let _ = conn
        .send_event(false, window, EventMask::NO_EVENT, msg)
        .map(|c| c.check());
    let _ = conn.flush();
}

// Re-export for the screen module to avoid an unused-import warning
// while this module's types live here.
#[allow(dead_code, reason = "exposed via X11Screen in a follow-up patch")]
pub(crate) fn _unused_force_compile(_: Property) {}
