use crate::{
    config::{ClientConfig, DEFAULT_CONTROL_PORT},
    model::ServerSummary,
    protocol::{
        decode_packet, encode_packet, AuthAttempt, AuthResult, DisconnectNotice, DiscoveryRequest,
        DiscoveryResponse, Packet, StatusQuery, StatusResponse, MAX_PACKET_SIZE, PROTOCOL_VERSION,
    },
};
use anyhow::{anyhow, Context, Result};
use std::{
    collections::BTreeMap,
    fs,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};
use tokio::{net::UdpSocket, task::JoinHandle, time};

pub async fn discover_servers(config: &ClientConfig) -> Result<Vec<ServerSummary>> {
    discover_servers_with_hosts(config, &[], true).await
}

pub async fn discover_servers_on_hosts(
    config: &ClientConfig,
    hosts: &[String],
) -> Result<Vec<ServerSummary>> {
    discover_servers_with_hosts(config, hosts, false).await
}

async fn discover_servers_with_hosts(
    config: &ClientConfig,
    extra_hosts: &[String],
    include_broadcast: bool,
) -> Result<Vec<ServerSummary>> {
    let payload = encode_packet(&Packet::DiscoveryRequest(DiscoveryRequest {
        protocol_version: PROTOCOL_VERSION,
        client_id: config.client_id.clone(),
    }))?;

    let mut resolved_hosts = Vec::new();
    let mut needs_ipv4_socket = include_broadcast;
    let mut needs_ipv6_socket = false;

    for host in config
        .known_hosts
        .iter()
        .chain(extra_hosts.iter())
        .filter(|host| !host.trim().is_empty())
    {
        if looks_like_wireguard_public_key(host) {
            continue;
        }
        let resolved = match resolve_socket_addrs(host).await {
            Ok(addresses) => addresses,
            Err(_) => continue,
        };
        for address in resolved {
            if address.is_ipv4() {
                needs_ipv4_socket = true;
            } else {
                needs_ipv6_socket = true;
            }
            resolved_hosts.push(address);
        }
    }

    let ipv4_socket = if needs_ipv4_socket {
        Some(bind_client_socket_for_family(false)?)
    } else {
        None
    };
    let ipv6_socket = if needs_ipv6_socket {
        Some(bind_client_socket_for_family(true)?)
    } else {
        None
    };

    if include_broadcast {
        let Some(socket) = ipv4_socket.as_ref() else {
            return Err(anyhow!(
                "could not open an IPv4 UDP socket for LAN discovery"
            ));
        };
        socket
            .send_to(
                &payload,
                SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), DEFAULT_CONTROL_PORT),
            )
            .await?;
    }

    for address in resolved_hosts {
        match address {
            SocketAddr::V4(_) => {
                if let Some(socket) = ipv4_socket.as_ref() {
                    let _ = socket.send_to(&payload, address).await;
                }
            }
            SocketAddr::V6(_) => {
                if let Some(socket) = ipv6_socket.as_ref() {
                    let _ = socket.send_to(&payload, address).await;
                }
            }
        }
    }

    let deadline = time::Instant::now() + Duration::from_millis(900);
    let ipv4_task =
        ipv4_socket.map(|socket| tokio::spawn(collect_discovery_responses(socket, deadline)));
    let ipv6_task =
        ipv6_socket.map(|socket| tokio::spawn(collect_discovery_responses(socket, deadline)));

    let mut responses = BTreeMap::new();
    merge_discovery_results(&mut responses, ipv4_task).await?;
    merge_discovery_results(&mut responses, ipv6_task).await?;

    Ok(responses.into_values().collect())
}

