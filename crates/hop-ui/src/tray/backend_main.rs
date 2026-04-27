//! macOS / Windows tray backend.
//!
//! On these OSes `tray-icon` piggybacks on the same event loop eframe
//! already runs (`NSRunLoop` on macOS, the Win32 message pump on
//! Windows), so the tray is constructed on the eframe main thread and
//! events are drained from `MenuEvent::receiver()` / `TrayIconEvent::
//! receiver()` inside `HopApp::update`.
//!
//! See `specs/milestones/M14-tray.md §Architecture` for the per-OS
//! decision.

use tracing::warn;
use tray_icon::menu::MenuEvent;
use tray_icon::{TrayIcon, TrayIconBuilder, TrayIconEvent};

use super::icons::TrayIcons;
use super::menu::MenuIds;
use super::{TrayCommand, TrayError, TrayState};
use crate::AppMode;

pub struct MainThreadTray {
    _tray: TrayIcon,
    icons: TrayIcons,
    ids: MenuIds,
    last_state: Option<TrayState>,
}

impl MainThreadTray {
    pub fn try_new() -> Result<Self, TrayError> {
        let icons = TrayIcons::load().map_err(TrayError::Icons)?;
        let (menu, ids) = MenuIds::build();

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Hop")
            .with_icon(icons.idle.clone())
            .build()
            .map_err(TrayError::Build)?;

        Ok(Self {
            _tray: tray,
            icons,
            ids,
            last_state: None,
        })
    }

    pub fn reconcile(
        &mut self,
        state: TrayState,
        _mode_locked: bool,
        _mode: AppMode,
    ) {
        if self.last_state == Some(state) {
            return;
        }
        self.last_state = Some(state);
        // Real reconcile (icon swap, label updates, enable flags) lands
        // in Commit 2. Touch the icon set so the field stays read.
        let _ = (&self.icons.idle, &self.icons.server, &self.icons.client);
    }

    pub fn poll(&self) -> Vec<TrayCommand> {
        let mut out = Vec::new();
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            if let Some(cmd) = self.ids.dispatch(&ev.id) {
                out.push(cmd);
            } else {
                warn!(id = %ev.id.0, "tray menu event with unknown id");
            }
        }
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            if let Some(cmd) = icon_event_to_command(&ev) {
                out.push(cmd);
            }
        }
        out
    }
}

fn icon_event_to_command(ev: &TrayIconEvent) -> Option<TrayCommand> {
    use tray_icon::{MouseButton, MouseButtonState};
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
    } = ev
    {
        Some(TrayCommand::ShowWindow)
    } else {
        None
    }
}
