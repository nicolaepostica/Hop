//! Keep-alive timing primitives for the server/client event loops.
//!
//! The wire protocol has no built-in heartbeating; both sides must
//! inject [`KeepAlive`](hop_protocol::Message::KeepAlive)
//! frames on a timer and track when they last saw traffic from the
//! peer. This module provides a small tracker that owns those two
//! concerns without dictating how the surrounding `select!` loop is
//! organised.

use std::time::Duration;

use tokio::time::{self, Instant, Interval, MissedTickBehavior};

/// How often to emit a keep-alive and check for peer timeout.
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(3);

/// Number of consecutive missed keep-alives before we declare the peer
/// dead. Three gives ~9 seconds of silence — enough tolerance for a
/// briefly stalled peer or a hiccupy Wi-Fi link, but noticeable to a
/// human.
pub const KEEPALIVE_MAX_MISSES: u32 = 3;

/// Silence window after which the peer is considered gone.
pub const KEEPALIVE_TIMEOUT: Duration =
    Duration::from_secs(KEEPALIVE_INTERVAL.as_secs() * KEEPALIVE_MAX_MISSES as u64);

/// Tracks inbound activity and schedules outbound keep-alives.
#[derive(Debug)]
pub struct KeepAliveTracker {
    last_seen: Instant,
    interval: Interval,
}

impl KeepAliveTracker {
    /// Create a fresh tracker. The tracker considers the peer "just
    /// seen" at construction time, so it will not fire a spurious
    /// timeout before the first `mark_seen` call.
    #[must_use]
    pub fn new() -> Self {
        let mut interval = time::interval(KEEPALIVE_INTERVAL);
        // Skip ticks we cannot keep up with instead of firing them
        // back-to-back — the exact cadence does not matter, only that
        // the average is close to the interval.
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        Self {
            last_seen: Instant::now(),
            interval,
        }
    }

    /// Record that we just received something from the peer.
    pub fn mark_seen(&mut self) {
        self.last_seen = Instant::now();
    }

    /// Has the peer been silent for longer than [`KEEPALIVE_TIMEOUT`]?
    #[must_use]
    pub fn is_timed_out(&self) -> bool {
        self.last_seen.elapsed() >= KEEPALIVE_TIMEOUT
    }

    /// Wait until the next keep-alive tick. Cancellation-safe.
    pub async fn tick(&mut self) -> Instant {
        self.interval.tick().await
    }
}

impl Default for KeepAliveTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::advance;

    #[tokio::test(start_paused = true)]
    async fn tracker_times_out_after_three_missed_intervals() {
        let tracker = KeepAliveTracker::new();
        assert!(!tracker.is_timed_out());

        // Just under the limit: still alive.
        advance(KEEPALIVE_TIMEOUT - Duration::from_millis(1)).await;
        assert!(!tracker.is_timed_out());

        // Cross the limit: now timed out.
        advance(Duration::from_millis(2)).await;
        assert!(tracker.is_timed_out());
    }

    #[tokio::test(start_paused = true)]
    async fn mark_seen_resets_the_clock() {
        let mut tracker = KeepAliveTracker::new();
        advance(KEEPALIVE_TIMEOUT - Duration::from_millis(100)).await;
        tracker.mark_seen();
        advance(KEEPALIVE_TIMEOUT - Duration::from_millis(100)).await;
        assert!(!tracker.is_timed_out());
    }

    #[tokio::test(start_paused = true)]
    async fn tick_fires_at_the_interval() {
        let mut tracker = KeepAliveTracker::new();
        // First tick is immediate by tokio::time::interval's convention.
        let _ = tracker.tick().await;
        let start = Instant::now();
        let _ = tracker.tick().await;
        assert!(start.elapsed() >= KEEPALIVE_INTERVAL - Duration::from_millis(1));
    }
}
