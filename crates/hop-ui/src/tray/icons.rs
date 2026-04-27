//! Pre-decoded `tray_icon::Icon` variants for the three runtime states.
//!
//! Commit 1 (scaffolding) ships a single image — the brand mark from
//! `assets/hop.png` — for all three slots. Per-state monochrome and
//! coloured variants land alongside `scripts/gen-icons.sh` updates in
//! a follow-up; see `specs/milestones/M14-tray.md §Icon assets`.

use thiserror::Error;
use tray_icon::Icon;

const HOP_PNG: &[u8] = include_bytes!("../../../../assets/hop.png");

/// Three icon variants displayed in the system tray.
pub struct TrayIcons {
    /// Shown when no backend is running.
    pub idle: Icon,
    /// Shown while the server is listening.
    pub server: Icon,
    /// Shown while the client is connected to a remote server.
    pub client: Icon,
}

/// Failure to decode the embedded PNG or hand it to `tray-icon`.
#[derive(Debug, Error)]
pub enum IconError {
    /// The bundled PNG could not be decoded into RGBA.
    #[error("decode bundled PNG: {0}")]
    Decode(String),
    /// `tray-icon` rejected the RGBA buffer (size mismatch, etc.).
    #[error("build tray icon: {0}")]
    Build(#[from] tray_icon::BadIcon),
}

impl TrayIcons {
    /// Decode the embedded PNG once and clone it into three slots.
    pub fn load() -> Result<Self, IconError> {
        let data = eframe::icon_data::from_png_bytes(HOP_PNG)
            .map_err(|e| IconError::Decode(e.to_string()))?;

        let icon = Icon::from_rgba(data.rgba, data.width, data.height)?;
        Ok(Self {
            idle: icon.clone(),
            server: icon.clone(),
            client: icon,
        })
    }
}
