//! `input-leap-migrate` — one-shot conversion of legacy XML configs to TOML.
//!
//! Stub for M0; real implementation lands alongside M4.

use clap::Parser;

/// Input Leap config migration tool.
#[derive(Debug, Parser)]
#[command(name = "input-leap-migrate", version, about)]
struct Cli {}

#[allow(
    clippy::unnecessary_wraps,
    reason = "stub; real main returns Result in M4+"
)]
fn main() -> anyhow::Result<()> {
    let _cli = Cli::parse();
    println!(
        "input-leap-migrate {} — not implemented yet",
        env!("CARGO_PKG_VERSION")
    );
    Ok(())
}
