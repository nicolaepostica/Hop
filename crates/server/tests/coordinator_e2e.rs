//! End-to-end exercise of the M11 coordinator subsystem.
//!
//! Drives [`spawn_coordinator`](input_leap_server::coordinator::spawn_coordinator)
//! with three named screens (`laptop`, `desk`, `monitor`), registers
//! two mock clients, and feeds a scripted `MouseMove` sequence that
//! walks the cursor right across the layout boundary (`desk` → `monitor`)
//! and then back left past the primary all the way to `laptop`.
//!
//! Verifies the exact message sequence each client receives:
//!
//!  * `monitor`: `ScreenEnter` → at least one `MouseMove` → `ScreenLeave`
//!  * `laptop`:  `ScreenEnter` (reached via primary on the way back)
//!
//! The full Server + TLS handshake stack is covered separately by
//! `tests/e2e.rs`; this file tests the coordinator's routing in
//! isolation so the assertions can be deterministic.

use std::sync::Arc;
use std::time::Duration;

use input_leap_platform::{InputEvent, MockScreen};
use input_leap_protocol::Message;
use input_leap_server::coordinator::{
    spawn_coordinator, CoordinatorEvent, LayoutStore, ScreenEntry, ScreenLayout,
};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

fn three_screen_layout() -> ScreenLayout {
    ScreenLayout {
        primary: "desk".into(),
        screens: vec![
            ScreenEntry {
                name: "laptop".into(),
                origin_x: -1440,
                origin_y: 0,
                width: 1440,
                height: 900,
            },
            ScreenEntry {
                name: "desk".into(),
                origin_x: 0,
                origin_y: 0,
                width: 1920,
                height: 1080,
            },
            ScreenEntry {
                name: "monitor".into(),
                origin_x: 1920,
                origin_y: 0,
                width: 2560,
                height: 1440,
            },
        ],
    }
}

/// Drain everything the client has received so far.
async fn drain(rx: &mut mpsc::Receiver<Message>) -> Vec<Message> {
    let mut out = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(200);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match timeout(remaining, rx.recv()).await {
            Ok(Some(msg)) => out.push(msg),
            Ok(None) | Err(_) => break,
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_routes_screen_crossing_to_the_right_clients() {
    let store = LayoutStore::from_layout(three_screen_layout());
    let screen = Arc::new(MockScreen::default_stub());
    let shutdown = CancellationToken::new();

    let (handle, coord_task, dispatcher_task) =
        spawn_coordinator(store.handle(), "desk".into(), Arc::clone(&screen), &shutdown);

    let (laptop_tx, mut laptop_rx) = mpsc::channel::<Message>(64);
    handle
        .register_client("laptop".into(), laptop_tx, vec![])
        .await
        .unwrap();

    let (monitor_tx, mut monitor_rx) = mpsc::channel::<Message>(64);
    handle
        .register_client("monitor".into(), monitor_tx, vec![])
        .await
        .unwrap();

    // Bootstrap the platform cursor at the desk centre.
    handle
        .send_event(CoordinatorEvent::LocalInput(InputEvent::MouseMove {
            x: 960,
            y: 540,
        }))
        .await
        .unwrap();

    // 1. Cross right into monitor. +1100 px on x → virtual (2060, 540),
    //    which sits inside the monitor rect (origin_x = 1920, width 2560).
    handle
        .send_event(CoordinatorEvent::LocalInput(InputEvent::MouseMove {
            x: 2060,
            y: 540,
        }))
        .await
        .unwrap();

    // 2. Continue moving inside monitor so it sees a MouseMove.
    handle
        .send_event(CoordinatorEvent::LocalInput(InputEvent::MouseMove {
            x: 2200,
            y: 540,
        }))
        .await
        .unwrap();

    // 3. Cross back left into desk, then keep going into laptop.
    //    From platform_pos (2200, 540) with virtual cursor (2200, 540),
    //    a delta of (−2300, 0) → virtual (−100, 540), which is inside
    //    laptop (origin_x = -1440, width 1440 → [-1440, 0)).
    handle
        .send_event(CoordinatorEvent::LocalInput(InputEvent::MouseMove {
            x: -100,
            y: 540,
        }))
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let monitor_msgs = drain(&mut monitor_rx).await;
    let laptop_msgs = drain(&mut laptop_rx).await;

    // ---- Monitor side ----
    assert!(
        matches!(monitor_msgs.first(), Some(Message::ScreenEnter { .. })),
        "monitor's first message should be ScreenEnter, got {monitor_msgs:?}"
    );
    assert!(
        monitor_msgs
            .iter()
            .any(|m| matches!(m, Message::MouseMove { .. })),
        "monitor should have seen at least one MouseMove, got {monitor_msgs:?}"
    );
    assert!(
        matches!(monitor_msgs.last(), Some(Message::ScreenLeave)),
        "monitor's last message should be ScreenLeave, got {monitor_msgs:?}"
    );

    // ---- Laptop side ----
    assert!(
        matches!(laptop_msgs.first(), Some(Message::ScreenEnter { .. })),
        "laptop's first message should be ScreenEnter, got {laptop_msgs:?}"
    );
    assert!(
        !laptop_msgs
            .iter()
            .any(|m| matches!(m, Message::ScreenLeave)),
        "laptop should not see ScreenLeave (cursor ended there), got {laptop_msgs:?}"
    );

    shutdown.cancel();
    let _ = timeout(Duration::from_secs(1), coord_task).await;
    let _ = timeout(Duration::from_secs(1), dispatcher_task).await;
}
