//! Backend controller for the GUI.
//!
//! Owns an embedded `tokio::Runtime` and the currently-running
//! `hop_server::run` / `hop_client::run` task. The egui thread interacts
//! with it through synchronous methods; status events drain into a
//! channel the UI polls every frame.
//!
//! See `specs/milestones/M13-gui-backend.md §Architecture` for the
//! reasoning behind the embedded-runtime (Option A) design.

mod controller;
mod platform;

pub use self::controller::{BackendController, StatusEvent};
