//! Dev task runner. Invoke via `cargo xtask <command>`.

use std::process::{Command, ExitStatus};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "xtask", about = "Hop dev task runner")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Run the full local CI pipeline (fmt, clippy, build, test, deny).
    Ci,
    /// Run `cargo fmt --all`.
    Fmt,
    /// Run `cargo clippy --workspace --all-targets -- -D warnings`.
    Lint,
    /// Run `cargo test --workspace` (nextest if available).
    Test,
    /// Run `cargo deny check`.
    Deny,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Ci => ci(),
        Cmd::Fmt => run("cargo", &["fmt", "--all"]),
        Cmd::Lint => run(
            "cargo",
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
        ),
        Cmd::Test => test(),
        Cmd::Deny => run("cargo", &["deny", "check"]),
    }
}

fn ci() -> Result<()> {
    run("cargo", &["fmt", "--all", "--check"])?;
    run(
        "cargo",
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    run("cargo", &["build", "--workspace", "--all-targets"])?;
    test()?;
    // cargo-deny is optional locally; skip with a warning if missing.
    if which("cargo-deny").is_some() {
        run("cargo", &["deny", "check"])?;
    } else {
        eprintln!("xtask: cargo-deny not installed, skipping");
    }
    Ok(())
}

fn test() -> Result<()> {
    if which("cargo-nextest").is_some() {
        run("cargo", &["nextest", "run", "--workspace"])
    } else {
        run("cargo", &["test", "--workspace"])
    }
}

fn run(program: &str, args: &[&str]) -> Result<()> {
    eprintln!("$ {} {}", program, args.join(" "));
    let status: ExitStatus = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("failed to spawn: {program}"))?;
    if !status.success() {
        bail!("{program} {args:?} exited with {status}");
    }
    Ok(())
}

fn which(program: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        [dir.join(program), dir.join(format!("{program}.exe"))]
            .into_iter()
            .find(|candidate| candidate.is_file())
    })
}
