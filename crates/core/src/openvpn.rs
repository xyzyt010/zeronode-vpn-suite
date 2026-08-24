use crate::{
    config::{ClientConfig, ServerConfig},
    model::{ControlSessionLease, ServerSummary},
};
use anyhow::{bail, Result};

#[derive(Clone, Debug)]
pub struct OpenvpnClientArtifact {
    pub server_name: String,
    pub endpoint: String,
    pub client_address: String,
    pub server_address: String,
    pub contents: String,
}

#[derive(Clone, Debug)]
pub struct OpenvpnServerPeerArtifact {
    pub session_id: String,
    pub client_name: String,
    pub contents: String,
}

pub fn build_client_artifact(
    _client_config: &ClientConfig,
    server: &ServerSummary,
    lease: &ControlSessionLease,
) -> Result<OpenvpnClientArtifact> {
    if lease.server_id != server.server_id {
        bail!("lease server id does not match the selected server");
    }

    let client_address = lease.reserved_client_ip.clone();
    let endpoint = if let Some(ep) = &server.openvpn_endpoint {
        ep
    } else {
        &server.endpoint
    };
    
    let contents = render_client_config(endpoint, &client_address);

    Ok(OpenvpnClientArtifact {
        server_name: server.name.clone(),
        endpoint: server.endpoint.clone(),
        client_address,
        server_address: lease.server_internal_ip.clone(),
        contents,
    })
}

pub fn build_server_peer_artifact(
    _server_config: &ServerConfig,
    lease: &ControlSessionLease,
) -> Result<OpenvpnServerPeerArtifact> {
    let mut allowed_ips = vec![format!("{}/32", lease.reserved_client_ip)];
    if let Some(ipv6) = lease.reserved_client_ipv6.as_ref() {
        allowed_ips.push(format!("{}/128", ipv6));
    }
    let contents = render_server_peer_config(&lease.client_name, &allowed_ips);

    Ok(OpenvpnServerPeerArtifact {
        session_id: lease.session_id.clone(),
        client_name: lease.client_name.clone(),
        contents,
    })
}

pub fn render_client_config(endpoint: &str, client_address: &str) -> String {
    format!(
        "client\ndev tun\nproto udp\nremote {endpoint}\nresolv-retry infinite\nnobind\npersist-key\npersist-tun\n# ifconfig {client_address} ...\n"
    )
}

pub fn render_server_peer_config(client_name: &str, allowed_ips: &[String]) -> String {
    let allowed_ips = allowed_ips.join(", ");
    format!(
        "# Client {client_name}\n# push \"route ...\"\n# AllowedIPs = {allowed_ips}\n"
    )
}
