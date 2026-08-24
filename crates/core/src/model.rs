use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControlSessionLease {
    pub session_id: String,
    pub server_id: String,
    pub client_id: String,
    pub client_name: String,
    #[serde(default)]
    pub client_public_key: String,
    pub reserved_client_ip: String,
    pub server_internal_ip: String,
    #[serde(default)]
    pub reserved_client_ipv6: Option<String>,
    #[serde(default)]
    pub server_internal_ipv6: Option<String>,
    pub authenticated_at_unix: u64,
    pub last_seen_unix: u64,
    pub expires_at_unix: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerSummary {
    pub server_id: String,
    pub name: String,
    pub country_code: String,
    pub country_name: String,
    pub endpoint: String,
    #[serde(default)]
    pub wireguard_endpoint: String,
    #[serde(default)]
    pub openvpn_endpoint: Option<String>,
    pub masked_endpoint: String,
    pub listen_port: u16,
    pub has_password: bool,
    pub last_seen_unix: u64,
    pub public_key: String,
    pub online: bool,
    pub cooldown_until_unix: Option<u64>,
    pub last_message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionPhase {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Cooldown,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VpnProtocol {
    WireGuard,
    OpenVPN,
    /// Legacy PPTP (MS-CHAPv2). Weak crypto — compatibility only.
    Pptp,
    /// Outline access keys (Shadowsocks-based).
    Outline,
}

impl VpnProtocol {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::WireGuard => "WireGuard",
            Self::OpenVPN => "OpenVPN",
            Self::Pptp => "PPTP",
            Self::Outline => "Outline",
        }
    }

    /// Prefix used in `ActiveConnection.server_id` for imported profiles.
    pub fn profile_id_prefix(self) -> &'static str {
        match self {
            Self::WireGuard => "wg_",
            Self::OpenVPN => "ovpn_",
            Self::Pptp => "pptp_",
            Self::Outline => "outline_",
        }
    }
}

/// Right-pane protocol selector (imported profiles; not Tor).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VpnUiProtocol {
    #[default]
    OpenVPN,
    WireGuard,
    Pptp,
    Outline,
}

impl VpnUiProtocol {
    pub const ALL: [VpnUiProtocol; 4] = [
        VpnUiProtocol::OpenVPN,
        VpnUiProtocol::WireGuard,
        VpnUiProtocol::Pptp,
        VpnUiProtocol::Outline,
    ];

    pub fn display_name(self) -> &'static str {
        match self {
            Self::OpenVPN => "OpenVPN",
            Self::WireGuard => "WireGuard",
            Self::Pptp => "PPTP",
            Self::Outline => "Outline",
        }
    }

    pub fn as_pref(self) -> &'static str {
        match self {
            Self::OpenVPN => "openvpn",
            Self::WireGuard => "wireguard",
            Self::Pptp => "pptp",
            Self::Outline => "outline",
        }
    }

    pub fn from_pref(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "wireguard" | "wg" => Self::WireGuard,
            "pptp" => Self::Pptp,
            "outline" | "ss" => Self::Outline,
            _ => Self::OpenVPN,
        }
    }

    pub fn to_vpn_protocol(self) -> VpnProtocol {
        match self {
            Self::OpenVPN => VpnProtocol::OpenVPN,
            Self::WireGuard => VpnProtocol::WireGuard,
            Self::Pptp => VpnProtocol::Pptp,
            Self::Outline => VpnProtocol::Outline,
        }
    }
}