pub async fn attempt_auth(
    config: &ClientConfig,
    endpoint: &str,
    server_id: &str,
    password: Option<String>,
) -> Result<AuthResult> {
    let address = resolve_first_socket_addr(endpoint).await?;
    let socket = bind_client_socket_for_address(address)?;

    let payload = encode_packet(&Packet::AuthAttempt(AuthAttempt {
        protocol_version: PROTOCOL_VERSION,
        server_id: server_id.to_owned(),
        client_id: config.client_id.clone(),
        client_name: config.display_name.clone(),
        client_public_key: client_public_key(config)?.to_owned(),
        password,
    }))?;

    let mut buffer = [0_u8; MAX_PACKET_SIZE];
    let max_attempts = 3;
    let attempt_timeout = Duration::from_millis(3000);

    for attempt in 1..=max_attempts {
        socket.send_to(&payload, address).await?;
        match time::timeout(attempt_timeout, socket.recv_from(&mut buffer)).await {
            Ok(Ok((bytes_read, _))) => match decode_packet(&buffer[..bytes_read])? {
                Packet::AuthResult(result) => return Ok(result),
                _ => {
                    return Err(anyhow!(
                        "unexpected authentication response from {endpoint}"
                    ))
                }
            },
            Ok(Err(e)) => {
                return Err(anyhow!("network error during auth to {endpoint}: {e}"));
            }
            Err(_) => {
                if attempt == max_attempts {
                    return Err(anyhow!(
                        "timed out waiting for authentication response from {endpoint} after {max_attempts} attempts"
                    ));
                }
            }
        }
    }

    Err(anyhow!(
        "authentication to {endpoint} failed after {max_attempts} attempts"
    ))
}

pub async fn query_server_status(
    endpoint: &str,
    server_id: &str,
    client_id: Option<String>,
    session_id: Option<String>,
) -> Result<StatusResponse> {
    let address = resolve_first_socket_addr(endpoint).await?;
    let socket = bind_client_socket_for_address(address)?;

    let payload = encode_packet(&Packet::StatusQuery(StatusQuery {
        protocol_version: PROTOCOL_VERSION,
        server_id: server_id.to_owned(),
        client_id,
        session_id,
    }))?;

    socket.send_to(&payload, address).await?;
    let mut buffer = [0_u8; MAX_PACKET_SIZE];
    let (bytes_read, _) = time::timeout(Duration::from_millis(1200), socket.recv_from(&mut buffer))
        .await
        .map_err(|_| anyhow!("timed out waiting for session status from {endpoint}"))??;

    match decode_packet(&buffer[..bytes_read])? {
        Packet::StatusResponse(result) => Ok(result),
        _ => Err(anyhow!("unexpected status response from {endpoint}")),
    }
}

pub async fn send_disconnect_notice(
    endpoint: &str,
    server_id: &str,
    client_id: &str,
    session_id: &str,
) -> Result<()> {
    let address = resolve_first_socket_addr(endpoint).await?;
    let socket = bind_client_socket_for_address(address)?;

    let payload = encode_packet(&Packet::Disconnect(DisconnectNotice {
        protocol_version: PROTOCOL_VERSION,
        server_id: server_id.to_owned(),
        client_id: client_id.to_owned(),
        session_id: Some(session_id.to_owned()),
    }))?;

    socket.send_to(&payload, address).await?;
    Ok(())
}

pub fn write_client_tunnel_artifact(
    profiles_dir: &std::path::Path,
    server_id: &str,
    contents: &str,
) -> Result<String> {
    let path = profiles_dir.join(format!("client-tunnel-{server_id}.conf"));
    fs::write(&path, contents).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path.display().to_string())
}

pub fn client_public_key(config: &ClientConfig) -> Result<&str> {
    config
        .wireguard_keys
        .as_ref()
        .filter(|keys| keys.is_complete())
        .map(|keys| keys.public_key.as_str())
        .context("client WireGuard public key is unavailable")
}

pub async fn resolve_first_socket_addr(value: &str) -> Result<SocketAddr> {
    resolve_socket_addrs(value)
        .await?
        .into_iter()
        .next()
        .with_context(|| format!("could not resolve an endpoint for {value}"))
}

async fn resolve_socket_addrs(value: &str) -> Result<Vec<SocketAddr>> {
    if let Ok(address) = value.parse::<SocketAddr>() {
        return Ok(vec![address]);
    }

    if let Ok(address) = value.parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(address, DEFAULT_CONTROL_PORT)]);
    }

    let candidate = if value.starts_with('[') && value.contains(']') {
        value.to_owned()
    } else if value.split(':').count() > 2 {
        format!(
            "[{}]:{}",
            value.trim().trim_matches(|c| c == '[' || c == ']'),
            DEFAULT_CONTROL_PORT
        )
    } else if value.contains(':') {
        value.to_owned()
    } else {
        format!("{value}:{DEFAULT_CONTROL_PORT}")
    };

    let resolved = tokio::net::lookup_host(&candidate)
        .await
        .with_context(|| format!("failed to resolve {candidate}"))?
        .collect::<Vec<_>>();
    if resolved.is_empty() {
        return Err(anyhow!("resolver returned no addresses for {candidate}"));
    }
    Ok(resolved)
}

