//! Screen layout — a rect-based virtual coordinate space.
//!
//! Each screen is a rectangle (`origin_x`, `origin_y`, `width`,
//! `height`) in the global virtual space; the cursor is a single
//! `(vx, vy)` point, and the "active screen" is whichever rectangle
//! currently contains it. This is the same model Barrier/Synergy uses
//! and it handles irregular arrangements naturally — for example a
//! laptop positioned above-and-to-the-left of the primary, or three
//! monitors in an L-shape, just by giving each screen the right origin.
//!
//! Reloading is live via [`LayoutStore`]: the store wraps an
//! `ArcSwap<ScreenLayout>` so readers on the hot path pay only a
//! pointer-bump per event, while a GUI-driven `reload_layout` swaps in
//! the new snapshot atomically.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::warn;

/// Name of a screen. Matches the `display_name` peers send in `Hello`.
pub type ScreenName = String;

/// A single screen's rectangle in the virtual coordinate space.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScreenEntry {
    /// Display name (must match the peer's `Hello.display_name`).
    pub name: ScreenName,
    /// X coordinate of the screen's top-left corner in virtual space.
    pub origin_x: i32,
    /// Y coordinate of the screen's top-left corner in virtual space.
    pub origin_y: i32,
    /// Physical resolution in pixels.
    pub width: u32,
    /// Physical resolution in pixels.
    pub height: u32,
}

impl ScreenEntry {
    /// Does `(vx, vy)` fall inside this screen's rectangle?
    ///
    /// The rectangle is half-open: `[origin, origin + size)`. A point
    /// exactly on the right/bottom edge belongs to the *neighbouring*
    /// screen if one exists, which keeps crossings from double-counting.
    #[must_use]
    pub fn contains(&self, vx: i32, vy: i32) -> bool {
        let max_x = self
            .origin_x
            .saturating_add(i32::try_from(self.width).unwrap_or(i32::MAX));
        let max_y = self
            .origin_y
            .saturating_add(i32::try_from(self.height).unwrap_or(i32::MAX));
        vx >= self.origin_x && vx < max_x && vy >= self.origin_y && vy < max_y
    }

    /// Clamp `(vx, vy)` to the closest point inside this screen.
    ///
    /// Used when a drag (held mouse button) tries to cross the edge:
    /// we refuse to switch screens mid-drag and instead pin the cursor
    /// to the current screen's border.
    #[must_use]
    pub fn clamp(&self, vx: i32, vy: i32) -> (i32, i32) {
        // width == 0 would make `max_*` < origin, which clamp can't
        // satisfy; degenerate screens are rejected at load time, but
        // we saturate defensively here anyway.
        let max_x = self
            .origin_x
            .saturating_add(i32::try_from(self.width.saturating_sub(1)).unwrap_or(i32::MAX));
        let max_y = self
            .origin_y
            .saturating_add(i32::try_from(self.height.saturating_sub(1)).unwrap_or(i32::MAX));
        (vx.clamp(self.origin_x, max_x), vy.clamp(self.origin_y, max_y))
    }
}

/// Full layout: primary screen + all known screens.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ScreenLayout {
    /// Name of the primary (server-side) screen.
    pub primary: ScreenName,
    /// All screens, including the primary.
    #[serde(rename = "screen", default)]
    pub screens: Vec<ScreenEntry>,
}

impl ScreenLayout {
    /// Build a degenerate layout containing only the primary at origin.
    ///
    /// Used at first-run when no `layout.toml` has been authored yet —
    /// the server still starts, it just can't route input anywhere.
    #[must_use]
    pub fn single_primary(name: impl Into<ScreenName>) -> Self {
        let name = name.into();
        Self {
            primary: name.clone(),
            screens: vec![ScreenEntry {
                name,
                origin_x: 0,
                origin_y: 0,
                width: 1920,
                height: 1080,
            }],
        }
    }

    /// Find the screen whose rectangle contains `(vx, vy)`, if any.
    #[must_use]
    pub fn screen_at(&self, vx: i32, vy: i32) -> Option<&ScreenEntry> {
        self.screens.iter().find(|s| s.contains(vx, vy))
    }

    /// Look up a screen by its display name.
    #[must_use]
    pub fn screen_by_name(&self, name: &str) -> Option<&ScreenEntry> {
        self.screens.iter().find(|s| s.name == name)
    }

    /// Validate internal invariants after deserialization. Called by
    /// [`LayoutStore::load`] before publishing the layout.
    fn validate(&self) -> Result<(), LayoutError> {
        if self.screens.iter().all(|s| s.name != self.primary) {
            return Err(LayoutError::PrimaryNotInScreens {
                primary: self.primary.clone(),
            });
        }
        for s in &self.screens {
            if s.width == 0 || s.height == 0 {
                return Err(LayoutError::DegenerateScreen {
                    name: s.name.clone(),
                });
            }
        }
        for i in 0..self.screens.len() {
            for j in (i + 1)..self.screens.len() {
                if self.screens[i].name == self.screens[j].name {
                    return Err(LayoutError::DuplicateScreen {
                        name: self.screens[i].name.clone(),
                    });
                }
            }
        }
        Ok(())
    }
}

