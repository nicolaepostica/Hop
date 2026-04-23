//! Default filesystem paths for config and data.

use std::path::{Path, PathBuf};

use directories::{ProjectDirs, UserDirs};

const QUALIFIER: &str = "com";
const ORGANIZATION: &str = "InputLeap";
const APPLICATION: &str = "input-leap";

/// Project-scoped directories (XDG on Linux, standard locations elsewhere).
fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
}

/// Default path for `config.toml`. Returns `None` only on systems
/// where `directories` cannot determine a home (unusual; e.g. a
/// container with no `HOME`).
#[must_use]
pub fn default_config_path() -> Option<PathBuf> {
    project_dirs().map(|d| d.config_dir().join("config.toml"))
}

/// Default path for `layout.toml`. Lives next to `config.toml` so the
/// GUI can atomically rewrite it (tempfile + rename) without disturbing
/// the admin-authored `config.toml`.
#[must_use]
pub fn default_layout_path() -> Option<PathBuf> {
    project_dirs().map(|d| d.config_dir().join("layout.toml"))
}

/// Default drop directory for received files
/// (`<user-download>/InputLeap`). Falls back to the temp dir if the
/// user doesn't have a recognisable Downloads folder.
#[must_use]
pub fn default_drop_directory() -> PathBuf {
    if let Some(user) = UserDirs::new() {
        if let Some(downloads) = user.download_dir() {
            return downloads.join("InputLeap");
        }
    }
    std::env::temp_dir().join("InputLeap")
}

/// Expand `~` and `$VAR` sequences in a user-supplied path.
///
/// Leaves the path untouched if it contains nothing to expand; returns
/// `None` if expansion fails (e.g. `$HOME` unset).
#[must_use]
pub fn expand_user_path(raw: &Path) -> Option<PathBuf> {
    let raw = raw.to_str()?;
    let expanded = shellexpand::full(raw).ok()?;
    Some(PathBuf::from(expanded.into_owned()))
}

#[cfg(test)]
mod tests {
    use serial_test::serial;

    use super::*;

    #[test]
    #[serial(env)]
    fn expand_handles_literal_paths() {
        let raw = Path::new("/tmp/inputleap");
        assert_eq!(
            expand_user_path(raw).unwrap(),
            PathBuf::from("/tmp/inputleap")
        );
    }

    #[test]
    #[serial(env)]
    #[allow(unsafe_code, reason = "std::env::set_var is unsafe on modern Rust")]
    fn expand_fills_in_env_var() {
        // SAFETY: test is single-threaded and cleans the variable up
        // immediately; no concurrent readers racing with us.
        unsafe {
            std::env::set_var("INPUT_LEAP_TEST_DIR", "/opt/inputleap");
        }
        let raw = Path::new("$INPUT_LEAP_TEST_DIR/cfg");
        let got = expand_user_path(raw).unwrap();
        assert_eq!(got, PathBuf::from("/opt/inputleap/cfg"));
        // SAFETY: same reasoning as the set_var above.
        unsafe {
            std::env::remove_var("INPUT_LEAP_TEST_DIR");
        }
    }
}
