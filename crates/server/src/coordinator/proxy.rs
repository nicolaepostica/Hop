//! Per-peer I/O adapter between a framed TLS stream and the
//! coordinator task.
//!
//! The [`ClientProxy`] runs one task per connected peer. It:
//!
//!  - reads inbound `Message`s off the framed stream, filters out
//!    handshake-level traffic (`KeepAlive`, `Disconnect`), and forwards
//!    the rest into the coordinator via an mpsc channel;
//!  - drains outbound `Message`s from a dedicated mpsc channel the
//!    coordinator writes into, pushing them out to the peer;
//!  - runs keep-alive bookkeeping (emit our own every
//!    [`KEEPALIVE_INTERVAL`](input_leap_net::KEEPALIVE_INTERVAL), time
//!    out after [`KEEPALIVE_TIMEOUT`](input_leap_net::KEEPALIVE_TIMEOUT));
//!  - always ends by sending a
//!    [`CoordinatorEvent::ClientDisconnected`] so the coordinator can
//!    garbage-collect its routing tables.
//!
//! Generic over the underlying AsyncRead/AsyncWrite so unit tests can
//! drive it with `tokio::io::duplex` instead of a real TLS socket.

use futures::{SinkExt, StreamExt};
use input_leap_net::KeepAliveTracker;
use input_leap_protocol::{framed, DisconnectReason, Message, MessageCodec, ProtocolError};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::select;
use tokio::sync::mpsc;
use tokio_util::codec::Framed;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::coordinator::layout::ScreenName;
use crate::coordinator::state::CoordinatorEvent;
use crate::coordinator::task::CoordinatorCommand;

/// Shape of the `Framed` stream a [`ClientProxy`] operates on.
pub type ProxyStream<T> = Framed<T, MessageCodec>;

/// Errors a [`ClientProxy`] can terminate with.
#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    /// Protocol-framing error on the underlying stream.
    #[error("protocol: {0}")]
    Protocol(#[from] ProtocolError),
    /// The coordinator task is gone (its inbound channel was closed).
    #[error("coordinator task no longer accepting events")]
    CoordinatorGone,
}

/// Per-peer adapter bridging a [`Framed`] stream and the coordinator.
pub struct ClientProxy<T> {
    name: ScreenName,
    framed: ProxyStream<T>,
    outbound_rx: mpsc::Receiver<Message>,
    commands_tx: mpsc::Sender<CoordinatorCommand>,
    shutdown: CancellationToken,
}

