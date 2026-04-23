//! Tokio driver for the [`Coordinator`] state machine.
//!
//! The [`Coordinator`] itself is pure (`fn on_event(&mut …)`) and
//! trivially testable. This module is the I/O halo around it:
//!
//!  - owns the `HashMap<ScreenName, mpsc::Sender<Message>>` that maps
//!    coordinator-produced `Send { to, msg }` outputs to a concrete
//!    per-peer channel;
//!  - owns the `mpsc::Sender<Message>` going to the platform
//!    dispatcher for [`CoordinatorOutput::InjectLocal`];
//!  - consumes a single [`CoordinatorCommand`] channel so every caller
//!    (proxies, the local-input forwarder, the accept loop) has one
//!    place to hand work in;
//!  - applies the spec's backpressure policy — `try_send` + drop the
//!    peer on full — so a slow client cannot stall the whole server.
//!
//! The driver owns the Coordinator; nobody else holds a reference, so
//! locking never comes up.

use std::collections::HashMap;
use std::sync::Arc;

use input_leap_platform::PlatformScreen;
use input_leap_protocol::{Capability, Message};
use tokio::select;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::coordinator::layout::{ScreenName, SharedLayout};
use crate::coordinator::state::{Coordinator, CoordinatorEvent, CoordinatorOutput};

/// Everything the driver task accepts through its command channel.
///
/// Kept as a single enum rather than two separate channels so there is
/// one FIFO order across "client registered" / "peer message arrived"
/// — the coordinator never sees a `PeerMessage` from a client it has
/// not yet registered.
#[derive(Debug)]
pub enum CoordinatorCommand {
    /// A pure state-machine event (peer message, disconnect, local input).
    Event(CoordinatorEvent),
    /// A peer finished its handshake. The driver stores `outbound` in
    /// its sender table and feeds a `ClientConnected` event into the
    /// coordinator.
    RegisterClient {
        /// Display name from `Hello`.
        name: ScreenName,
        /// Where to deliver `Send { to: name, .. }` outputs.
        outbound: mpsc::Sender<Message>,
        /// Capabilities advertised in `Hello`.
        capabilities: Vec<Capability>,
    },
}

/// Channels callers hand work into. Cheap to clone.
#[derive(Debug, Clone)]
pub struct CoordinatorHandle {
    /// Push a [`CoordinatorCommand`] into the driver.
    pub commands_tx: mpsc::Sender<CoordinatorCommand>,
}

impl CoordinatorHandle {
    /// Convenience: push a raw [`CoordinatorEvent`].
    pub async fn send_event(
        &self,
        event: CoordinatorEvent,
    ) -> Result<(), mpsc::error::SendError<CoordinatorCommand>> {
        self.commands_tx.send(CoordinatorCommand::Event(event)).await
    }

    /// Convenience: register a newly-connected client.
    pub async fn register_client(
        &self,
        name: ScreenName,
        outbound: mpsc::Sender<Message>,
        capabilities: Vec<Capability>,
    ) -> Result<(), mpsc::error::SendError<CoordinatorCommand>> {
        self.commands_tx
            .send(CoordinatorCommand::RegisterClient {
                name,
                outbound,
                capabilities,
            })
            .await
    }
}

/// Default command-channel capacity. Coordinators never burn CPU, so
/// this mostly protects against a pathological event-stream burst.
pub const COMMAND_CHANNEL_CAPACITY: usize = 4096;

/// Default per-client outbound-channel capacity. Consistent with the
/// M11 spec: 1024 entries, `try_send` with drop-on-full.
pub const OUTBOUND_CHANNEL_CAPACITY: usize = 1024;

/// Spawn the coordinator task and the platform dispatcher.
///
/// Returns a [`CoordinatorHandle`] callers use to hand in commands,
/// plus the two `JoinHandle`s for graceful shutdown.
pub fn spawn_coordinator<S>(
    layout: SharedLayout,
    local_name: ScreenName,
    screen: Arc<S>,
    shutdown: &CancellationToken,
) -> (CoordinatorHandle, JoinHandle<()>, JoinHandle<()>)
where
    S: PlatformScreen + 'static,
{
    let (commands_tx, commands_rx) = mpsc::channel::<CoordinatorCommand>(COMMAND_CHANNEL_CAPACITY);
    let (platform_tx, platform_rx) = mpsc::channel::<Message>(OUTBOUND_CHANNEL_CAPACITY);

    let coord_shutdown = shutdown.clone();
    let coord_task = tokio::spawn(async move {
        run_coordinator(layout, local_name, commands_rx, platform_tx, coord_shutdown).await;
    });

    let dispatcher_shutdown = shutdown.clone();
    let dispatcher_task = tokio::spawn(async move {
        run_platform_dispatcher(screen, platform_rx, dispatcher_shutdown).await;
    });

    (
        CoordinatorHandle { commands_tx },
        coord_task,
        dispatcher_task,
    )
}

