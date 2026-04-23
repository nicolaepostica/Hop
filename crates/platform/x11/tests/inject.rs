//! Integration tests for `X11Screen` injection.
//!
//! Each test spins up its own `Xvfb` instance on a unique display
//! number, connects to it, runs the operation under test, and then
//! verifies via a second connection. If the `Xvfb` binary is not
//! available the test logs a notice and exits successfully — the same
//! code path runs on the Ubuntu CI runner where `xvfb` is installed
//! explicitly.

#![cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd"))]

use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use hop_common::{ButtonId, KeyId, ModifierMask};
use hop_platform::PlatformScreen;
use hop_platform_x11::X11Screen;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::ConnectionExt as _;

/// Pick a fresh display number for every test that wants one so the
/// harness can run tests in parallel without colliding on sockets.
/// Displays 90+ are very unlikely to be claimed by a real session.
fn next_display() -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(90);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(":{n}")
}

fn xvfb_available() -> bool {
    Command::new("Xvfb")
        .arg("-help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// Spawn Xvfb and wait for its socket to appear. Returns `None` when
/// the binary is not installed, so callers can skip gracefully.
fn spawn_xvfb(display: &str) -> Option<KillOnDrop> {
    if !xvfb_available() {
        return None;
    }
    let child = Command::new("Xvfb")
        .args([display, "-screen", "0", "800x600x24", "-nolisten", "tcp"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let guard = KillOnDrop(Some(child));
    let display_num = display.trim_start_matches(':');
    let socket = std::path::PathBuf::from(format!("/tmp/.X11-unix/X{display_num}"));
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if socket.exists() {
            // Small grace period so the server is actually accepting.
            thread::sleep(Duration::from_millis(50));
            return Some(guard);
        }
        thread::sleep(Duration::from_millis(20));
    }
    // Xvfb started but never exposed its socket — treat as unavailable.
    None
}

struct KillOnDrop(Option<Child>);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn inject_mouse_move_updates_pointer() {
    let display = next_display();
    let Some(_xvfb) = spawn_xvfb(&display) else {
        eprintln!("Xvfb not installed; skipping");
        return;
    };

    let screen = X11Screen::open(Some(&display)).expect("open X11 on Xvfb");
    screen.inject_mouse_move(321, 234).await.unwrap();

    // Read back via an independent connection.
    let (conn, screen_num) = x11rb::connect(Some(&display)).unwrap();
    let root = conn.setup().roots[screen_num].root;
    let reply = conn.query_pointer(root).unwrap().reply().unwrap();
    assert_eq!(reply.root_x, 321);
    assert_eq!(reply.root_y, 234);
}

#[tokio::test(flavor = "multi_thread")]
async fn inject_mouse_button_is_accepted() {
    let display = next_display();
    let Some(_xvfb) = spawn_xvfb(&display) else {
        eprintln!("Xvfb not installed; skipping");
        return;
    };

    let screen = X11Screen::open(Some(&display)).expect("open X11 on Xvfb");
    // Press and release: the test succeeds if XTest accepts both
    // requests without a protocol error (check() inside fake_input
    // bubbles up X errors).
    screen
        .inject_mouse_button(ButtonId::LEFT, true)
        .await
        .unwrap();
    screen
        .inject_mouse_button(ButtonId::LEFT, false)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn inject_mouse_wheel_emits_button_ticks() {
    let display = next_display();
    let Some(_xvfb) = spawn_xvfb(&display) else {
        eprintln!("Xvfb not installed; skipping");
        return;
    };

    let screen = X11Screen::open(Some(&display)).expect("open X11 on Xvfb");
    // One vertical tick up, one horizontal tick right. Just verify
    // we don't error out — scroll history is not queryable via core
    // X11 the way pointer position is.
    screen.inject_mouse_wheel(120, 120).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn inject_key_for_unknown_keysym_is_a_no_op() {
    let display = next_display();
    let Some(_xvfb) = spawn_xvfb(&display) else {
        eprintln!("Xvfb not installed; skipping");
        return;
    };

    let screen = X11Screen::open(Some(&display)).expect("open X11 on Xvfb");
    // 0xdead_beef has no keycode under any layout; the call must
    // succeed silently rather than erroring out.
    screen
        .inject_key(KeyId::new(0xdead_beef), ModifierMask::empty(), true)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn screen_info_reflects_xvfb_geometry() {
    let display = next_display();
    let Some(_xvfb) = spawn_xvfb(&display) else {
        eprintln!("Xvfb not installed; skipping");
        return;
    };

    let screen = X11Screen::open(Some(&display)).expect("open X11 on Xvfb");
    let info = screen.screen_info();
    assert_eq!(info.width, 800);
    assert_eq!(info.height, 600);
}