/// Shared `ArcSwap<ScreenLayout>` snapshot — hot-reload handle used
/// by the coordinator hot path.
pub type SharedLayout = Arc<ArcSwap<ScreenLayout>>;

/// Errors parsing or validating a layout file.
#[derive(Debug, Error)]
pub enum LayoutError {
    /// I/O error reading the layout file.
    #[error("cannot read layout file {path}: {source}")]
    Io {
        /// Path we tried to read.
        path: PathBuf,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },

    /// TOML parse failure.
    #[error("failed to parse layout: {0}")]
    Parse(String),

    /// `primary` names a screen that does not appear in `[[screen]]`.
    #[error("primary screen `{primary}` is not defined in [[screen]] list")]
    PrimaryNotInScreens {
        /// The primary name that is missing.
        primary: ScreenName,
    },

    /// A screen declared zero width or height.
    #[error("screen `{name}` has zero width or height")]
    DegenerateScreen {
        /// The offending screen.
        name: ScreenName,
    },

    /// Two screens share the same name.
    #[error("screen name `{name}` appears more than once")]
    DuplicateScreen {
        /// The duplicate name.
        name: ScreenName,
    },
}

/// Hot-reloadable layout backed by a TOML file on disk.
#[derive(Debug)]
pub struct LayoutStore {
    path: PathBuf,
    inner: SharedLayout,
}

impl LayoutStore {
    /// Load the layout from `path`.
    ///
    /// Missing file is **not** an error — the store defaults to a
    /// lone-primary layout with a warning in the log. This keeps the
    /// server's first-run experience smooth (user runs it, it boots,
    /// then the GUI / CLI writes an actual layout file).
    pub fn load(path: PathBuf, primary_fallback: &str) -> Result<Self, LayoutError> {
        let layout = match std::fs::read_to_string(&path) {
            Ok(text) => {
                let parsed: ScreenLayout =
                    toml::from_str(&text).map_err(|e| LayoutError::Parse(e.to_string()))?;
                parsed.validate()?;
                parsed
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                warn!(
                    path = %path.display(),
                    "layout file not found; starting with a single-primary layout. \
                     Add at least one client screen to route input."
                );
                ScreenLayout::single_primary(primary_fallback)
            }
            Err(err) => {
                return Err(LayoutError::Io { path, source: err });
            }
        };
        Ok(Self {
            path,
            inner: Arc::new(ArcSwap::from_pointee(layout)),
        })
    }

    /// Construct an in-memory store (mainly for tests).
    #[must_use]
    pub fn from_layout(layout: ScreenLayout) -> Self {
        Self {
            path: PathBuf::new(),
            inner: Arc::new(ArcSwap::from_pointee(layout)),
        }
    }

    /// Cheap: returns an `Arc<ScreenLayout>` snapshot (pointer-bump).
    #[must_use]
    pub fn snapshot(&self) -> Arc<ScreenLayout> {
        self.inner.load_full()
    }

    /// Shared handle suitable for the coordinator's hot path.
    #[must_use]
    pub fn handle(&self) -> SharedLayout {
        Arc::clone(&self.inner)
    }

