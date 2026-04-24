//! Platform-backend selection for the embedded runtime.
//!
//! Mirrors the cascade in `bins/hops/src/main.rs::backend::run_server`
//! but packaged as a cloneable enum so the controller can hold the
//! selected backend across Start/Stop cycles without re-initialising
//! the X11 / libei / macOS / Windows handle each time.
//!
//! The [`PlatformScreen`](hop_platform::PlatformScreen) trait uses
//! AFIT and is not object-safe, so we use an enum (monomorphized
//! `match`) instead of `Arc<dyn PlatformScreen>`.

use std::sync::Arc;

use hop_client::{ClientConfig, ClientError};
use hop_platform::MockScreen;
use hop_server::{ServerConfig, ServerError};
use tokio_util::sync::CancellationToken;

/// A concrete platform backend, selected once at app startup.
///
/// Cheap to clone — each variant wraps the screen in [`Arc`].
#[derive(Clone)]
pub enum PlatformBackend {
    /// libei / Wayland portal backend.
    #[cfg(target_os = "linux")]
    Ei(Arc<hop_platform_ei::EiScreen>),
    /// Linux / *BSD X11 backend.
    #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd"))]
    X11(Arc<hop_platform_x11::X11Screen>),
    /// macOS Carbon backend.
    #[cfg(target_os = "macos")]
    MacOs(Arc<hop_platform_macos::MacOsScreen>),
    /// Windows backend.
    #[cfg(windows)]
    Windows(Arc<hop_platform_windows::WindowsScreen>),
    /// In-memory noop backend — used in tests and when no native
    /// backend can be opened. Input will not be injected.
    Mock(Arc<MockScreen>),
}

impl PlatformBackend {
    /// Open the best available native backend, falling back to
    /// [`MockScreen`] if nothing initialises.
    ///
    /// Logs the choice via `tracing`. Never panics.
    #[must_use]
    pub fn select() -> Self {
        #[cfg(target_os = "linux")]
        {
            match hop_platform_ei::EiScreen::try_open() {
                Ok(screen) => {
                    tracing::info!("using libei platform backend");
                    return Self::Ei(Arc::new(screen));
                }
                Err(err) => {
                    tracing::debug!(error = %err, "libei backend unavailable; trying X11");
                }
            }
        }
        #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd"))]
        {
            match hop_platform_x11::X11Screen::open(None) {
                Ok(screen) => {
                    tracing::info!("using X11 platform backend");
                    return Self::X11(Arc::new(screen));
                }
                Err(err) => {
                    tracing::warn!(error = %err, "X11 unavailable; falling back to MockScreen");
                }
            }
        }
        #[cfg(target_os = "macos")]
        {
            match hop_platform_macos::MacOsScreen::try_open() {
                Ok(screen) => {
                    tracing::info!("using macOS platform backend");
                    return Self::MacOs(Arc::new(screen));
                }
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        "macOS backend unavailable; falling back to MockScreen"
                    );
                }
            }
        }
        #[cfg(windows)]
        {
            match hop_platform_windows::WindowsScreen::try_open() {
                Ok(screen) => {
                    tracing::info!("using Windows platform backend");
                    return Self::Windows(Arc::new(screen));
                }
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        "Windows backend unavailable; falling back to MockScreen"
                    );
                }
            }
        }
        tracing::warn!(
            "no native platform backend available; using MockScreen (input will not be injected)"
        );
        Self::Mock(Arc::new(MockScreen::default_stub()))
    }

    /// Short label identifying which backend is active. Useful for
    /// status bars and debug logs.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            #[cfg(target_os = "linux")]
            Self::Ei(_) => "libei",
            #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd"))]
            Self::X11(_) => "X11",
            #[cfg(target_os = "macos")]
            Self::MacOs(_) => "macOS",
            #[cfg(windows)]
            Self::Windows(_) => "Windows",
            Self::Mock(_) => "mock",
        }
    }

    /// `true` when input will not actually be injected (`MockScreen` fallback).
    #[must_use]
    pub fn is_mock(&self) -> bool {
        matches!(self, Self::Mock(_))
    }

    /// Spawn the server event loop with this backend's screen.
    pub async fn run_server(
        &self,
        cfg: ServerConfig,
        shutdown: CancellationToken,
    ) -> Result<(), ServerError> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Ei(s) => hop_server::run(cfg, Arc::clone(s), shutdown).await,
            #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd"))]
            Self::X11(s) => hop_server::run(cfg, Arc::clone(s), shutdown).await,
            #[cfg(target_os = "macos")]
            Self::MacOs(s) => hop_server::run(cfg, Arc::clone(s), shutdown).await,
            #[cfg(windows)]
            Self::Windows(s) => hop_server::run(cfg, Arc::clone(s), shutdown).await,
            Self::Mock(s) => hop_server::run(cfg, Arc::clone(s), shutdown).await,
        }
    }

    /// Spawn the client session with this backend's screen.
    pub async fn run_client(
        &self,
        cfg: ClientConfig,
        shutdown: CancellationToken,
    ) -> Result<(), ClientError> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Ei(s) => hop_client::run(cfg, Arc::clone(s), shutdown).await,
            #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd"))]
            Self::X11(s) => hop_client::run(cfg, Arc::clone(s), shutdown).await,
            #[cfg(target_os = "macos")]
            Self::MacOs(s) => hop_client::run(cfg, Arc::clone(s), shutdown).await,
            #[cfg(windows)]
            Self::Windows(s) => hop_client::run(cfg, Arc::clone(s), shutdown).await,
            Self::Mock(s) => hop_client::run(cfg, Arc::clone(s), shutdown).await,
        }
    }
}
