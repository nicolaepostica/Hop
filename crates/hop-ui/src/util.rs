//! Small utility helpers shared across views.
//!
//! - [`system_hostname`] — reliable hostname lookup (replaces the old
//!   env-var fallback that broke on macOS GUI apps).
//! - [`lan_ipv4`] — first non-loopback IPv4 address for a LAN peer,
//!   shown in the Server view under "Listening on".

use std::net::Ipv4Addr;

/// Return the OS hostname via `uname`/`GetComputerNameExW`.
///
/// Falls back to `"hop-host"` only if the call itself fails or the
/// value isn't valid UTF-8 (both exceedingly rare on real systems).
#[must_use]
pub fn system_hostname() -> String {
    gethostname::gethostname()
        .into_string()
        .unwrap_or_else(|_| "hop-host".into())
}

/// Return the first non-loopback IPv4 address attached to this host,
/// if any.
///
/// Useful for the Server view: the user binds to `0.0.0.0:25900`, but
/// the *client* needs a concrete, reachable address. On hosts with no
/// network (CI sandboxes, etc.) this returns `None` and the UI simply
/// omits the "Reachable at" line.
#[must_use]
pub fn lan_ipv4() -> Option<Ipv4Addr> {
    match local_ip_address::local_ip() {
        Ok(std::net::IpAddr::V4(v4)) if !v4.is_loopback() => Some(v4),
        _ => None,
    }
}
