//! `input-leaps` — Input Leap server binary.
//!
//! At M0 this only prints the version and exits. Real behavior lands in M2+.

use clap::Parser;

/// Input Leap server.
#[derive(Debug, Parser)]
#[command(name = "input-leaps", version, about)]
struct Cli {}

#[allow(
    clippy::unnecessary_wraps,
    reason = "stub; real main returns Result in M2+"
)]
fn main() -> anyhow::Result<()> {
    let _cli = Cli::parse();
    println!("input-leaps {}", env!("CARGO_PKG_VERSION"));
    Ok(())
}
