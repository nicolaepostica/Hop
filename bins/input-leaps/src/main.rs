//! `input-leaps` — Input Leap server binary.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand};
use input_leap_config::{
    default_config_path, load_server_settings, ConfigOverrides, ServerSettings,
};
use async_trait::async_trait;
use input_leap_ipc::{
    default_socket_path, protocol::IpcError, IpcHandler, IpcServer, StatusReply,
};
use input_leap_net::{load_or_generate_cert, Fingerprint, FingerprintDb, PeerEntry};
use input_leap_server::ServerConfig;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Input Leap server.
#[derive(Debug, Parser)]
#[command(name = "input-leaps", version, about)]
struct Cli {
    /// Subcommand; if omitted, starts the server.
    #[command(subcommand)]
    cmd: Option<Cmd>,

    #[command(flatten)]
    common: CommonArgs,

    #[command(flatten)]
    server: ServerArgs,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Manage the fingerprint trust database.
    Fingerprint(FingerprintArgs),
}

#[derive(Debug, clap::Args)]
struct FingerprintArgs {
    #[command(subcommand)]
    action: FingerprintAction,
    #[command(flatten)]
    common: CommonArgs,
}

#[derive(Debug, Subcommand)]
enum FingerprintAction {
    /// Add a trusted peer by name and fingerprint.
    Add {
        /// Human-readable peer name.
        name: String,
        /// Peer's certificate fingerprint in `sha256:<hex>` form.
        fingerprint: Fingerprint,
    },
    /// List all trusted peers.
    List,
    /// Remove the peer with the given name.
    Remove {
        /// Human-readable peer name.
        name: String,
    },
    /// Print our own certificate fingerprint.
    Show,
}

#[derive(Debug, Clone, clap::Args)]
struct CommonArgs {
    /// Path to `config.toml`. Defaults to the per-user config dir.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Directory holding `cert.pem` and `key.pem`.
    #[arg(long)]
    cert_dir: Option<PathBuf>,
    /// Path to the fingerprint trust database.
    #[arg(long)]
    fingerprint_db: Option<PathBuf>,
}

#[derive(Debug, clap::Args)]
struct ServerArgs {
    /// Address to bind on. Overrides the file/env setting.
    #[arg(long)]
    listen: Option<SocketAddr>,
    /// Display name advertised to peers. Overrides the file/env setting.
    #[arg(long)]
    name: Option<String>,
    /// Path to the IPC socket for the GUI to connect to.
    ///
    /// Defaults to `<runtime>/input-leap/daemon.sock`. Pass a path or
    /// use `--no-ipc` to disable the IPC server entirely.
    #[arg(long, conflicts_with = "no_ipc")]
    ipc_socket: Option<PathBuf>,
    /// Disable the GUI IPC server.
    #[arg(long)]
    no_ipc: bool,
    /// Run as a Windows NT service (M10 scaffold).
    ///
    /// When set on Windows, the process registers with the Service
    /// Control Manager via `windows-service` and turns into a service
    /// dispatcher. On non-Windows targets the flag is accepted but
    /// ignored with a warning so the same `input-leaps --service`
    /// command line compiles everywhere.
    #[arg(long)]
    service: bool,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    init_tracing();

    let cli = Cli::parse();
    match cli.cmd {
        Some(Cmd::Fingerprint(args)) => run_fingerprint(args),
        None => run_server(cli.common, cli.server).await,
    }
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("input_leap=info,info"));
    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
}

#[cfg(windows)]
fn warn_service_mode() {
    // M10 scaffold. A real NT service entry point uses
    // `windows_service::service_dispatcher::start()` and a
    // ServiceControlHandler; wire that up when a Windows iteration
    // loop is available.
    tracing::warn!(
        "--service is a scaffold: running in foreground. Real NT \
         service dispatch lands alongside the Windows backend when \
         a Windows CI iteration loop is set up."
    );
}

#[cfg(not(windows))]
fn warn_service_mode() {
    tracing::warn!("--service has no effect on non-Windows targets; ignoring");
}

fn config_path(cli_override: Option<&Path>) -> Option<PathBuf> {
    cli_override
        .map(Path::to_path_buf)
        .or_else(default_config_path)
}

fn resolve_settings(common: &CommonArgs, server: &ServerArgs) -> Result<ServerSettings> {
    let path = config_path(common.config.as_deref());
    let overrides = ConfigOverrides {
        address: server.listen,
        display_name: server.name.clone(),
        cert_dir: common.cert_dir.clone(),
        fingerprint_db: common.fingerprint_db.clone(),
    };
    load_server_settings(path.as_deref(), overrides).context("load server settings")
}

