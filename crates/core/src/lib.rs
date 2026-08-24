pub mod app_paths;
pub mod auth;
pub mod config;
pub mod control_plane;
pub mod crypto;
pub mod model;
pub mod net_info;
pub mod protocol;
pub mod setup;
pub mod wireguard;
pub mod openvpn;
pub mod outline;
pub mod pptp;
pub mod geoip;

use std::time::{SystemTime, UNIX_EPOCH};

pub const APP_NAME: &str = "ZeroNode VPN Suite";

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}