async fn run_coordinator(
    layout: SharedLayout,
    local_name: ScreenName,
    mut commands_rx: mpsc::Receiver<CoordinatorCommand>,
    platform_tx: mpsc::Sender<Message>,
    shutdown: CancellationToken,
) {
    let mut coord = Coordinator::new(layout, local_name);
    let mut senders: HashMap<ScreenName, mpsc::Sender<Message>> = HashMap::new();
    let mut out_buf: Vec<CoordinatorOutput> = Vec::with_capacity(16);

    loop {
        select! {
            biased;

            () = shutdown.cancelled() => {
                debug!("coordinator task: shutdown requested");
                break;
            }

            command = commands_rx.recv() => {
                match command {
                    Some(CoordinatorCommand::RegisterClient {
                        name,
                        outbound,
                        capabilities,
                    }) => {
                        senders.insert(name.clone(), outbound);
                        coord.on_event(
                            CoordinatorEvent::ClientConnected {
                                name,
                                capabilities,
                            },
                            &mut out_buf,
                        );
                        flush_outputs(&mut out_buf, &mut senders, &platform_tx);
                    }
                    Some(CoordinatorCommand::Event(event)) => {
                        // Remove a departing client's sender **before**
                        // the coordinator runs so any late Send outputs
                        // don't try to route to a now-dead channel.
                        if let CoordinatorEvent::ClientDisconnected { name } = &event {
                            senders.remove(name);
                        }
                        coord.on_event(event, &mut out_buf);
                        flush_outputs(&mut out_buf, &mut senders, &platform_tx);
                    }
                    None => {
                        debug!("coordinator task: command channel closed");
                        break;
                    }
                }
            }
        }
    }
}

/// Drain every queued [`CoordinatorOutput`] onto the right channel.
///
/// `Send` → per-client `try_send`; on `Full` we drop the client as the
/// spec requires (sender removed; when the proxy notices its outbound
/// channel closed, it will disconnect and send us
/// `ClientDisconnected`). `InjectLocal` → the platform dispatcher.
/// `Warn` → `tracing::warn!`.
fn flush_outputs(
    buf: &mut Vec<CoordinatorOutput>,
    senders: &mut HashMap<ScreenName, mpsc::Sender<Message>>,
    platform_tx: &mpsc::Sender<Message>,
) {
    for output in buf.drain(..) {
        match output {
            CoordinatorOutput::Send { to, msg } => {
                let Some(sender) = senders.get(&to) else {
                    debug!(peer = %to, "Send to unknown client, dropping");
                    continue;
                };
                match sender.try_send(msg) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        warn!(
                            peer = %to,
                            "client outbound channel full; dropping client due to backpressure"
                        );
                        senders.remove(&to);
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        debug!(peer = %to, "client outbound channel closed; removing");
                        senders.remove(&to);
                    }
                }
            }
            CoordinatorOutput::InjectLocal(msg) => {
                if let Err(err) = platform_tx.try_send(msg) {
                    warn!(error = ?err, "platform dispatcher channel full or closed");
                }
            }
            CoordinatorOutput::Warn(text) => {
                warn!(message = %text, "coordinator");
            }
        }
    }
}

/// Forward whatever the coordinator wants injected locally into real
/// platform calls.
///
/// Only a subset of `Message` makes sense here — anything the
/// coordinator would want to replay on the primary's own screen
/// (keyboard/mouse injection is unused today; clipboard writes land
/// here when a remote peer pushes `ClipboardData`).
async fn run_platform_dispatcher<S>(
    screen: Arc<S>,
    mut rx: mpsc::Receiver<Message>,
    shutdown: CancellationToken,
) where
    S: PlatformScreen,
{
    loop {
        select! {
            biased;
            () = shutdown.cancelled() => {
                debug!("platform dispatcher: shutdown requested");
                break;
            }
            msg = rx.recv() => {
                let Some(msg) = msg else {
                    debug!("platform dispatcher: channel closed");
                    break;
                };
                dispatch_to_platform(&screen, msg).await;
            }
        }
    }
}

async fn dispatch_to_platform<S>(screen: &Arc<S>, msg: Message)
where
    S: PlatformScreen,
{
    match msg {
        Message::KeyDown { key, mods } => {
            log_err(screen.inject_key(key, mods, true).await, "inject_key down");
        }
        Message::KeyUp { key, mods } => {
            log_err(screen.inject_key(key, mods, false).await, "inject_key up");
        }
        Message::KeyRepeat { key, mods, count } => {
            for _ in 0..count {
                log_err(screen.inject_key(key, mods, true).await, "inject_key repeat");
            }
        }
        Message::MouseMove { x, y } => {
            log_err(screen.inject_mouse_move(x, y).await, "inject_mouse_move");
        }
        Message::MouseButton { button, down } => {
            log_err(screen.inject_mouse_button(button, down).await, "inject_mouse_button");
        }
        Message::MouseWheel { dx, dy } => {
            log_err(screen.inject_mouse_wheel(dx, dy).await, "inject_mouse_wheel");
        }
        Message::ClipboardData { id, format, data } => {
            log_err(screen.set_clipboard(id, format, data).await, "set_clipboard");
        }
        other => {
            debug!(?other, "platform dispatcher: ignoring non-actionable message");
        }
    }
}