async fn run_server(common: CommonArgs, server: ServerArgs) -> Result<()> {
    if server.service {
        warn_service_mode();
    }
    let ipc_socket: Option<PathBuf> = if server.no_ipc {
        None
    } else {
        Some(
            server
                .ipc_socket
                .clone()
                .unwrap_or_else(default_socket_path),
        )
    };
    let settings = resolve_settings(&common, &server)?;
    info!(
        listen = %settings.listen_addr,
        name = %settings.display_name,
        cert_dir = %settings.tls.cert_dir.display(),
        "loaded server settings"
    );

    let identity = load_or_generate_cert(&settings.tls.cert_dir)
        .with_context(|| format!("load cert from {}", settings.tls.cert_dir.display()))?;
    info!(fingerprint = %identity.fingerprint, "local identity loaded");

    let trusted = FingerprintDb::load(&settings.tls.fingerprint_db).with_context(|| {
        format!(
            "load fingerprint DB from {}",
            settings.tls.fingerprint_db.display()
        )
    })?;
    if trusted.is_empty() {
        tracing::warn!(
            "fingerprint DB is empty — no peers will be accepted. \
             Add peers with `input-leaps fingerprint add`."
        );
    }

    let cfg = ServerConfig {
        listen_addr: settings.listen_addr,
        display_name: settings.display_name,
        identity,
        trusted_peers: Arc::new(trusted),
        capabilities: Vec::new(),
    };

    let shutdown = CancellationToken::new();
    let shutdown_trigger = shutdown.clone();
    tokio::spawn(async move {
        if let Ok(()) = tokio::signal::ctrl_c().await {
            info!("SIGINT received");
            shutdown_trigger.cancel();
        }
    });

    // Optional: spawn the GUI IPC server on the daemon socket.
    let ipc_join = if let Some(socket_path) = ipc_socket {
        match IpcServer::bind(&socket_path) {
            Ok(server) => {
                let state = Arc::new(DaemonIpcState::new(
                    cfg.listen_addr.to_string(),
                    cfg.display_name.clone(),
                    cfg.identity.fingerprint.to_string(),
                    settings.tls.fingerprint_db.clone(),
                    &cfg.trusted_peers,
                ));
                let shutdown = shutdown.clone();
                Some(tokio::spawn(async move {
                    if let Err(err) = server.serve(state, shutdown).await {
                        warn!(error = %err, "IPC server terminated with error");
                    }
                }))
            }
            Err(err) => {
                warn!(error = %err, "failed to start IPC server; continuing without it");
                None
            }
        }
    } else {
        None
    };

    let result = backend::run_server(cfg, shutdown.clone()).await;
    shutdown.cancel();
    if let Some(join) = ipc_join {
        let _ = join.await;
    }
    result
}

/// Thread-safe IPC view of the running daemon.
///
/// Exposes read-only status and a mutable trust-store proxy (writes
/// persist to disk but require a restart to take effect on new TLS
/// handshakes; live reload of the verifier is a follow-up).
struct DaemonIpcState {
    listen_addr: String,
    display_name: String,
    local_fingerprint: String,
    fingerprint_db_path: PathBuf,
    edits: Mutex<FingerprintDb>,
}

impl DaemonIpcState {
    fn new(
        listen_addr: String,
        display_name: String,
        local_fingerprint: String,
        fingerprint_db_path: PathBuf,
        initial_snapshot: &Arc<FingerprintDb>,
    ) -> Self {
        let initial = (**initial_snapshot).clone();
        Self {
            listen_addr,
            display_name,
            local_fingerprint,
            fingerprint_db_path,
            edits: Mutex::new(initial),
        }
    }
}

#[async_trait]
impl IpcHandler for DaemonIpcState {
    async fn status(&self) -> StatusReply {
        let count = self.edits.lock().await.len();
        StatusReply {
            listen_addr: self.listen_addr.clone(),
            display_name: self.display_name.clone(),
            local_fingerprint: self.local_fingerprint.clone(),
            trusted_peer_count: count,
        }
    }

    async fn add_peer(
        &self,
        name: String,
        fingerprint: String,
    ) -> Result<bool, (IpcError, String)> {
        let parsed: Fingerprint = fingerprint
            .parse()
            .map_err(|err| (IpcError::InvalidArgument, format!("bad fingerprint: {err}")))?;
        let mut db = self.edits.lock().await;
        let was_new = db.lookup(&parsed).is_none();
        db.add(PeerEntry {
            name,
            fingerprint: parsed,
            added: Utc::now(),
        });
        db.save(&self.fingerprint_db_path).map_err(|err| {
            (
                IpcError::HandlerFailed,
                format!("failed to persist DB: {err}"),
            )
        })?;
        Ok(was_new)
    }