fn bind_client_socket_for_address(address: SocketAddr) -> Result<UdpSocket> {
    bind_client_socket_for_family(address.is_ipv6())
}

fn bind_client_socket_for_family(ipv6: bool) -> Result<UdpSocket> {
    let std_socket = if ipv6 {
        std::net::UdpSocket::bind((Ipv6Addr::UNSPECIFIED, 0))?
    } else {
        std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?
    };
    std_socket.set_nonblocking(true)?;
    if !ipv6 {
        std_socket.set_broadcast(true)?;
    }
    UdpSocket::from_std(std_socket).map_err(Into::into)
}

async fn collect_discovery_responses(
    socket: UdpSocket,
    deadline: time::Instant,
) -> Result<Vec<(DiscoveryResponse, SocketAddr)>> {
    let mut responses = Vec::new();
    let mut buffer = [0_u8; MAX_PACKET_SIZE];

    loop {
        let now = time::Instant::now();
        if now >= deadline {
            break;
        }

        let timeout = deadline - now;
        match time::timeout(timeout, socket.recv_from(&mut buffer)).await {
            Ok(Ok((bytes_read, remote))) => {
                if let Ok(Packet::DiscoveryResponse(response)) =
                    decode_packet(&buffer[..bytes_read])
                {
                    if response.protocol_version == PROTOCOL_VERSION {
                        responses.push((response, remote));
                    }
                }
            }
            Ok(Err(error)) => return Err(error.into()),
            Err(_) => break,
        }
    }

    Ok(responses)
}

async fn merge_discovery_results(
    responses: &mut BTreeMap<String, ServerSummary>,
    task: Option<JoinHandle<Result<Vec<(DiscoveryResponse, SocketAddr)>>>>,
) -> Result<()> {
    let Some(task) = task else {
        return Ok(());
    };

    for (response, remote) in task.await.context("discovery task join failed")?? {
        let endpoint = format_endpoint(remote.ip(), response.server.listen_port);
        let wireguard_endpoint = format_endpoint(remote.ip(), response.server.wireguard_port);
        let openvpn_endpoint = response.server.openvpn_port.map(|p| format_endpoint(remote.ip(), p));
        responses.insert(
            response.server.server_id.clone(),
            ServerSummary {
                server_id: response.server.server_id,
                name: response.server.name,
                country_code: response.server.country_code,
                country_name: response.server.country_name,
                endpoint: endpoint.clone(),
                wireguard_endpoint,
                openvpn_endpoint,
                masked_endpoint: mask_endpoint(&endpoint),
                listen_port: response.server.listen_port,
                has_password: response.server.has_password,
                last_seen_unix: response.server.observed_at_unix,
                public_key: response.server.public_key,
                online: true,
                cooldown_until_unix: None,
                last_message: None,
            },
        );
    }

    Ok(())
}

fn format_endpoint(ip: IpAddr, port: u16) -> String {
    SocketAddr::new(ip, port).to_string()
}

fn looks_like_wireguard_public_key(value: &str) -> bool {
    let trimmed = value.trim();
    let base = trimmed
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(trimmed);
    let candidate = base.split(':').next().unwrap_or(base);
    let len_ok = (42..=60).contains(&candidate.len());
    let charset_ok = candidate
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '/' || ch == '+' || ch == '=');
    len_ok
        && charset_ok
        && (candidate.contains('/') || candidate.contains('+') || candidate.ends_with('='))
}

fn mask_endpoint(endpoint: &str) -> String {
    match endpoint.parse::<SocketAddr>() {
        Ok(SocketAddr::V4(address)) => {
            let octets = address.ip().octets();
            format!("{}.{}.•••.•••", octets[0], octets[1])
        }
        Ok(SocketAddr::V6(_)) => String::from("[••••:••••:••••:••••]:••••"),
        Err(_) => String::from("•••.•••.•••.•••"),
    }
}
