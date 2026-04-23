//! TOML configuration loader for Hop.
//!
//! Produces [`ServerSettings`] / [`ClientSettings`] by merging, in
//! order of increasing priority:
//!
//! 1. Hard-coded defaults.
//! 2. A TOML file at `<config_dir>/config.toml` (missing is fine).
//! 3. Environment variables prefixed with `HOP_`.
//! 4. Overrides supplied programmatically (typically from CLI).
//!
//! `figment` handles layering. Path expansion uses the `directories`
//! and `shellexpand` crates so a config like
//! `drop_directory = "~/Downloads/Hop"` resolves to the current
//! user's actual download location.

pub mod paths;
mod settings;

pub use self::paths::{
    default_config_path, default_drop_directory, default_layout_path, expand_user_path,
};
pub use self::settings::{
    load_client_settings, load_server_settings, ClientSettings, ConfigError, ConfigOverrides,
    FileTransferSettings, LayoutSettings, ServerSettings, TlsSettings,
};
