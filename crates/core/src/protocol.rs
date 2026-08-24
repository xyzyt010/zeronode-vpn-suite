use crate::model::ControlSessionLease;
use anyhow::{Context, Result};
use bincode::{config, serde as bincode_serde};
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_PACKET_SIZE: usize = 4096;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Packet {
    DiscoveryRequest(DiscoveryRequest),
    DiscoveryResponse(DiscoveryResponse),
    AuthAttempt(AuthAttempt),
    AuthResult(AuthResult),
    StatusQuery(StatusQuery),
    StatusResponse(StatusResponse),
    Disconnect(DisconnectNotice),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiscoveryRequest {
    pub protocol_version: u16,
    pub client_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiscoveryResponse {
    pub protocol_version: u16,
    pub server: AnnouncedServer,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnnouncedServer {
    pub server_id: String,
    pub name: String,
    pub country_code: String,
    pub country_name: String,
    pub listen_port: u16,
    pub wireguard_port: u16,
    pub openvpn_port: Option<u16>,
    pub has_password: bool,
    pub public_key: String,
    pub observed_at_unix: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuthAttempt {
    pub protocol_version: u16,
    pub server_id: String,
    pub client_id: String,
    pub client_name: String,
    pub client_public_key: String,
    pub password: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuthResult {
    pub protocol_version: u16,
    pub accepted: bool,
    pub message: String,
    pub session_id: Option<String>,
    pub lease: Option<ControlSessionLease>,
    pub cooldown_until_unix: Option<u64>,
    pub locked_down: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StatusQuery {
    pub protocol_version: u16,
    pub server_id: String,
    pub client_id: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StatusResponse {
    pub protocol_version: u16,
    pub server_id: String,
    pub server_name: String,
    pub locked_down: bool,
    pub requires_password: bool,
    pub connected_peers: u32,
    pub uptime_secs: u64,
    pub active_session: Option<ControlSessionLease>,
    pub banner_message: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DisconnectNotice {
    pub protocol_version: u16,
    pub server_id: String,
    pub client_id: String,
    pub session_id: Option<String>,
}

pub fn encode_packet(packet: &Packet) -> Result<Vec<u8>> {
    bincode_serde::encode_to_vec(packet, config::standard()).context("packet encoding failed")
}

pub fn decode_packet(bytes: &[u8]) -> Result<Packet> {
    let (packet, _) = bincode_serde::decode_from_slice(bytes, config::standard())
        .context("packet decoding failed")?;
    Ok(packet)
}
