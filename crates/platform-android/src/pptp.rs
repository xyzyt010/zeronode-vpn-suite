//! PPTP on Android.
//!
//! CPU architecture (aarch64) is not the blocker. PPTP's data channel uses
//! **GRE (IP protocol 47)**, which requires raw sockets. Unprivileged Android
//! apps only get TCP/UDP via the sandbox — no `CAP_NET_RAW` — so a proper
//! PPTP client cannot run inside a normal `VpnService` app (same reason Google
//! removed system PPTP in Android 12).
//!
//! We still expose full UI + config storage, and return a clear, non-fake error
//! if connect is attempted. Never report ACTIVE for a non-functional tunnel.

use anyhow::{bail, Result};
use crate::progress::set_progress;

pub fn is_pptp_supported() -> bool {
    false
}

pub fn pptp_support_message() -> &'static str {
    "PPTP cannot run in an unprivileged Android app: GRE (IP protocol 47) needs raw sockets, which the sandbox does not expose. \
     This is an OS permission limit, not an aarch64 limit. Use WireGuard, Outline, or Tor. \
     Root would be required for real PPTP and is not supported in this build."
}

pub fn start_pptp(_host: &str, _user: &str, _password: &str, _tun_fd: i32) -> Result<()> {
    set_progress("pptp", 0.0, pptp_support_message());
    bail!("{}", pptp_support_message())
}

pub fn stop_pptp() -> Result<()> {
    Ok(())
}
