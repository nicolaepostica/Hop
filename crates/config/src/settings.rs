//! Strongly-typed settings + layered loader.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use figment::providers::{Env, Format, Serialized, Toml};
use figment::Figment;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors produced while loading settings.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// A figment provider (file/env/CLI) failed to parse or merge.
    #[error("config load failed: {0}")]
    Figment(String),

    /// Reading the config file itself failed (permission denied, ...).
    #[error("cannot read config file {path}: {source}")]
    ReadFile {
        /// Path we tried to read.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

impl From<figment::Error> for ConfigError {
    fn from(err: figment::Error) -> Self {
        Self::Figment(err.to_string())
    }
}

/// Settings for the server binary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerSettings {
    /// TCP address to bind the mTLS listener on.
    #[serde(default = "default_listen")]
    pub listen_addr: SocketAddr,
    /// Human-readable name advertised in the handshake.
    #[serde(default = "default_server_name")]
    pub display_name: String,
    /// TLS identity + trust store locations.
    #[serde(default)]
    pub tls: TlsSettings,
    /// File-transfer behaviour (used by M9).
    #[serde(default)]
    pub file_transfer: FileTransferSettings,
    /// Screen-layout file (used by M11).
    #[serde(default)]
    pub layout: LayoutSettings,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            listen_addr: default_listen(),
            display_name: default_server_name(),
            tls: TlsSettings::default(),
            file_transfer: FileTransferSettings::default(),
            layout: LayoutSettings::default(),
        }
    }
}

/// Settings for the client binary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClientSettings {
    /// Server address to connect to.
    #[serde(default = "default_connect")]
    pub server_addr: SocketAddr,
    /// Human-readable name advertised in the handshake.
    #[serde(default = "default_client_name")]
    pub display_name: String,
    /// TLS identity + trust store locations.
    #[serde(default)]
    pub tls: TlsSettings,
    /// File-transfer behaviour (used by M9).
    #[serde(default)]
    pub file_transfer: FileTransferSettings,
}

impl Default for ClientSettings {
    fn default() -> Self {
        Self {
            server_addr: default_connect(),
            display_name: default_client_name(),
            tls: TlsSettings::default(),
            file_transfer: FileTransferSettings::default(),
        }
    }
}

/// Where the TLS material lives.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TlsSettings {
    /// Directory holding `cert.pem` and `key.pem`.
    #[serde(default = "default_cert_dir")]
    pub cert_dir: PathBuf,
    /// Path to the fingerprint trust DB.
    #[serde(default = "default_fingerprint_db")]
    pub fingerprint_db: PathBuf,
}

impl Default for TlsSettings {
    fn default() -> Self {
        Self {
            cert_dir: default_cert_dir(),
            fingerprint_db: default_fingerprint_db(),
        }
    }
}

/// Where the screen-layout file lives on disk.
///
/// `path` is optional: `None` means "use
/// [`crate::paths::default_layout_path`] when the loader runs". The
/// loader tolerates a missing file — it starts with a single-primary
/// layout and logs a warning.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LayoutSettings {
    /// Explicit path to `layout.toml`. `None` → use the default
    /// `<project_config_dir>/layout.toml`.
    #[serde(default)]
    pub path: Option<PathBuf>,
}

/// File-clipboard / drop-directory settings (M9).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileTransferSettings {
    /// Whether to accept file transfers at all.
    #[serde(default = "truthy")]
    pub enabled: bool,
    /// Where received files land.
    #[serde(default = "default_drop_dir")]
    pub drop_directory: PathBuf,
    /// Reject transfers larger than this (bytes).
    #[serde(default = "default_max_transfer")]
    pub max_transfer_bytes: u64,
    /// Size of each `FileChunk` in bytes.
    #[serde(default = "default_chunk_bytes")]
    pub chunk_bytes: u32,
}

impl Default for FileTransferSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            drop_directory: default_drop_dir(),
            max_transfer_bytes: default_max_transfer(),
            chunk_bytes: default_chunk_bytes(),
        }
    }
}

