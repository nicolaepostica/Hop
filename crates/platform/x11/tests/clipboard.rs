//! X11 clipboard integration tests.
//!
//! Each test starts its own Xvfb instance so the tests can run in
//! parallel and on a CI runner without touching the developer's live
//! session. Tests skip gracefully when Xvfb is not installed.

#![cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd"))]

use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use bytes::Bytes;
use input_leap_common::{ClipboardFormat, ClipboardId};
use input_leap_platform::PlatformScreen;
use input_leap_platform_x11::X11Screen;

fn next_display() -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(80);
    format!(":{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

fn xvfb_available() -> bool {
    Command::new("Xvfb")
        .arg("-help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

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
    let num = display.trim_start_matches(':');
    let socket = std::path::PathBuf::from(format!("/tmp/.X11-unix/X{num}"));
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if socket.exists() {
            thread::sleep(Duration::from_millis(50));
            return Some(guard);
        }
        thread::sleep(Duration::from_millis(20));
    }
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
async fn text_round_trip_within_process() {
    let display = next_display();
    let Some(_xvfb) = spawn_xvfb(&display) else {
        eprintln!("Xvfb not installed; skipping");
        return;
    };
    let screen = X11Screen::open(Some(&display)).expect("open X11");
    screen
        .set_clipboard(
            ClipboardId::Clipboard,
            ClipboardFormat::Text,
            Bytes::from_static(b"hello from input-leap"),
        )
        .await
        .unwrap();

    // Give the worker a moment to process the set.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let got = screen
        .get_clipboard(ClipboardId::Clipboard, ClipboardFormat::Text)
        .await
        .unwrap();
    assert_eq!(got.as_ref(), b"hello from input-leap");
}

#[tokio::test(flavor = "multi_thread")]
async fn html_round_trip_within_process() {
    let display = next_display();
    let Some(_xvfb) = spawn_xvfb(&display) else {
        eprintln!("Xvfb not installed; skipping");
        return;
    };
    let screen = X11Screen::open(Some(&display)).expect("open X11");
    let payload: &[u8] = b"<b>bold</b>";
    screen
        .set_clipboard(
            ClipboardId::Clipboard,
            ClipboardFormat::Html,
            Bytes::copy_from_slice(payload),
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let got = screen
        .get_clipboard(ClipboardId::Clipboard, ClipboardFormat::Html)
        .await
        .unwrap();
    assert_eq!(got.as_ref(), payload);
}

#[tokio::test(flavor = "multi_thread")]
async fn primary_and_clipboard_are_independent() {
    let display = next_display();
    let Some(_xvfb) = spawn_xvfb(&display) else {
        eprintln!("Xvfb not installed; skipping");
        return;
    };
    let screen = X11Screen::open(Some(&display)).expect("open X11");
    screen
        .set_clipboard(
            ClipboardId::Clipboard,
            ClipboardFormat::Text,
            Bytes::from_static(b"in clipboard"),
        )
        .await
        .unwrap();
    screen
        .set_clipboard(
            ClipboardId::Primary,
            ClipboardFormat::Text,
            Bytes::from_static(b"in primary"),
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let clip = screen
        .get_clipboard(ClipboardId::Clipboard, ClipboardFormat::Text)
        .await
        .unwrap();
    let prim = screen
        .get_clipboard(ClipboardId::Primary, ClipboardFormat::Text)
        .await
        .unwrap();
    assert_eq!(clip.as_ref(), b"in clipboard");
    assert_eq!(prim.as_ref(), b"in primary");
}

#[tokio::test(flavor = "multi_thread")]
async fn reading_empty_selection_returns_empty() {
    let display = next_display();
    let Some(_xvfb) = spawn_xvfb(&display) else {
        eprintln!("Xvfb not installed; skipping");
        return;
    };
    let screen = X11Screen::open(Some(&display)).expect("open X11");
    let got = screen
        .get_clipboard(ClipboardId::Clipboard, ClipboardFormat::Text)
        .await
        .unwrap();
    assert!(got.is_empty(), "empty selection should read back empty");
}
