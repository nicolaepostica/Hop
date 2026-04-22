//! `input-leapc` — Input Leap client binary.
//!
//! At M0 this only prints the version and exits. Real behavior lands in M2+.

use clap::Parser;

/// Input Leap client.
#[derive(Debug, Parser)]
#[command(name = "input-leapc", version, about)]
struct Cli {}

#[allow(
    clippy::unnecessary_wraps,
    reason = "stub; real main returns Result in M2+"
)]
fn main() -> anyhow::Result<()> {
    let _cli = Cli::parse();
    println!("input-leapc {}", env!("CARGO_PKG_VERSION"));
    Ok(())
}
