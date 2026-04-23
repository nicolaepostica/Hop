//! `input-leapc` — Input Leap client binary.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand};
use input_leap_client::ClientConfig;
use input_leap_config::{
    default_config_path, load_client_settings, ClientSettings, ConfigOverrides,
};
use input_leap_net::{load_or_generate_cert, Fingerprint, FingerprintDb, PeerEntry};
use tokio_util::sync::CancellationToken;
use tracing::info;

/// Input Leap client.
#[derive(Debug, Parser)]
#[command(name = "input-leapc", version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,

    #[command(flatten)]
    common: CommonArgs,

    #[command(flatten)]
    client: ClientArgs,
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
        /// Human-readable peer name (usually the server's).
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
struct ClientArgs {
    /// Server address to connect to. Overrides the file/env setting.
    #[arg(long)]
    connect: Option<SocketAddr>,
    /// Display name advertised to the server. Overrides the file/env setting.
    #[arg(long)]
    name: Option<String>,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    init_tracing();

    let cli = Cli::parse();
    match cli.cmd {
        Some(Cmd::Fingerprint(args)) => run_fingerprint(args),
        None => run_client(cli.common, cli.client).await,
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

fn config_path(cli_override: Option<&Path>) -> Option<PathBuf> {
    cli_override
        .map(Path::to_path_buf)
        .or_else(default_config_path)
}

fn resolve_settings(common: &CommonArgs, client: &ClientArgs) -> Result<ClientSettings> {
    let path = config_path(common.config.as_deref());
    let overrides = ConfigOverrides {
        address: client.connect,
        display_name: client.name.clone(),
        cert_dir: common.cert_dir.clone(),
        fingerprint_db: common.fingerprint_db.clone(),
    };
    load_client_settings(path.as_deref(), overrides).context("load client settings")
}

async fn run_client(common: CommonArgs, client: ClientArgs) -> Result<()> {
    let settings = resolve_settings(&common, &client)?;
    info!(
        server = %settings.server_addr,
        name = %settings.display_name,
        cert_dir = %settings.tls.cert_dir.display(),
        "loaded client settings"
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
            "fingerprint DB is empty — cannot verify the server. \
             Add the server's fingerprint with `input-leapc fingerprint add`."
        );
    }

    let cfg = ClientConfig {
        server_addr: settings.server_addr,
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

    backend::run_client(cfg, shutdown).await
}

fn run_fingerprint(args: FingerprintArgs) -> Result<()> {
    let settings = resolve_settings(
        &args.common,
        &ClientArgs {
            connect: None,
            name: None,
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

#[cfg(target_os = "linux")]
mod backend {
    use std::sync::Arc;

    use anyhow::Result;
    use input_leap_client::{run, ClientConfig};
    use input_leap_platform::MockScreen;
    use tokio_util::sync::CancellationToken;
    use tracing::{debug, info, warn};

    pub async fn run_client(cfg: ClientConfig, shutdown: CancellationToken) -> Result<()> {
        match input_leap_platform_ei::EiScreen::try_open() {
            Ok(screen) => {
                info!("using libei platform backend");
                return run(cfg, Arc::new(screen), shutdown).await.map_err(Into::into);
            }
            Err(err) => debug!(error = %err, "libei backend unavailable; trying X11"),
        }
        match input_leap_platform_x11::X11Screen::open(None) {
            Ok(screen) => {
                info!("using X11 platform backend");
                run(cfg, Arc::new(screen), shutdown).await.map_err(Into::into)
            }
            Err(err) => {
                warn!(error = %err, "X11 unavailable; falling back to MockScreen");
                run(cfg, Arc::new(MockScreen::default_stub()), shutdown).await.map_err(Into::into)
            }
        }
    }
}

#[cfg(any(target_os = "freebsd", target_os = "openbsd"))]
mod backend {
    use std::sync::Arc;

    use anyhow::Result;
    use input_leap_client::{run, ClientConfig};
    use input_leap_platform::MockScreen;
    use tokio_util::sync::CancellationToken;
    use tracing::{info, warn};

    pub async fn run_client(cfg: ClientConfig, shutdown: CancellationToken) -> Result<()> {
        match input_leap_platform_x11::X11Screen::open(None) {
            Ok(screen) => {
                info!("using X11 platform backend");
                run(cfg, Arc::new(screen), shutdown).await.map_err(Into::into)
            }
            Err(err) => {
                warn!(error = %err, "X11 unavailable; falling back to MockScreen");
                run(cfg, Arc::new(MockScreen::default_stub()), shutdown).await.map_err(Into::into)
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod backend {
    use std::sync::Arc;

    use anyhow::Result;
    use input_leap_client::{run, ClientConfig};
    use input_leap_platform::MockScreen;
    use tokio_util::sync::CancellationToken;
    use tracing::{info, warn};

    pub async fn run_client(cfg: ClientConfig, shutdown: CancellationToken) -> Result<()> {
        match input_leap_platform_macos::MacOsScreen::try_open() {
            Ok(screen) => {
                info!("using macOS platform backend");
                run(cfg, Arc::new(screen), shutdown).await.map_err(Into::into)
            }
            Err(err) => {
                warn!(error = %err, "macOS backend unavailable; falling back to MockScreen");
                run(cfg, Arc::new(MockScreen::default_stub()), shutdown).await.map_err(Into::into)
            }
        }
    }
}

#[cfg(windows)]
mod backend {
    use std::sync::Arc;

    use anyhow::Result;
    use input_leap_client::{run, ClientConfig};
    use input_leap_platform::MockScreen;
    use tokio_util::sync::CancellationToken;
    use tracing::{info, warn};

    pub async fn run_client(cfg: ClientConfig, shutdown: CancellationToken) -> Result<()> {
        match input_leap_platform_windows::WindowsScreen::try_open() {
            Ok(screen) => {
                info!("using Windows platform backend");
                run(cfg, Arc::new(screen), shutdown).await.map_err(Into::into)
            }
            Err(err) => {
                warn!(error = %err, "Windows backend unavailable; falling back to MockScreen");
                run(cfg, Arc::new(MockScreen::default_stub()), shutdown).await.map_err(Into::into)
            }
        }
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "macos",
    windows
)))]
mod backend {
    use std::sync::Arc;

    use anyhow::Result;
    use input_leap_client::{run, ClientConfig};
    use input_leap_platform::MockScreen;
    use tokio_util::sync::CancellationToken;
    use tracing::warn;

    pub async fn run_client(cfg: ClientConfig, shutdown: CancellationToken) -> Result<()> {
        warn!("no native platform backend on this OS yet; using MockScreen");
        run(cfg, Arc::new(MockScreen::default_stub()), shutdown).await.map_err(Into::into)
    }
}
