//! Android platform layer for ZeroNode VPN.
//!
//! On non-Android hosts this crate only exposes metadata. On `target_os = "android"`
//! it owns the real data plane:
//! - WireGuard via boringtun on a VpnService TUN fd
//! - Outline via embedded shadowsocks + tun2proxy(SOCKS→TUN)
//! - Tor via bundled libTor.so (expert bundle) + tun2proxy
//! - PPTP: UI only — GRE raw sockets blocked for unprivileged apps (not an aarch64 issue)

use vpn_suite_core::model::VpnProtocol;

#[derive(Clone, Debug)]
pub struct AndroidPlatformSummary {
    pub service_model: &'static str,
    pub tunnel_backend: &'static str,
    pub ui_shell: &'static str,
}

pub fn describe_client_platform() -> AndroidPlatformSummary {
    AndroidPlatformSummary {
        service_model: "Android VpnService with foreground notification and system-exempted lifecycle",
        tunnel_backend: "boringtun WireGuard + tun2proxy SOCKS (Outline/Tor) + bundled libTor.so expert bundle",
        ui_shell: "Vertical Java Activity: protocol cards, Tor, Your IP, GlobeView, Rust JNI bridge",
    }
}

/// Which data-plane engine the VpnService should start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AndroidTunnelKind {
    WireGuard,
    Outline,
    Tor,
    Pptp,
    ZeroNodeWireGuard,
}

impl AndroidTunnelKind {
    pub fn from_str_label(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "outline" | "ss" | "shadowsocks" => Self::Outline,
            "tor" => Self::Tor,
            "pptp" => Self::Pptp,
            "zeronode" | "zn" | "zeronode_wg" => Self::ZeroNodeWireGuard,
            _ => Self::WireGuard,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::WireGuard => "wireguard",
            Self::Outline => "outline",
            Self::Tor => "tor",
            Self::Pptp => "pptp",
            Self::ZeroNodeWireGuard => "zeronode",
        }
    }

    pub fn from_protocol(p: VpnProtocol) -> Self {
        match p {
            VpnProtocol::WireGuard => Self::WireGuard,
            VpnProtocol::Outline => Self::Outline,
            VpnProtocol::OpenVPN => Self::WireGuard,
            VpnProtocol::Pptp => Self::Pptp,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TunnelProgress {
    pub stage: String,
    pub fraction: f32,
    pub detail: String,
}

#[cfg(target_os = "android")]
mod wireguard;
#[cfg(target_os = "android")]
mod socks_tun;
#[cfg(target_os = "android")]
mod outline;
#[cfg(target_os = "android")]
mod tor;
#[cfg(target_os = "android")]
mod pptp;
#[cfg(target_os = "android")]
mod progress;
#[cfg(target_os = "android")]
mod protect;

#[cfg(target_os = "android")]
pub use protect::{protect_fd, set_protect_fn};
#[cfg(target_os = "android")]
pub use wireguard::{
    start_wireguard, start_wireguard_ex, stop_wireguard, is_wireguard_running,
    is_wireguard_handshake_ok, wireguard_byte_counts,
};
#[cfg(target_os = "android")]
pub use outline::{start_outline, stop_outline, is_outline_running, outline_socks_port};
#[cfg(target_os = "android")]
pub use tor::{
    prepare_tor_home, start_tor_socks, start_tor_system_tunnel, stop_tor, is_tor_running,
    tor_socks_port, tor_bootstrap_hint,
};
#[cfg(target_os = "android")]
pub use socks_tun::{start_socks_system_tunnel, stop_socks_system_tunnel, is_socks_tunnel_running};
#[cfg(target_os = "android")]
pub use pptp::{start_pptp, stop_pptp, is_pptp_supported};
#[cfg(target_os = "android")]
pub use progress::{get_progress, set_progress, clear_progress};

/// Stop data-plane engines but **keep Tor SOCKS process alive**.
///
/// Critical: VpnService always calls this before attaching a new tunnel.
/// If we killed Tor here, `start_tor_system_tunnel` would fail because
/// SOCKS was just torn down (progress also reset to 0 → UI "stuck at Tor").
#[cfg(target_os = "android")]
pub fn stop_all_tunnels() {
    let _ = stop_wireguard();
    let _ = stop_outline();
    let _ = stop_pptp();
    let _ = stop_socks_system_tunnel();
    // Intentionally NOT stop_tor() / clear_progress() — see stop_everything().
}

/// Full teardown including Tor process (user Disconnect).
#[cfg(target_os = "android")]
pub fn stop_everything() {
    let _ = stop_wireguard();
    let _ = stop_outline();
    let _ = stop_tor();
    let _ = stop_pptp();
    let _ = stop_socks_system_tunnel();
    clear_progress();
}

#[cfg(not(target_os = "android"))]
pub fn stop_all_tunnels() {}

#[cfg(not(target_os = "android"))]
pub fn stop_everything() {}
