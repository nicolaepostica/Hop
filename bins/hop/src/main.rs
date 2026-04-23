//! Hop — desktop UI for Hop.
//!
//! Thin launcher around [`hop_ui::run`]: initialise tracing, open the
//! native window, surface errors.

use anyhow::Result;

fn main() -> Result<()> {
    init_tracing();
    hop_ui::run().map_err(|err| anyhow::anyhow!("eframe: {err}"))
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("hop=info,hop_ui=info,info"));
    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
}