fn log_err<E: std::fmt::Debug>(result: Result<(), E>, what: &'static str) {
    if let Err(err) = result {
        warn!(error = ?err, "platform dispatcher: {what} failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinator::layout::{LayoutStore, ScreenEntry, ScreenLayout};
    use input_leap_common::{ClipboardFormat, ClipboardId};
    use input_leap_platform::{InputEvent, MockScreen};
    use tokio::time::{timeout, Duration};

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

    #[tokio::test]
    async fn registered_client_receives_screen_enter_on_cross() {
        let store = LayoutStore::from_layout(three_screen_layout());
        let screen = Arc::new(MockScreen::default_stub());
        let shutdown = CancellationToken::new();
        let (handle, coord_task, dispatcher_task) =
            spawn_coordinator(store.handle(), "desk".into(), screen, &shutdown);

        let (outbound_tx, mut outbound_rx) = mpsc::channel::<Message>(32);
        handle
            .register_client("monitor".into(), outbound_tx, vec![])
            .await
            .unwrap();

        handle
            .send_event(CoordinatorEvent::LocalInput(InputEvent::MouseMove {
                x: 960,
                y: 540,
            }))
            .await
            .unwrap();
        handle
            .send_event(CoordinatorEvent::LocalInput(InputEvent::MouseMove {
                x: 2060,
                y: 540,
            }))
            .await
            .unwrap();

        let msg = timeout(Duration::from_secs(1), outbound_rx.recv())
            .await
            .expect("client got something")
            .expect("channel open");
        assert!(matches!(msg, Message::ScreenEnter { .. }), "got {msg:?}");

        shutdown.cancel();
        let _ = timeout(Duration::from_secs(1), coord_task).await;
        let _ = timeout(Duration::from_secs(1), dispatcher_task).await;
    }

    #[tokio::test]
    async fn backpressure_drops_the_slow_client() {
        let store = LayoutStore::from_layout(three_screen_layout());
        let screen = Arc::new(MockScreen::default_stub());
        let shutdown = CancellationToken::new();
        let (handle, coord_task, dispatcher_task) =
            spawn_coordinator(store.handle(), "desk".into(), screen, &shutdown);

        // Drop the receiver so every try_send sees Closed; either
        // Full or Closed is a valid "bad client" signal the driver
        // must tolerate without panicking.
        let (outbound_tx, outbound_rx) = mpsc::channel::<Message>(1);
        drop(outbound_rx);
        handle
            .register_client("monitor".into(), outbound_tx, vec![])
            .await
            .unwrap();

        handle
            .send_event(CoordinatorEvent::LocalInput(InputEvent::MouseMove {
                x: 960,
                y: 540,
            }))
            .await
            .unwrap();
        handle
            .send_event(CoordinatorEvent::LocalInput(InputEvent::MouseMove {
                x: 2060,
                y: 540,
            }))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;

        // After the drop, further events must not panic.
        handle
            .send_event(CoordinatorEvent::LocalInput(InputEvent::MouseMove {
                x: 2100,
                y: 540,
            }))
            .await
            .unwrap();

        shutdown.cancel();
        let _ = timeout(Duration::from_secs(1), coord_task).await;
        let _ = timeout(Duration::from_secs(1), dispatcher_task).await;
    }

    #[tokio::test]
    async fn inject_local_reaches_platform_dispatcher() {
        let store = LayoutStore::from_layout(three_screen_layout());
        let screen = Arc::new(MockScreen::default_stub());
        let shutdown = CancellationToken::new();
        let (handle, coord_task, dispatcher_task) = spawn_coordinator(
            store.handle(),
            "desk".into(),
            Arc::clone(&screen),
            &shutdown,
        );

        let (laptop_tx, _laptop_rx) = mpsc::channel::<Message>(32);
        handle
            .register_client("laptop".into(), laptop_tx, vec![])
            .await
            .unwrap();
        let (monitor_tx, _monitor_rx) = mpsc::channel::<Message>(32);
        handle
            .register_client("monitor".into(), monitor_tx, vec![])
            .await
            .unwrap();

        handle
            .send_event(CoordinatorEvent::PeerMessage {
                from: "laptop".into(),
                msg: Message::ClipboardGrab {
                    id: ClipboardId::Clipboard,
                    seq: 1,
                },
            })
            .await
            .unwrap();
        handle
            .send_event(CoordinatorEvent::PeerMessage {
                from: "laptop".into(),
                msg: Message::ClipboardData {
                    id: ClipboardId::Clipboard,
                    format: ClipboardFormat::Text,
                    data: bytes::Bytes::from_static(b"hello"),
                },
            })
            .await
            .unwrap();

        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        loop {
            if !screen.clipboard_entries().is_empty() {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "platform dispatcher never received ClipboardData"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        shutdown.cancel();
        let _ = timeout(Duration::from_secs(1), coord_task).await;
        let _ = timeout(Duration::from_secs(1), dispatcher_task).await;
    }
}
