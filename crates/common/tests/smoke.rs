//! Smoke test: verifies the test harness actually runs.
//!
//! Replaced with real tests as `hop-common` grows. Keep at least one
//! assertion so `cargo nextest` always reports a non-empty test set.

#[test]
fn harness_runs() {
    assert_eq!(2 + 2, 4);
}