/// Full GeoIP resolution of a Tor exit node, parsed from the ip-api.com
/// `/json/` response. All fields are optional because the upstream API may
/// omit some (e.g. city for some exit relays) or return an error status.
///
/// This is stored separately from `ActiveConnection.country_code` (which only
/// carries the ISO-2 code used by the globe renderer / flag lookup) so the
/// right-pane Tor card can render the full human-readable location details
/// without touching the globe animation logic.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TorExitInfo {
    /// The exit node's public IP address (ip-api "query" field). Usually IPv4.
    pub ip: String,
    /// Public IPv6 address when dual-stack is available (empty if none / unreachable).
    #[serde(default)]
    pub ipv6: String,
    /// ISO-3166 alpha-2 country code, e.g. "DE" (ip-api "countryCode").
    pub country_code: String,
    /// Human-readable country name, e.g. "Germany" (ip-api "country").
    pub country: String,
    /// ISO 3166-2 region code, e.g. "BY" (ip-api "region").
    pub region_code: String,
    /// Human-readable region/state name, e.g. "Bavaria" (ip-api "regionName").
    pub region: String,
    /// City name, e.g. "Munich" (ip-api "city").
    pub city: String,
    /// Postal/ZIP code (ip-api "zip"). Often empty outside the US.
    pub zip: String,
    /// Latitude (ip-api "lat").
    pub lat: f64,
    /// Longitude (ip-api "lon").
    pub lon: f64,
    /// IANA timezone, e.g. "Europe/Berlin" (ip-api "timezone").
    pub timezone: String,
    /// ISP / ASN organization name (ip-api "isp").
    pub isp: String,
    /// Organization (ip-api "org"). Often overlaps with isp.
    pub org: String,
    /// AS number and name, e.g. "AS3320 Deutsche Telekom AG" (ip-api "as").
    pub as_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActiveConnection {
    pub server_id: String,
    pub server_name: String,
    pub endpoint: String,
    pub protocol: VpnProtocol,
    pub phase: ConnectionPhase,
    pub connected_at_unix: Option<u64>,
    pub attempt_count: u32,
    pub session_id: Option<String>,
    pub reserved_client_ip: Option<String>,
    pub server_internal_ip: Option<String>,
    #[serde(default)]
    pub tunnel_profile_path: Option<String>,
    /// ISO-3166 country code for the active connection's exit location.
    /// Populated for normal servers from the discovery list, and for the Tor
    /// transport from the GeoIP lookup of the exit node (e.g. "DE", "US").
    /// Stored separately from server_name so the globe renderer and UI flag
    /// lookups can read it directly instead of parsing a display string.
    #[serde(default)]
    pub country_code: Option<String>,
    /// Full GeoIP details for the Tor exit node (city, region, ISP, lat/lon,
    /// etc.). Only populated for the Tor transport. The globe animation does
    /// NOT use this — it still reads `country_code` → centroid — this is
    /// purely for the right-pane Tor card detail display.
    #[serde(default)]
    pub tor_exit_info: Option<TorExitInfo>,
    pub cooldown_until_unix: Option<u64>,
    pub last_status_unix: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ClientSnapshot {
    pub servers: Vec<ServerSummary>,
    pub active_connection: Option<ActiveConnection>,
    pub notice: Option<String>,
    pub last_refresh_unix: u64,
    #[serde(default)]
    pub local_server_active: bool,
    #[serde(default)]
    pub local_server_peers: u32,
    #[serde(default)]
    pub local_server_port: u16,
    #[serde(default)]
    pub local_server_public_key: String,
    #[serde(default)]
    pub local_server_selected_ipv4: Vec<String>,
    #[serde(default)]
    pub local_server_selected_ipv6: Vec<String>,
    /// The user's real (non-Tor) public IP and full GeoIP details, refreshed
    /// on startup and every 5 minutes by the backend. Always populated via a
    /// direct ip-api.com lookup (NOT through Tor) so the right-pane "IP
    /// Details" card has something to display the moment the UI opens, even
    /// before the user has clicked any "Connect" button.
    ///
    /// `None` until the first lookup succeeds — typically <2s after startup.
    #[serde(default)]
    pub local_ip_info: Option<TorExitInfo>,
    /// Unix timestamp of the most recent local-IP refresh, surfaced in the
    /// UI as an "updated 12s ago" hint next to the IP Details card.
    #[serde(default)]
    pub local_ip_refresh_unix: u64,
    /// True once we've scheduled the connection attempt to add the system
    /// route (`0.0.0.0/0`) via the tun2proxy/Wintun adapter. This requires
    /// Administrator; we let the user opt in separately instead of forcing
    /// elevation at connect time (which used to silently kill the process
    /// when UAC was declined).
    #[serde(default)]
    pub tor_system_route_active: bool,
    /// Local Tor SOCKS5 listener port while a Tor session is active.
    /// Surfaced so the UI can show `127.0.0.1:<port>` even after `endpoint`
    /// is rewritten to the exit node's public IP for the GeoIP card.
    #[serde(default)]
    pub tor_socks_port: Option<u16>,
    /// Real tunnel operation progress in `[0.0, 1.0]`. Updated by the backend
    /// as connect/disconnect stages complete (not a time-based fake bar).
    #[serde(default)]
    pub op_progress: f32,
    /// Human label for the current progress stage (e.g. "Starting SOCKS…").
    #[serde(default)]
    pub op_progress_label: Option<String>,
    /// `"connect"`, `"disconnect"`, or empty when idle.
    #[serde(default)]
    pub op_progress_kind: Option<String>,
    /// Bumped on every successful public-IP refresh so the UI can force a
    /// globe pan/tilt to the current location even when the IP is unchanged.
    #[serde(default)]
    pub globe_pan_token: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ClientState {
    pub cooldowns: BTreeMap<String, CooldownEntry>,
    pub last_connected_server_id: Option<String>,
    #[serde(default)]
    pub last_tunnel_profile_path: Option<String>,
    #[serde(default)]
    pub last_active_connection: Option<ActiveConnection>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CooldownEntry {
    pub server_id: String,
    pub endpoint: String,
    pub until_unix: u64,
    pub reason: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ServerRuntimeSnapshot {
    pub started_at_unix: u64,
    pub connected_peers: u32,
    pub locked_down: bool,
    pub last_banner: Option<String>,
    pub sessions: Vec<RuntimeSessionSnapshot>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeSessionSnapshot {
    pub session_id: String,
    pub client_id: String,
    pub client_name: String,
    pub client_public_key: String,
    pub remote_endpoint: String,
    pub reserved_client_ip: String,
    pub server_internal_ip: String,
    #[serde(default)]
    pub server_peer_config_path: Option<String>,
    pub authenticated_at_unix: u64,
    pub last_seen_unix: u64,
    pub expires_at_unix: u64,
}
