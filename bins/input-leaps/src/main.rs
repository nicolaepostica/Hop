//! `input-leaps` — Input Leap server binary.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand};
use input_leap_net::{load_or_generate_cert, Fingerprint, FingerprintDb, PeerEntry};
use input_leap_server::ServerConfig;
use tokio_util::sync::CancellationToken;
use tracing::info;

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

#[derive(Debug, clap::Args)]
struct CommonArgs {
    /// Directory holding `cert.pem` and `key.pem`.
    #[arg(long, default_value = "./config/tls")]
    cert_dir: PathBuf,
    /// Path to the fingerprint trust database.
    #[arg(long, default_value = "./config/fingerprints.toml")]
    fingerprint_db: PathBuf,
}

#[derive(Debug, clap::Args)]
struct ServerArgs {
    /// Address to bind on.
    #[arg(long, default_value = "0.0.0.0:24800")]
    listen: SocketAddr,
    /// Display name advertised to peers.
    #[arg(long, default_value = "input-leap-server")]
    name: String,
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
    // Route logs to stderr so the `fingerprint show` subcommand can
    // produce clean, pipe-friendly stdout.
    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
}

async fn run_server(common: CommonArgs, server: ServerArgs) -> Result<()> {
    let identity = load_or_generate_cert(&common.cert_dir)
        .with_context(|| format!("load cert from {}", common.cert_dir.display()))?;
    info!(fingerprint = %identity.fingerprint, "local identity loaded");
    let trusted = FingerprintDb::load(&common.fingerprint_db).with_context(|| {
        format!(
            "load fingerprint DB from {}",
            common.fingerprint_db.display()
        )
    })?;
    if trusted.is_empty() {
        tracing::warn!(
            "fingerprint DB is empty — no peers will be accepted. \
             Add peers with `input-leaps fingerprint add`."
        );
    }

    let cfg = ServerConfig {
        listen_addr: server.listen,
        display_name: server.name,
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

    backend::run_server(cfg, shutdown).await
}

#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd"))]
mod backend {
    use std::sync::Arc;

    use anyhow::Result;
    use input_leap_platform::MockScreen;
    use input_leap_server::{run, ServerConfig};
    use tokio_util::sync::CancellationToken;
    use tracing::{info, warn};

    pub async fn run_server(cfg: ServerConfig, shutdown: CancellationToken) -> Result<()> {
        match input_leap_platform_x11::X11Screen::open(None) {
            Ok(screen) => {
                info!("using X11 platform backend");
                run(cfg, Arc::new(screen), shutdown).await
            }
            Err(err) => {
                warn!(error = %err, "X11 unavailable; falling back to MockScreen");
                run(cfg, Arc::new(MockScreen::default_stub()), shutdown).await
            }
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd")))]
mod backend {
    use std::sync::Arc;

    use anyhow::Result;
    use input_leap_platform::MockScreen;
    use input_leap_server::{run, ServerConfig};
    use tokio_util::sync::CancellationToken;
    use tracing::warn;

    pub async fn run_server(cfg: ServerConfig, shutdown: CancellationToken) -> Result<()> {
        warn!("no native platform backend on this OS yet; using MockScreen");
        run(cfg, Arc::new(MockScreen::default_stub()), shutdown).await
    }
}

fn run_fingerprint(args: FingerprintArgs) -> Result<()> {
    match args.action {
        FingerprintAction::Show => {
            let identity = load_or_generate_cert(&args.common.cert_dir)
                .with_context(|| format!("load cert from {}", args.common.cert_dir.display()))?;
            println!("{}", identity.fingerprint);
            Ok(())
        }
        FingerprintAction::Add { name, fingerprint } => {
            let mut db = FingerprintDb::load(&args.common.fingerprint_db)?;
            db.add(PeerEntry {
                name: name.clone(),
                fingerprint,
                added: Utc::now(),
            });
            db.save(&args.common.fingerprint_db)?;
            println!("added {name} = {fingerprint}");
            Ok(())
        }
        FingerprintAction::Remove { name } => {
            let mut db = FingerprintDb::load(&args.common.fingerprint_db)?;
            let removed = db.remove(&name);
            db.save(&args.common.fingerprint_db)?;
            if removed {
                println!("removed {name}");
            } else {
                println!("no entry named {name}");
            }
            Ok(())
        }
        FingerprintAction::List => {
            let db = FingerprintDb::load(&args.common.fingerprint_db)?;
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
