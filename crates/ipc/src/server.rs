//! Daemon-side IPC: listens on a local socket, hands each connection
//! off to an async handler.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use interprocess::local_socket::tokio::{Listener, Stream};
use interprocess::local_socket::traits::tokio::Listener as _;
use interprocess::local_socket::{GenericFilePath, ListenerOptions, ToFsName};
use thiserror::Error;
use tokio::task::JoinHandle;
use tokio_util::codec::Framed;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::codec::{LineJsonCodec, LineJsonError};
use crate::protocol::{
    ErrorPayload, IpcError, IpcMessage, IpcRequest, IpcResponse, JsonRpcVersion, RequestPayload,
    ResponseOutcome, ResultPayload, StatusReply,
};

/// Default location for the daemon socket.
///
/// Linux follows `$XDG_RUNTIME_DIR` convention; macOS and everything
/// else fall back to the system tempdir.
#[must_use]
pub fn default_socket_path() -> PathBuf {
    if cfg!(target_os = "linux") {
        if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
            return PathBuf::from(runtime)
                .join("hop")
                .join("daemon.sock");
        }
    }
    std::env::temp_dir().join("hop").join("daemon.sock")
}

/// Errors from the IPC server's own lifecycle (not per-connection).
#[derive(Debug, Error)]
pub enum IpcServerError {
    /// Failed to bind the socket.
    #[error("bind {path}: {source}")]
    Bind {
        /// Path we tried to bind.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// Accept failed in a way we can't recover from.
    #[error("accept failed: {0}")]
    Accept(#[source] std::io::Error),
}

/// User-supplied callback that implements the actual daemon actions.
///
/// Implementations write plain `async fn` — the `#[async_trait]` macro
/// takes care of the boxing that `dyn IpcHandler` requires at runtime.
#[async_trait]
pub trait IpcHandler: Send + Sync + 'static {
    /// Return the current daemon state.
    async fn status(&self) -> StatusReply;

    /// Add a peer fingerprint. Returns `Ok(true)` if the DB gained a
    /// new entry, `Ok(false)` if it replaced an existing one. Returns
    /// an [`IpcError`] + human message on failure.
    async fn add_peer(
        &self,
        name: String,
        fingerprint: String,
    ) -> Result<bool, (IpcError, String)>;

    /// Remove a peer by name. Returns `true` when an entry was found.
    async fn remove_peer(&self, name: String) -> Result<bool, (IpcError, String)>;
}

/// Listens on a local socket, dispatches requests to an `IpcHandler`.
pub struct IpcServer {
    listener: Listener,
    path: PathBuf,
}

impl IpcServer {
    /// Bind on `path`, removing any stale socket file first.
    pub fn bind(path: &Path) -> Result<Self, IpcServerError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|source| IpcServerError::Bind {
                    path: path.to_path_buf(),
                    source,
                })?;
            }
        }
        // Clean up a stale socket from a previous crashed run.
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(path);
        }

        let name = path
            .to_fs_name::<GenericFilePath>()
            .map_err(|source| IpcServerError::Bind {
                path: path.to_path_buf(),
                source,
            })?;
        let listener = ListenerOptions::new()
            .name(name)
            .create_tokio()
            .map_err(|source| IpcServerError::Bind {
                path: path.to_path_buf(),
                source,
            })?;

        #[cfg(unix)]
        apply_socket_perms(path);

        Ok(Self {
            listener,
            path: path.to_path_buf(),
        })
    }

    /// Socket path (useful for logs / docs).
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Run the accept loop until `shutdown` fires. Spawns one task per
    /// connection.
    pub async fn serve<H>(
        self,
        handler: Arc<H>,
        shutdown: CancellationToken,
    ) -> Result<(), IpcServerError>
    where
        H: IpcHandler,
    {
        info!(path = %self.path.display(), "IPC server listening");
        let mut tasks: Vec<JoinHandle<()>> = Vec::new();

        loop {
            tokio::select! {
                biased;

                () = shutdown.cancelled() => break,

                accept = self.listener.accept() => {
                    match accept {
                        Ok(stream) => {
                            let handler = Arc::clone(&handler);
                            let shutdown = shutdown.clone();
                            tasks.push(tokio::spawn(async move {
                                if let Err(err) = serve_conn(stream, handler, shutdown).await {
                                    warn!(error = %err, "IPC connection ended with error");
                                }
                            }));
                        }
                        Err(err) => {
                            warn!(error = %err, "IPC accept failed");
                        }
                    }
                }
            }
        }

        // Let in-flight connections drain.
        for task in tasks {
            let _ = task.await;
        }
        // Best-effort cleanup of the socket file.
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(&self.path);
        }
        Ok(())
    }
}

#[cfg(unix)]
fn apply_socket_perms(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(metadata) = std::fs::metadata(path) {
        let mut perms = metadata.permissions();
        perms.set_mode(0o600);
        let _ = std::fs::set_permissions(path, perms);
    }
}

async fn serve_conn<H>(
    stream: Stream,
    handler: Arc<H>,
    shutdown: CancellationToken,
) -> Result<(), LineJsonError>
where
    H: IpcHandler,
{
    let mut framed = Framed::new(stream, LineJsonCodec::new());

    loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => return Ok(()),
            msg = framed.next() => {
                let Some(msg) = msg else { return Ok(()); };
                let msg = msg?;
                if let IpcMessage::Request(req) = msg {
                    let response = handle_request(handler.as_ref(), req).await;
                    framed.send(IpcMessage::Response(response)).await?;
                } else {
                    debug!("IPC client sent a response/notify; ignoring");
                }
            }
        }
    }
}

async fn handle_request<H>(handler: &H, req: IpcRequest) -> IpcResponse
where
    H: IpcHandler,
{
    let id = req.id;
    let outcome = match req.payload {
        RequestPayload::GetStatus => {
            let status = handler.status().await;
            ResponseOutcome::Result(ResultPayload::Status(status))
        }
        RequestPayload::AddPeerFingerprint { name, fingerprint } => {
            match handler.add_peer(name, fingerprint).await {
                Ok(ok) => ResponseOutcome::Result(ResultPayload::Ok { ok }),
                Err((code, message)) => ResponseOutcome::Error(ErrorPayload {
                    code: code.code(),
                    message,
                    data: None,
                }),
            }
        }
        RequestPayload::RemovePeer { name } => match handler.remove_peer(name).await {
            Ok(ok) => ResponseOutcome::Result(ResultPayload::Ok { ok }),
            Err((code, message)) => ResponseOutcome::Error(ErrorPayload {
                code: code.code(),
                message,
                data: None,
            }),
        },
    };
    IpcResponse {
        jsonrpc: JsonRpcVersion,
        id,
        outcome,
    }
}
