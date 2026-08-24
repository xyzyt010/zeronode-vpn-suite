use crate::{
    config::{ClientConfig, ServerConfig},
    crypto::WireguardKeyMaterial,
    model::{ControlSessionLease, ServerSummary},
};
use anyhow::{bail, Context, Result};

const DEFAULT_CLIENT_DNS: &[&str] = &["1.1.1.1", "9.9.9.9"];
const DEFAULT_CLIENT_MTU: u16 = 1420;

#[derive(Clone, Debug)]
pub struct WireguardClientArtifact {
    pub server_name: String,
    pub endpoint: String,
    pub client_address: String,
    pub server_address: String,
    pub client_public_key: String,
    pub contents: String,
}

#[derive(Clone, Debug)]
pub struct WireguardServerPeerArtifact {
    pub session_id: String,
    pub client_name: String,
    pub client_public_key: String,
    pub contents: String,
}

pub fn build_client_artifact(
    client_config: &ClientConfig,
    server: &ServerSummary,
    lease: &ControlSessionLease,
) -> Result<WireguardClientArtifact> {
    if lease.server_id != server.server_id {
        bail!("lease server id does not match the selected server");
    }

    let keys = client_keys(client_config)?;
    let mut client_address = format!("{}/32", lease.reserved_client_ip);
    if let Some(ipv6) = lease.reserved_client_ipv6.as_ref() {
        client_address.push_str(&format!(", {}/128", ipv6));
    }
    let endpoint = if server.wireguard_endpoint.trim().is_empty() {
        &server.endpoint
    } else {
        &server.wireguard_endpoint
    };
    let mut allowed_ips = vec![String::from("0.0.0.0/0")];
    if lease.reserved_client_ipv6.is_some() {
        allowed_ips.push(String::from("::/0"));
    }
    let dns_servers = DEFAULT_CLIENT_DNS
        .iter()
        .map(|server| (*server).to_owned())
        .collect::<Vec<_>>();
    let contents = render_client_config(
        &keys.private_key,
        &server.public_key,
        endpoint,
        &client_address,
        &allowed_ips,
        &dns_servers,
    );

    Ok(WireguardClientArtifact {
        server_name: server.name.clone(),
        endpoint: server.endpoint.clone(),
        client_address,
        server_address: lease.server_internal_ip.clone(),
        client_public_key: keys.public_key.clone(),
        contents,
    })
}

pub fn build_server_peer_artifact(
    _server_config: &ServerConfig,
    lease: &ControlSessionLease,
) -> Result<WireguardServerPeerArtifact> {
    if lease.client_public_key.trim().is_empty() {
        bail!("client public key is missing from the control session lease");
    }

    let mut allowed_ips = vec![format!("{}/32", lease.reserved_client_ip)];
    if let Some(ipv6) = lease.reserved_client_ipv6.as_ref() {
        allowed_ips.push(format!("{}/128", ipv6));
    }
    let contents = render_server_peer_config(&lease.client_public_key, &allowed_ips);

    Ok(WireguardServerPeerArtifact {
        session_id: lease.session_id.clone(),
        client_name: lease.client_name.clone(),
        client_public_key: lease.client_public_key.clone(),
        contents,
    })
}

pub fn render_client_config(
    client_private_key: &str,
    server_public_key: &str,
    endpoint: &str,
    client_address: &str,
    allowed_ips: &[String],
    dns_servers: &[String],
) -> String {
    let allowed_ips = allowed_ips.join(", ");
    let dns_line = if dns_servers.is_empty() {
        String::new()
    } else {
        format!("DNS = {}\n", dns_servers.join(", "))
    };

    format!(
        "[Interface]\nPrivateKey = {client_private_key}\nAddress = {client_address}\nMTU = {DEFAULT_CLIENT_MTU}\nBlockUntunneledTraffic = true\n{dns_line}\n[Peer]\nPublicKey = {server_public_key}\nAllowedIPs = {allowed_ips}\nEndpoint = {endpoint}\nPersistentKeepalive = 25\n"
    )
}

pub fn render_server_peer_config(client_public_key: &str, allowed_ips: &[String]) -> String {
    let allowed_ips = allowed_ips.join(", ");
    format!(
        "[Peer]\nPublicKey = {client_public_key}\nAllowedIPs = {allowed_ips}\nPersistentKeepalive = 25\n"
    )
}

fn client_keys(client_config: &ClientConfig) -> Result<&WireguardKeyMaterial> {
    client_config
        .wireguard_keys
        .as_ref()
        .filter(|keys| keys.is_complete())
        .context("client WireGuard key material is missing")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_client_config_with_dns() {
        let rendered = render_client_config(
            "private-key",
            "server-key",
            "203.0.113.7:51820",
            "10.44.0.2/32",
            &[String::from("0.0.0.0/0")],
            &[String::from("10.44.0.1")],
        );

        assert!(rendered.contains("[Interface]"));
        assert!(rendered.contains("DNS = 10.44.0.1"));
        assert!(rendered.contains("MTU = 1420"));
        assert!(rendered.contains("BlockUntunneledTraffic = true"));
        assert!(rendered.contains("Endpoint = 203.0.113.7:51820"));
    }

    #[test]
    fn renders_server_peer_config() {
        let rendered = render_server_peer_config("client-key", &[String::from("10.44.0.2/32")]);
        assert!(rendered.contains("[Peer]"));
        assert!(rendered.contains("PublicKey = client-key"));
        assert!(rendered.contains("AllowedIPs = 10.44.0.2/32"));
    }
}