    async fn remove_peer(&self, name: String) -> Result<bool, (IpcError, String)> {
        let mut db = self.edits.lock().await;
        let removed = db.remove(&name);
        db.save(&self.fingerprint_db_path).map_err(|err| {
            (
                IpcError::HandlerFailed,
                format!("failed to persist DB: {err}"),
            )
        })?;
        Ok(removed)
    }
}

fn run_fingerprint(args: FingerprintArgs) -> Result<()> {
    // Fingerprint subcommand bypasses most of the settings pipeline —
    // we only need the cert_dir and DB path.
    let settings = resolve_settings(
        &args.common,
        &ServerArgs {
            listen: None,
            name: None,
            ipc_socket: None,
            no_ipc: true,
            service: false,
        },
    )?;
    match args.action {
        FingerprintAction::Show => {
            let identity = load_or_generate_cert(&settings.tls.cert_dir)
                .with_context(|| format!("load cert from {}", settings.tls.cert_dir.display()))?;
            println!("{}", identity.fingerprint);
            Ok(())
        }
        FingerprintAction::Add { name, fingerprint } => {
            let mut db = FingerprintDb::load(&settings.tls.fingerprint_db)?;
            db.add(PeerEntry {
                name: name.clone(),
                fingerprint,
                added: Utc::now(),
            });
            db.save(&settings.tls.fingerprint_db)?;
            println!("added {name} = {fingerprint}");
            Ok(())
        }
        FingerprintAction::Remove { name } => {
            let mut db = FingerprintDb::load(&settings.tls.fingerprint_db)?;
            let removed = db.remove(&name);
            db.save(&settings.tls.fingerprint_db)?;
            if removed {
                println!("removed {name}");
            } else {
                println!("no entry named {name}");
            }
            Ok(())
        }
        FingerprintAction::List => {
            let db = FingerprintDb::load(&settings.tls.fingerprint_db)?;
            if db.is_empty() {
                println!("(fingerprint DB is empty)");
            } else {
                for entry in db.iter() {
                    println!(
                        "{:<24} {}  (added {})",
                        entry.name,
                        entry.fingerprint,
                        entry.added.format("%Y-%m-%d")
                    );
                }
            }
            Ok(())
        }
    }
}

/// Per-OS backend selection for `input-leaps`.
///
/// Each `#[cfg]` block below is mutually exclusive; at compile time
/// exactly one of the "try X before falling back" blocks is kept, and
/// the final `MockScreen` arm catches any OS that has no native
/// backend yet. Keeping the cascade inside one function (instead of
/// the five `mod backend` blocks it replaced) means adding a new
/// backend only touches one place.
#[allow(clippy::used_underscore_binding, reason = "cfg-gated: _shutdown is used on Linux only")]
mod backend {
    use std::sync::Arc;

    use anyhow::Result;
    use input_leap_platform::MockScreen;
    use input_leap_server::{run, ServerConfig};
    use tokio_util::sync::CancellationToken;
    #[allow(unused_imports, reason = "cfg-gated")]
    use tracing::{debug, info, warn};

    pub async fn run_server(cfg: ServerConfig, shutdown: CancellationToken) -> Result<()> {
        #[cfg(target_os = "linux")]
        match input_leap_platform_ei::EiScreen::try_open() {
            Ok(screen) => {
                info!("using libei platform backend");
                return run(cfg, Arc::new(screen), shutdown)
                    .await
                    .map_err(Into::into);
            }
            Err(err) => debug!(error = %err, "libei backend unavailable; trying X11"),
        }

        #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd"))]
        match input_leap_platform_x11::X11Screen::open(None) {
            Ok(screen) => {
                info!("using X11 platform backend");
                return run(cfg, Arc::new(screen), shutdown)
                    .await
                    .map_err(Into::into);
            }
            Err(err) => warn!(error = %err, "X11 unavailable; falling back to MockScreen"),
        }

        #[cfg(target_os = "macos")]
        match input_leap_platform_macos::MacOsScreen::try_open() {
            Ok(screen) => {
                info!("using macOS platform backend");
                return run(cfg, Arc::new(screen), shutdown)
                    .await
                    .map_err(Into::into);
            }
            Err(err) => warn!(error = %err, "macOS backend unavailable; falling back to MockScreen"),
        }

        #[cfg(windows)]
        match input_leap_platform_windows::WindowsScreen::try_open() {
            Ok(screen) => {
                info!("using Windows platform backend");
                return run(cfg, Arc::new(screen), shutdown)
                    .await
                    .map_err(Into::into);
            }
            Err(err) => {
                warn!(error = %err, "Windows backend unavailable; falling back to MockScreen");
            }
        }

        #[cfg(not(any(
            target_os = "linux",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "macos",
            windows
        )))]
        warn!("no native platform backend on this OS yet; using MockScreen");

        run(cfg, Arc::new(MockScreen::default_stub()), shutdown)
            .await
            .map_err(Into::into)
    }
}