impl<T> ClientProxy<T>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    /// Build a proxy around an already-handshaken, framed stream.
    #[must_use]
    pub fn new(
        name: ScreenName,
        framed: ProxyStream<T>,
        outbound_rx: mpsc::Receiver<Message>,
        commands_tx: mpsc::Sender<CoordinatorCommand>,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            name,
            framed,
            outbound_rx,
            commands_tx,
            shutdown,
        }
    }

    /// Convenience: build a proxy from an unframed AsyncRead/AsyncWrite.
    pub fn from_io(
        name: ScreenName,
        io: T,
        outbound_rx: mpsc::Receiver<Message>,
        commands_tx: mpsc::Sender<CoordinatorCommand>,
        shutdown: CancellationToken,
    ) -> Self {
        Self::new(name, framed(io), outbound_rx, commands_tx, shutdown)
    }

    /// Drive the proxy until the peer goes away, the shutdown token
    /// fires, or the outbound channel is closed.
    ///
    /// Always emits a final
    /// [`CoordinatorEvent::ClientDisconnected`] to the coordinator
    /// before returning, so the coordinator can prune its routing
    /// tables regardless of why we exited.
    pub async fn run(mut self) -> Result<(), ProxyError> {
        let result = self.run_inner().await;

        // Best-effort: coordinator may already be gone during shutdown,
        // in which case the send fails silently.
        let _ = self
            .commands_tx
            .send(CoordinatorCommand::Event(
                CoordinatorEvent::ClientDisconnected {
                    name: self.name.clone(),
                },
            ))
            .await;

        result
    }

    async fn run_inner(&mut self) -> Result<(), ProxyError> {
        let mut keepalive = KeepAliveTracker::new();

        loop {
            select! {
                biased;

                () = self.shutdown.cancelled() => {
                    let _ = self
                        .framed
                        .send(Message::Disconnect {
                            reason: DisconnectReason::UserInitiated,
                        })
                        .await;
                    return Ok(());
                }

                outbound = self.outbound_rx.recv() => {
                    let Some(msg) = outbound else {
                        // Coordinator closed our outbound channel —
                        // typically because it dropped us (e.g.
                        // backpressure). Tell the peer cleanly.
                        let _ = self
                            .framed
                            .send(Message::Disconnect {
                                reason: DisconnectReason::InternalError,
                            })
                            .await;
                        return Ok(());
                    };
                    self.framed.send(msg).await?;
                }

                incoming = self.framed.next() => {
                    match incoming {
                        Some(Ok(msg)) => {
                            keepalive.mark_seen();
                            match msg {
                                Message::KeepAlive => {
                                    debug!(peer = %self.name, "keep-alive from peer");
                                }
                                Message::Disconnect { reason } => {
                                    debug!(
                                        peer = %self.name,
                                        ?reason,
                                        "peer sent Disconnect"
                                    );
                                    return Ok(());
                                }
                                other => {
                                    self.commands_tx
                                        .send(CoordinatorCommand::Event(
                                            CoordinatorEvent::PeerMessage {
                                                from: self.name.clone(),
                                                msg: other,
                                            },
                                        ))
                                        .await
                                        .map_err(|_| ProxyError::CoordinatorGone)?;
                                }
                            }
                        }
                        Some(Err(err)) => return Err(err.into()),
                        None => {
                            debug!(peer = %self.name, "peer closed stream");
                            return Ok(());
                        }
                    }
                }

                _ = keepalive.tick() => {
                    if keepalive.is_timed_out() {
                        warn!(peer = %self.name, "peer keep-alive timeout");
                        let _ = self
                            .framed
                            .send(Message::Disconnect {
                                reason: DisconnectReason::KeepAliveTimeout,
                            })
                            .await;
                        return Ok(());
                    }
                    self.framed.send(Message::KeepAlive).await?;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use input_leap_common::{ClipboardId, KeyId, ModifierMask};
    use tokio::io::duplex;
    use tokio::time::{timeout, Duration};

    type ProxyHarness = (
        tokio::task::JoinHandle<Result<(), ProxyError>>,
        ProxyStream<tokio::io::DuplexStream>,
        mpsc::Sender<Message>,
        mpsc::Receiver<CoordinatorCommand>,
        CancellationToken,
    );

    fn spawn_proxy() -> ProxyHarness {
        let (server_io, peer_io) = duplex(4096);
        let (outbound_tx, outbound_rx) = mpsc::channel::<Message>(16);
        let (commands_tx, commands_rx) = mpsc::channel::<CoordinatorCommand>(16);
        let shutdown = CancellationToken::new();

        let proxy = ClientProxy::from_io(
            "laptop".into(),
            server_io,
            outbound_rx,
            commands_tx,
            shutdown.clone(),
        );
        let handle = tokio::spawn(async move { proxy.run().await });

        (handle, framed(peer_io), outbound_tx, commands_rx, shutdown)
    }

    fn expect_peer_message(cmd: CoordinatorCommand) -> (ScreenName, Message) {
        match cmd {
            CoordinatorCommand::Event(CoordinatorEvent::PeerMessage { from, msg }) => (from, msg),
            other => panic!("expected PeerMessage event, got {other:?}"),
        }
    }

    fn expect_client_disconnected(cmd: CoordinatorCommand) -> ScreenName {
        match cmd {
            CoordinatorCommand::Event(CoordinatorEvent::ClientDisconnected { name }) => name,
            other => panic!("expected ClientDisconnected event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn inbound_peer_message_forwarded_to_coordinator() {
        let (handle, mut peer, _outbound, mut inbound, shutdown) = spawn_proxy();

        peer.send(Message::ClipboardGrab {
            id: ClipboardId::Clipboard,
            seq: 1,
        })
        .await
        .unwrap();

        let cmd = timeout(Duration::from_secs(1), inbound.recv())
            .await
            .expect("coordinator received something")
            .expect("channel still open");
        let (from, msg) = expect_peer_message(cmd);
        assert_eq!(from, "laptop");
        assert!(matches!(msg, Message::ClipboardGrab { .. }));

        shutdown.cancel();
        let _ = timeout(Duration::from_secs(1), peer.next()).await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn outbound_message_is_written_to_peer() {
        let (handle, mut peer, outbound, _inbound, shutdown) = spawn_proxy();

        outbound
            .send(Message::KeyDown {
                key: KeyId::new(0x61),
                mods: ModifierMask::empty(),
            })
            .await
            .unwrap();

        let received = timeout(Duration::from_secs(1), peer.next())
            .await
            .expect("peer saw something")
            .expect("stream alive")
            .expect("decode ok");
        assert!(matches!(received, Message::KeyDown { .. }));

        shutdown.cancel();
        let _ = timeout(Duration::from_secs(1), peer.next()).await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn keepalive_is_not_forwarded_to_coordinator() {
        let (handle, mut peer, _outbound, mut inbound, shutdown) = spawn_proxy();

        peer.send(Message::KeepAlive).await.unwrap();

        let nothing = timeout(Duration::from_millis(100), inbound.recv()).await;
        assert!(nothing.is_err(), "KeepAlive must not reach coordinator");

        shutdown.cancel();
        let _ = timeout(Duration::from_secs(1), peer.next()).await;
        handle.await.unwrap().unwrap();

        let cmd = timeout(Duration::from_secs(1), inbound.recv())
            .await
            .expect("disconnect arrived")
            .expect("channel still open");
        assert_eq!(expect_client_disconnected(cmd), "laptop");
    }

    #[tokio::test]
    async fn peer_disconnect_ends_the_proxy_cleanly() {
        let (handle, mut peer, _outbound, mut inbound, _shutdown) = spawn_proxy();

        peer.send(Message::Disconnect {
            reason: DisconnectReason::UserInitiated,
        })
        .await
        .unwrap();

        let res = timeout(Duration::from_secs(1), handle).await.unwrap();
        res.unwrap().unwrap();

        let cmd = timeout(Duration::from_secs(1), inbound.recv())
            .await
            .expect("coordinator heard about it")
            .expect("channel open");
        assert_eq!(expect_client_disconnected(cmd), "laptop");
    }

    #[tokio::test]
    async fn coordinator_closing_outbound_channel_makes_proxy_exit() {
        let (handle, mut peer, outbound, mut inbound, _shutdown) = spawn_proxy();

        drop(outbound);

        let received = timeout(Duration::from_secs(1), peer.next())
            .await
            .expect("peer saw something")
            .expect("stream alive")
            .expect("decode ok");
        assert!(matches!(received, Message::Disconnect { .. }));

        let res = timeout(Duration::from_secs(1), handle).await.unwrap();
        res.unwrap().unwrap();

        let cmd = timeout(Duration::from_secs(1), inbound.recv())
            .await
            .expect("coordinator heard about it")
            .expect("channel open");
        assert_eq!(expect_client_disconnected(cmd), "laptop");
    }
}
