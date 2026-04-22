//! GUI-side IPC client: connects to a daemon socket, sends requests.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use futures::{SinkExt, StreamExt};
use interprocess::local_socket::tokio::Stream;
use interprocess::local_socket::traits::tokio::Stream as _;
use interprocess::local_socket::{GenericFilePath, ToFsName};
use thiserror::Error;
use tokio_util::codec::Framed;

use crate::codec::{LineJsonCodec, LineJsonError};
use crate::protocol::{
    ErrorPayload, IpcMessage, IpcRequest, RequestPayload, ResponseOutcome, ResultPayload,
    StatusReply,
};

/// Handle for issuing requests against a running daemon.
pub struct IpcClient {
    framed: Framed<Stream, LineJsonCodec>,
    next_id: AtomicU64,
}

/// Errors from the IPC client.
#[derive(Debug, Error)]
pub enum IpcClientError {
    /// Could not open the socket path.
    #[error("connect: {0}")]
    Connect(#[source] std::io::Error),
    /// Underlying codec error (I/O, malformed JSON, oversized frame).
    #[error("codec: {0}")]
    Codec(#[from] LineJsonError),
    /// Connection closed while we were waiting for a reply.
    #[error("daemon hung up")]
    Closed,
    /// Daemon returned a structured error for our request.
    #[error("daemon error {}: {}", .0.code, .0.message)]
    DaemonError(ErrorPayload),
    /// Reply's shape did not match the request we sent.
    #[error("unexpected response shape")]
    UnexpectedResponse,
}

impl IpcClient {
    /// Connect to a daemon socket.
    pub async fn connect(path: &Path) -> Result<Self, IpcClientError> {
        let name = path
            .to_fs_name::<GenericFilePath>()
            .map_err(IpcClientError::Connect)?;
        let stream = Stream::connect(name)
            .await
            .map_err(IpcClientError::Connect)?;
        Ok(Self {
            framed: Framed::new(stream, LineJsonCodec::new()),
            next_id: AtomicU64::new(1),
        })
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    async fn call(&mut self, payload: RequestPayload) -> Result<ResultPayload, IpcClientError> {
        let id = self.next_id();
        let req = IpcRequest { id, payload };
        self.framed.send(IpcMessage::Request(req)).await?;
        loop {
            let msg = self.framed.next().await.ok_or(IpcClientError::Closed)??;
            match msg {
                IpcMessage::Response(resp) if resp.id == id => {
                    return match resp.outcome {
                        ResponseOutcome::Result(p) => Ok(p),
                        ResponseOutcome::Error(e) => Err(IpcClientError::DaemonError(e)),
                    };
                }
                _ => {
                    // Ignore notifications and out-of-order responses.
                }
            }
        }
    }

    /// Call `get_status`.
    pub async fn get_status(&mut self) -> Result<StatusReply, IpcClientError> {
        match self.call(RequestPayload::GetStatus).await? {
            ResultPayload::Status(s) => Ok(s),
            ResultPayload::Ok { .. } => Err(IpcClientError::UnexpectedResponse),
        }
    }

    /// Call `add_peer_fingerprint`.
    pub async fn add_peer_fingerprint(
        &mut self,
        name: &str,
        fingerprint: &str,
    ) -> Result<bool, IpcClientError> {
        match self
            .call(RequestPayload::AddPeerFingerprint {
                name: name.into(),
                fingerprint: fingerprint.into(),
            })
            .await?
        {
            ResultPayload::Ok { ok } => Ok(ok),
            ResultPayload::Status(_) => Err(IpcClientError::UnexpectedResponse),
        }
    }

    /// Call `remove_peer`.
    pub async fn remove_peer(&mut self, name: &str) -> Result<bool, IpcClientError> {
        match self
            .call(RequestPayload::RemovePeer { name: name.into() })
            .await?
        {
            ResultPayload::Ok { ok } => Ok(ok),
            ResultPayload::Status(_) => Err(IpcClientError::UnexpectedResponse),
        }
    }
}
