//! Tray context-menu construction and id-based dispatch.
//!
//! The actual `MenuItem` / `CheckMenuItem` handles are not `Send` (they
//! wrap `Rc<RefCell<…>>` from `muda`), so they live on whichever thread
//! owns the [`tray_icon::TrayIcon`]: the eframe main thread on macOS /
//! Windows, the GTK worker thread on Linux. What this module exposes is
//! the resulting `Menu` (boxed for `tray-icon`) plus a [`MenuIds`]
//! handle that copies the assigned [`MenuId`]s so the dispatcher can map
//! an incoming `MenuEvent` back to a typed [`TrayCommand`].

use tray_icon::menu::{
    CheckMenuItem, IsMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem,
};

use crate::AppMode;

use super::TrayCommand;

/// Identifiers for every menu item the tray exposes. `MenuId` is a
/// `String` newtype, so this struct is `Send + Sync` and can travel
/// from the GTK worker back to the eframe main thread for dispatch.
#[derive(Debug, Clone)]
pub struct MenuIds {
    /// Disabled label at the top of the menu — never fires.
    pub status_header: MenuId,
    /// "Show window" entry.
    pub show_window: MenuId,
    /// Server radio in the mode submenu.
    pub mode_server: MenuId,
    /// Client radio in the mode submenu.
    pub mode_client: MenuId,
    /// Start/Stop toggle entry. Label flips based on backend state.
    pub start_stop: MenuId,
    /// "About Hop" entry.
    pub about: MenuId,
    /// "Quit" entry.
    pub quit: MenuId,
}

impl MenuIds {
    /// Build the static menu and return it together with the id table.
    ///
    /// Caller (per-OS backend) is responsible for handing the returned
    /// `Menu` to `TrayIconBuilder::with_menu`.
    pub fn build() -> (Menu, Self) {
        let menu = Menu::new();

        let status_header =
            MenuItem::with_id("hop.tray.status", "Hop — Idle", false, None);
        let show_window =
            MenuItem::with_id("hop.tray.show", "Show window", true, None);
        let sep1 = PredefinedMenuItem::separator();
        let mode_server = CheckMenuItem::with_id(
            "hop.tray.mode_server",
            "Server",
            true,
            true,
            None,
        );
        let mode_client = CheckMenuItem::with_id(
            "hop.tray.mode_client",
            "Client",
            true,
            false,
            None,
        );
        let sep2 = PredefinedMenuItem::separator();
        let start_stop =
            MenuItem::with_id("hop.tray.start_stop", "Start", true, None);
        let sep3 = PredefinedMenuItem::separator();
        let about = MenuItem::with_id("hop.tray.about", "About Hop", true, None);
        let quit = MenuItem::with_id("hop.tray.quit", "Quit", true, None);

        let items: [&dyn IsMenuItem; 10] = [
            &status_header,
            &sep1,
            &show_window,
            &mode_server,
            &mode_client,
            &sep2,
            &start_stop,
            &sep3,
            &about,
            &quit,
        ];
        menu.append_items(&items)
            .expect("muda::Menu::append_items at construction");

        let ids = Self {
            status_header: status_header.id().clone(),
            show_window: show_window.id().clone(),
            mode_server: mode_server.id().clone(),
            mode_client: mode_client.id().clone(),
            start_stop: start_stop.id().clone(),
            about: about.id().clone(),
            quit: quit.id().clone(),
        };

        (menu, ids)
    }

    /// Translate an incoming menu event id into a typed [`TrayCommand`].
    /// Returns `None` for ids we do not recognise (e.g. the disabled
    /// status header, which never fires but is matched defensively).
    pub fn dispatch(&self, id: &MenuId) -> Option<TrayCommand> {
        if id == &self.show_window {
            Some(TrayCommand::ShowWindow)
        } else if id == &self.mode_server {
            Some(TrayCommand::SwitchMode(AppMode::Server))
        } else if id == &self.mode_client {
            Some(TrayCommand::SwitchMode(AppMode::Client))
        } else if id == &self.start_stop {
            Some(TrayCommand::StartOrStop)
        } else if id == &self.about {
            Some(TrayCommand::About)
        } else if id == &self.quit {
            Some(TrayCommand::Quit)
        } else {
            None
        }
    }
}
