//! VpnService.protect(fd) bridge — keeps control-plane sockets off the TUN.
//!
//! WireGuard UDP (and any native socket that must reach the real endpoint) must
//! call this when the app package is *not* excluded via addDisallowedApplication.

use std::sync::OnceLock;

type ProtectFn = fn(i32) -> bool;

static PROTECT: OnceLock<ProtectFn> = OnceLock::new();

/// Register the Java-side protect callback (once per process).
pub fn set_protect_fn(f: ProtectFn) {
    let _ = PROTECT.set(f);
}

/// Protect a socket fd so it bypasses the VPN TUN (uses underlying network).
/// Returns false if the callback is missing or protect fails — callers should
/// still work when the app package is addDisallowedApplication.
pub fn protect_fd(fd: i32) -> bool {
    if fd < 0 {
        return false;
    }
    match PROTECT.get() {
        Some(f) => f(fd),
        None => false,
    }
}