    /// Where this store was loaded from (empty for `from_layout`).
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Re-read the file and atomically swap the stored layout.
    pub fn reload(&self) -> Result<(), LayoutError> {
        if self.path.as_os_str().is_empty() {
            return Err(LayoutError::Io {
                path: self.path.clone(),
                source: std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "LayoutStore has no backing file to reload from",
                ),
            });
        }
        let text = std::fs::read_to_string(&self.path).map_err(|err| LayoutError::Io {
            path: self.path.clone(),
            source: err,
        })?;
        let parsed: ScreenLayout =
            toml::from_str(&text).map_err(|e| LayoutError::Parse(e.to_string()))?;
        parsed.validate()?;
        self.inner.store(Arc::new(parsed));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(name: &str, x: i32, y: i32, w: u32, h: u32) -> ScreenEntry {
        ScreenEntry {
            name: name.into(),
            origin_x: x,
            origin_y: y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn contains_half_open_rect() {
        let s = screen("a", 0, 0, 100, 100);
        assert!(s.contains(0, 0), "top-left corner is inside");
        assert!(s.contains(99, 99), "bottom-right-minus-one is inside");
        assert!(!s.contains(100, 50), "right edge is outside (half-open)");
        assert!(!s.contains(50, 100), "bottom edge is outside");
        assert!(!s.contains(-1, 50), "left of origin is outside");
    }

    #[test]
    fn contains_respects_negative_origin() {
        let s = screen("laptop", -1440, 90, 1440, 900);
        assert!(s.contains(-1440, 90));
        assert!(s.contains(-1, 500));
        assert!(!s.contains(0, 500));
    }

    #[test]
    fn clamp_is_identity_inside() {
        let s = screen("a", 0, 0, 100, 100);
        assert_eq!(s.clamp(50, 50), (50, 50));
    }

    #[test]
    fn clamp_pulls_back_when_outside() {
        let s = screen("a", 0, 0, 100, 100);
        assert_eq!(s.clamp(200, 50), (99, 50));
        assert_eq!(s.clamp(-10, 50), (0, 50));
        assert_eq!(s.clamp(50, 200), (50, 99));
    }

    #[test]
    fn screen_at_finds_right_screen() {
        let layout = ScreenLayout {
            primary: "desk".into(),
            screens: vec![
                screen("laptop", -1440, 0, 1440, 900),
                screen("desk", 0, 0, 1920, 1080),
                screen("monitor", 1920, 0, 2560, 1440),
            ],
        };
        assert_eq!(layout.screen_at(-100, 500).unwrap().name, "laptop");
        assert_eq!(layout.screen_at(500, 500).unwrap().name, "desk");
        assert_eq!(layout.screen_at(2500, 500).unwrap().name, "monitor");
    }

    #[test]
    fn screen_at_gap_returns_none() {
        let layout = ScreenLayout {
            primary: "a".into(),
            screens: vec![screen("a", 0, 0, 100, 100), screen("b", 200, 0, 100, 100)],
        };
        assert!(layout.screen_at(150, 50).is_none(), "gap between screens");
    }

    #[test]
    fn screen_by_name_returns_expected() {
        let layout = ScreenLayout {
            primary: "a".into(),
            screens: vec![screen("a", 0, 0, 100, 100), screen("b", 100, 0, 100, 100)],
        };
        assert_eq!(layout.screen_by_name("b").unwrap().origin_x, 100);
        assert!(layout.screen_by_name("missing").is_none());
    }

    #[test]
    fn single_primary_builds_valid_layout() {
        let layout = ScreenLayout::single_primary("solo");
        assert_eq!(layout.primary, "solo");
        assert_eq!(layout.screens.len(), 1);
        assert_eq!(layout.screens[0].name, "solo");
        layout.validate().expect("single_primary is valid");
    }

    #[test]
    fn validate_rejects_primary_missing_from_list() {
        let layout = ScreenLayout {
            primary: "ghost".into(),
            screens: vec![screen("a", 0, 0, 100, 100)],
        };
        assert!(matches!(
            layout.validate(),
            Err(LayoutError::PrimaryNotInScreens { .. })
        ));
    }

    #[test]
    fn validate_rejects_zero_size() {
        let layout = ScreenLayout {
            primary: "a".into(),
            screens: vec![screen("a", 0, 0, 0, 100)],
        };
        assert!(matches!(
            layout.validate(),
            Err(LayoutError::DegenerateScreen { .. })
        ));
    }

    #[test]
    fn validate_rejects_duplicate_names() {
        let layout = ScreenLayout {
            primary: "a".into(),
            screens: vec![screen("a", 0, 0, 100, 100), screen("a", 100, 0, 100, 100)],
        };
        assert!(matches!(
            layout.validate(),
            Err(LayoutError::DuplicateScreen { .. })
        ));
    }

    #[test]
    fn toml_round_trip_preserves_layout() {
        let layout = ScreenLayout {
            primary: "desk".into(),
            screens: vec![
                screen("laptop", -1440, 90, 1440, 900),
                screen("desk", 0, 0, 1920, 1080),
            ],
        };
        let text = toml::to_string_pretty(&layout).unwrap();
        let back: ScreenLayout = toml::from_str(&text).unwrap();
        assert_eq!(layout, back);
    }

    #[test]
    fn store_load_missing_file_falls_back_to_single_primary() {
        let tmp = std::env::temp_dir().join("hop-m11-missing.toml");
        let _ = std::fs::remove_file(&tmp);
        let store = LayoutStore::load(tmp, "desk").expect("load falls back");
        let snap = store.snapshot();
        assert_eq!(snap.primary, "desk");
        assert_eq!(snap.screens.len(), 1);
    }

    #[test]
    fn store_reload_picks_up_changes() {
        let tmp = std::env::temp_dir().join("hop-m11-reload.toml");
        let v1 = r#"
primary = "desk"

[[screen]]
name     = "desk"
origin_x = 0
origin_y = 0
width    = 1920
height   = 1080
"#;
        std::fs::write(&tmp, v1).unwrap();

        let store = LayoutStore::load(tmp.clone(), "desk").unwrap();
        assert_eq!(store.snapshot().screens.len(), 1);

        let v2 = r#"
primary = "desk"

[[screen]]
name     = "desk"
origin_x = 0
origin_y = 0
width    = 1920
height   = 1080

[[screen]]
name     = "laptop"
origin_x = -1440
origin_y = 0
width    = 1440
height   = 900
"#;
        std::fs::write(&tmp, v2).unwrap();
        store.reload().unwrap();
        assert_eq!(store.snapshot().screens.len(), 2);

        let _ = std::fs::remove_file(&tmp);
    }
}