/// CLI-driven overrides applied after file + env.
///
/// Each field is optional: `None` means "don't override, keep whatever
/// the file or env said".
#[derive(Debug, Clone, Default, Serialize)]
pub struct ConfigOverrides {
    /// Override `listen_addr` / `server_addr` (binary-specific).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<SocketAddr>,
    /// Override `display_name`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Override `tls.cert_dir`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cert_dir: Option<PathBuf>,
    /// Override `tls.fingerprint_db`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint_db: Option<PathBuf>,
    /// Override `layout.path` (server only; ignored for client settings).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout_path: Option<PathBuf>,
}

// Defaults -----------------------------------------------------------------

fn default_listen() -> SocketAddr {
    "0.0.0.0:25900".parse().expect("literal socket addr")
}

fn default_connect() -> SocketAddr {
    "127.0.0.1:25900".parse().expect("literal socket addr")
}

fn default_server_name() -> String {
    "hop-server".into()
}

fn default_client_name() -> String {
    "hop-client".into()
}

fn default_cert_dir() -> PathBuf {
    PathBuf::from("./config/tls")
}

fn default_fingerprint_db() -> PathBuf {
    PathBuf::from("./config/fingerprints.toml")
}

fn default_drop_dir() -> PathBuf {
    crate::paths::default_drop_directory()
}

fn default_max_transfer() -> u64 {
    2 * 1024 * 1024 * 1024
}

fn default_chunk_bytes() -> u32 {
    64 * 1024
}

fn truthy() -> bool {
    true
}

// Loaders ------------------------------------------------------------------

/// Load [`ServerSettings`] from file + env + CLI. The file is optional.
#[allow(
    clippy::needless_pass_by_value,
    reason = "callers build overrides and hand them off"
)]
pub fn load_server_settings(
    file: Option<&Path>,
    overrides: ConfigOverrides,
) -> Result<ServerSettings, ConfigError> {
    load_layered(file, &overrides, "server")
}

/// Load [`ClientSettings`] from file + env + CLI.
#[allow(
    clippy::needless_pass_by_value,
    reason = "callers build overrides and hand them off"
)]
pub fn load_client_settings(
    file: Option<&Path>,
    overrides: ConfigOverrides,
) -> Result<ClientSettings, ConfigError> {
    load_layered(file, &overrides, "client")
}

fn load_layered<T>(
    file: Option<&Path>,
    overrides: &ConfigOverrides,
    kind: &str,
) -> Result<T, ConfigError>
where
    T: Default + for<'de> Deserialize<'de> + Serialize,
{
    let mut fig = Figment::from(Serialized::defaults(T::default()));

    if let Some(path) = file {
        if path.exists() {
            fig = fig.merge(Toml::file(path));
        }
        // Missing file is fine — the defaults layer covers it.
    }

    // Layer environment variables: HOP_<NAME> (no role segmentation
    // at M4; we can introduce HOP_SERVER_* / _CLIENT_* later if
    // the two binaries need to diverge).
    fig = fig.merge(Env::prefixed("HOP_").split("__"));

    // CLI overrides map into the relevant fields.
    let cli = cli_merge_map::<T>(overrides, kind);
    fig = fig.merge(Serialized::defaults(cli));

    let out: T = fig.extract()?;
    Ok(out)
}

/// Build a `serde_json::Value` that only contains the fields the CLI
/// is explicitly overriding. figment will merge this on top of file +
/// env, leaving everything else alone.
fn cli_merge_map<T>(overrides: &ConfigOverrides, kind: &str) -> serde_json::Value {
    let _ = std::marker::PhantomData::<T>;
    let mut map = serde_json::Map::new();
    if let Some(addr) = overrides.address {
        let key = if kind == "server" {
            "listen_addr"
        } else {
            "server_addr"
        };
        map.insert(key.into(), serde_json::json!(addr.to_string()));
    }
    if let Some(name) = &overrides.display_name {
        map.insert("display_name".into(), serde_json::json!(name));
    }
    let mut tls = serde_json::Map::new();
    if let Some(dir) = &overrides.cert_dir {
        tls.insert("cert_dir".into(), serde_json::json!(dir));
    }
    if let Some(db) = &overrides.fingerprint_db {
        tls.insert("fingerprint_db".into(), serde_json::json!(db));
    }
    if !tls.is_empty() {
        map.insert("tls".into(), serde_json::Value::Object(tls));
    }
    if kind == "server" {
        if let Some(path) = &overrides.layout_path {
            let mut layout = serde_json::Map::new();
            layout.insert("path".into(), serde_json::json!(path));
            map.insert("layout".into(), serde_json::Value::Object(layout));
        }
    }
    serde_json::Value::Object(map)
}

