//! Local TLS identity bootstrap for Hop.
//!
//! The UI needs the local certificate fingerprint on day one (for the
//! Server view's "Your fingerprint" card and the Client view's "My
//! fingerprint" card). We locate a per-user data directory through
//! `directories::ProjectDirs`, create it if missing, and delegate the
//! actual cert generation/loading to `hop_net::load_or_generate_cert`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::ProjectDirs;
use hop_net::{load_or_generate_cert, LoadedIdentity};

const QUALIFIER: &str = "com";
const ORGANIZATION: &str = "Hop";
const APPLICATION: &str = "hop";

/// Return the directory Hop uses for TLS material.
///
/// On Linux this is typically `~/.local/share/hop/tls`; on
/// macOS and Windows the `directories` crate resolves to the
/// platform's conventional data location.
#[must_use]
pub fn cert_dir() -> PathBuf {
    let base = ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
        .map_or_else(|| std::env::temp_dir().join("hop"), |p| p.data_dir().to_path_buf());
    base.join("tls")
}

/// Load (or, on first run, generate + save) this machine's identity.
///
/// # Errors
/// Fails if the cert directory cannot be created, or if the underlying
/// key generation / PEM write fails.
pub fn load_or_create() -> Result<LoadedIdentity> {
    let dir = cert_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create cert dir {}", dir.display()))?;
    load_or_generate_cert(&dir)
        .with_context(|| format!("load or generate cert in {}", dir.display()))
}
