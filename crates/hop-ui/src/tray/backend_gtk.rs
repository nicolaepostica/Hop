//! Linux tray backend — GTK worker thread.
//!
//! `tray-icon`'s Linux backend (`libappindicator` over D-Bus, glued
//! through GTK) requires `gtk::init()` and a live `gtk::main()` loop
//! on the same thread that owns the [`tray_icon::TrayIcon`]. eframe
//! does not run a GTK loop, so we spawn a dedicated worker that does.
//!
//! The worker owns the `TrayIcon` and `Menu` for its full lifetime;
//! the eframe main thread only holds [`GtkWorkerHandle`], which is
//! channel ends plus the `JoinHandle`. See `specs/milestones/M14-tray.md
//! §Architecture` for the per-OS decision.

use std::sync::Once;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::{bounded, unbounded, Receiver, Sender, TryRecvError};
use gtk::glib;
use tracing::{debug, warn};
use tray_icon::menu::MenuEvent;
use tray_icon::{TrayIconBuilder, TrayIconEvent};

use super::icons::TrayIcons;
use super::menu::MenuIds;
use super::{TrayCommand, TrayError, TrayState};
use crate::AppMode;

/// Process-wide guard. `gtk::init()` panics if called twice.
static GTK_INIT: Once = Once::new();

enum WorkerCmd {
    Reconcile {
        state: TrayState,
        mode_locked: bool,
        mode: AppMode,
    },
    Shutdown,
}

pub struct GtkWorkerHandle {
    cmd_tx: Sender<WorkerCmd>,
    evt_rx: Receiver<TrayCommand>,
    join: Option<JoinHandle<()>>,
    last_state: Option<TrayState>,
}

impl GtkWorkerHandle {
    pub fn try_new() -> Result<Self, TrayError> {
        let (cmd_tx, cmd_rx) = unbounded::<WorkerCmd>();
        let (evt_tx, evt_rx) = unbounded::<TrayCommand>();
        let (ready_tx, ready_rx) = bounded::<Result<(), TrayError>>(1);

        let join = thread::Builder::new()
            .name("hop-tray-gtk".into())
            .spawn(move || worker_main(cmd_rx, evt_tx, ready_tx))
            .map_err(|e| TrayError::WorkerSpawn(e.to_string()))?;

        match ready_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(())) => Ok(Self {
                cmd_tx,
                evt_rx,
                join: Some(join),
                last_state: None,
            }),
            Ok(Err(err)) => Err(err),
            Err(_) => Err(TrayError::WorkerSpawn(
                "GTK worker did not report ready in time".into(),
            )),
        }
    }

    pub fn reconcile(
        &mut self,
        state: TrayState,
        mode_locked: bool,
        mode: AppMode,
    ) {
        if self.last_state == Some(state) {
            return;
        }
        self.last_state = Some(state);
        if self
            .cmd_tx
            .send(WorkerCmd::Reconcile {
                state,
                mode_locked,
                mode,
            })
            .is_err()
        {
            warn!("tray worker disconnected; dropping reconcile");
        }
    }

    pub fn poll(&self) -> Vec<TrayCommand> {
        let mut out = Vec::new();
        loop {
            match self.evt_rx.try_recv() {
                Ok(cmd) => out.push(cmd),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    warn!("tray worker channel disconnected");
                    break;
                }
            }
        }
        out
    }
}

impl Drop for GtkWorkerHandle {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(WorkerCmd::Shutdown);
        if let Some(join) = self.join.take() {
            // Worker should exit promptly once gtk::main_quit() runs.
            // join() blocks; if the worker is wedged we still wait, but
            // it's expected to be near-instant. (Future: bounded join.)
            let _ = join.join();
        }
    }
}

#[allow(clippy::needless_pass_by_value)] // moved into closures + thread state
fn worker_main(
    cmd_rx: Receiver<WorkerCmd>,
    evt_tx: Sender<TrayCommand>,
    ready_tx: Sender<Result<(), TrayError>>,
) {
    let mut init_result: Result<(), TrayError> = Ok(());
    GTK_INIT.call_once(|| {
        if let Err(err) = gtk::init() {
            init_result = Err(TrayError::GtkInit(err.to_string()));
        }
    });
    if let Err(e) = init_result {
        let _ = ready_tx.send(Err(e));
        return;
    }

    let icons = match TrayIcons::load() {
        Ok(i) => i,
        Err(e) => {
            let _ = ready_tx.send(Err(TrayError::Icons(e)));
            return;
        }
    };
    let (menu, ids) = MenuIds::build();

    let tray = match TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Hop")
        .with_icon(icons.idle.clone())
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            let _ = ready_tx.send(Err(TrayError::Build(e)));
            return;
        }
    };

    let evt_tx_menu = evt_tx.clone();
    let ids_for_menu = ids.clone();
    MenuEvent::set_event_handler(Some(move |ev: MenuEvent| {
        if let Some(cmd) = ids_for_menu.dispatch(&ev.id) {
            let _ = evt_tx_menu.send(cmd);
        }
    }));
    // AppIndicator on Linux usually does not fire icon clicks (left
    // click opens the menu directly), but install the handler for
    // symmetry with macOS / Windows on desktops that do support it.
    let evt_tx_icon = evt_tx.clone();
    TrayIconEvent::set_event_handler(Some(move |ev: TrayIconEvent| {
        use tray_icon::{MouseButton, MouseButtonState};
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = ev
        {
            let _ = evt_tx_icon.send(TrayCommand::ShowWindow);
        }
    }));

    // Pump cmd_rx from the GTK loop. 100 ms cadence is eyes-fast and
    // imperceptible CPU on a modern desktop.
    let cmd_rx_pump = cmd_rx;
    glib::timeout_add_local(Duration::from_millis(100), move || {
        loop {
            match cmd_rx_pump.try_recv() {
                Ok(WorkerCmd::Reconcile { .. }) => {
                    // Real reconcile (icon swap, label updates, enabled
                    // flags) lands in Commit 2; the message is currently
                    // accepted and dropped.
                }
                Ok(WorkerCmd::Shutdown) | Err(TryRecvError::Disconnected) => {
                    gtk::main_quit();
                    return glib::ControlFlow::Break;
                }
                Err(TryRecvError::Empty) => return glib::ControlFlow::Continue,
            }
        }
    });

    let _ = ready_tx.send(Ok(()));
    gtk::main();

    drop(tray);
    drop(icons);
    debug!(target: "hop_ui::tray", "GTK worker exited");
}