#[cfg(test)]
mod tests {
    use serial_test::serial;

    use super::*;

    #[test]
    #[serial(env)]
    fn defaults_produce_sane_server_settings() {
        let s = ServerSettings::default();
        assert_eq!(s.listen_addr.port(), 25900);
        assert_eq!(s.display_name, "hop-server");
        assert!(s.file_transfer.enabled);
    }

    #[test]
    #[serial(env)]
    fn toml_round_trip_preserves_fields() {
        let s = ServerSettings {
            display_name: "laptop".into(),
            tls: TlsSettings {
                cert_dir: "/etc/inputleap/tls".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let text = toml::to_string_pretty(&s).unwrap();
        let back: ServerSettings = toml::from_str(&text).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    #[serial(env)]
    fn load_with_missing_file_uses_defaults() {
        let tmp = tempfile::TempDir::new().unwrap();
        let missing = tmp.path().join("nope.toml");
        let settings = load_server_settings(Some(&missing), ConfigOverrides::default()).unwrap();
        assert_eq!(settings, ServerSettings::default());
    }

    #[test]
    #[serial(env)]
    fn cli_overrides_beat_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
display_name = "from-file"
listen_addr = "0.0.0.0:12345"
"#,
        )
        .unwrap();

        let ov = ConfigOverrides {
            display_name: Some("from-cli".into()),
            ..Default::default()
        };
        let s = load_server_settings(Some(&path), ov).unwrap();
        assert_eq!(s.display_name, "from-cli");
        assert_eq!(s.listen_addr.to_string(), "0.0.0.0:12345");
    }

    #[test]
    #[serial(env)]
    #[allow(unsafe_code, reason = "std::env::set_var is unsafe on modern Rust")]
    fn env_overrides_beat_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "display_name = \"from-file\"\n").unwrap();

        // SAFETY: test is single-threaded and cleans the variable up
        // immediately; no concurrent readers racing with us.
        unsafe {
            std::env::set_var("HOP_DISPLAY_NAME", "from-env");
        }
        let s = load_server_settings(Some(&path), ConfigOverrides::default()).unwrap();
        // SAFETY: same reasoning as the set_var above.
        unsafe {
            std::env::remove_var("HOP_DISPLAY_NAME");
        }
        assert_eq!(s.display_name, "from-env");
    }

    #[test]
    #[serial(env)]
    fn client_settings_have_distinct_defaults() {
        let c = ClientSettings::default();
        assert_eq!(c.server_addr.to_string(), "127.0.0.1:25900");
        assert_eq!(c.display_name, "hop-client");
    }

    #[test]
    #[serial(env)]
    fn layout_path_roundtrips_through_toml() {
        let s = ServerSettings {
            layout: LayoutSettings {
                path: Some("/etc/hop/layout.toml".into()),
            },
            ..Default::default()
        };
        let text = toml::to_string_pretty(&s).unwrap();
        let back: ServerSettings = toml::from_str(&text).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    #[serial(env)]
    fn cli_layout_override_wins_over_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[layout]
path = "/from-file/layout.toml"
"#,
        )
        .unwrap();

        let ov = ConfigOverrides {
            layout_path: Some("/from-cli/layout.toml".into()),
            ..Default::default()
        };
        let s = load_server_settings(Some(&path), ov).unwrap();
        assert_eq!(
            s.layout.path.as_deref().and_then(Path::to_str),
            Some("/from-cli/layout.toml"),
        );
    }

    #[test]
    #[serial(env)]
    fn cli_address_override_targets_the_right_field() {
        let addr: SocketAddr = "10.0.0.1:1234".parse().unwrap();
        let ov_server = ConfigOverrides {
            address: Some(addr),
            ..Default::default()
        };
        let s = load_server_settings(None, ov_server).unwrap();
        assert_eq!(s.listen_addr, addr);

        let ov_client = ConfigOverrides {
            address: Some(addr),
            ..Default::default()
        };
        let c = load_client_settings(None, ov_client).unwrap();
        assert_eq!(c.server_addr, addr);
    }
}
