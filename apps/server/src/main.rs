use anyhow::{bail, Context, Result};
mod gui;
use clap::{Parser, Subcommand, ValueEnum};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
};
use tokio::{net::UdpSocket, signal, sync::mpsc};
use tracing::{info, warn};
use uuid::Uuid;
use vpn_suite_core::{
    app_paths::{server_paths, AppPaths},
    auth::{hash_password, verify_password, RateLimitJournal},
    config::{
        load_or_create_journal, load_or_create_server_config, load_or_create_server_runtime,
        save_journal, save_server_config, save_server_runtime, ServerBootstrapOptions,
        ServerConfig,
    },
    model::{ControlSessionLease, RuntimeSessionSnapshot, ServerRuntimeSnapshot},
    net_info::HostNetInfo,
    protocol::{
        encode_packet, AnnouncedServer, AuthResult, DisconnectNotice, DiscoveryResponse, Packet,
        StatusResponse, MAX_PACKET_SIZE, PROTOCOL_VERSION,
    },
    setup::{format_setup_report, server_setup_report},
    unix_now,
    wireguard::build_server_peer_artifact,
};
use zeroize::Zeroizing;

const SESSION_TTL_SECS: u64 = 45;

#[derive(Parser, Debug)]
#[command(author, version, about = "ZeroNode VPN server foundation")]
struct Cli {
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    country_code: Option<String>,
    #[arg(long)]
    country_name: Option<String>,
    #[arg(long)]
    port: Option<u16>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    Run,
    ShowConfig,
    SetupCheck,
    HostSetup {
        #[arg(value_enum, default_value_t = HostSetupAction::Plan)]
        action: HostSetupAction,
    },
    ShowRuntime,
    SetPassword {
        password: String,
    },
    DisablePassword,
    Unlock,
    ListBans,
    Unban {
        ip: String,
    },
    Gui,
    Dashboard,
}

#[derive(Clone, Debug, ValueEnum)]
enum HostSetupAction {
    Plan,
    Apply,
    Remove,
}

#[derive(Clone, Debug)]
struct SessionRecord {
    lease: ControlSessionLease,
    remote: SocketAddr,
    server_peer_config_path: Option<String>,
}

#[derive(Debug)]
struct ServerRuntime {
    journal: RateLimitJournal,
    started_at_unix: u64,
    sessions: BTreeMap<String, SessionRecord>,
    last_banner: Option<String>,
}

impl ServerRuntime {
    fn new(journal: RateLimitJournal) -> Self {
        Self {
            journal,
            started_at_unix: unix_now(),
            sessions: BTreeMap::new(),
            last_banner: None,
        }
    }

    fn connected_peers(&self) -> u32 {
        self.sessions.len() as u32
    }

    fn cleanup_expired_sessions(&mut self, now_unix: u64) {
        self.sessions
            .retain(|_, session| session.lease.expires_at_unix > now_unix);
    }

    fn remove_client_sessions(&mut self, client_id: &str) {
        self.sessions
            .retain(|_, session| session.lease.client_id != client_id);
    }

    fn clear_sessions(&mut self) {
        self.sessions.clear();
    }

    fn issue_lease(
        &mut self,
        config: &ServerConfig,
        client_id: &str,
        client_name: &str,
        client_public_key: &str,
        remote: SocketAddr,
        now_unix: u64,
    ) -> Result<ControlSessionLease> {
        self.cleanup_expired_sessions(now_unix);
        self.remove_client_sessions(client_id);

        let reserved_client_ip = allocate_client_ip(&config.vpn_subnet, &self.sessions)?;
        let server_internal_ip = server_internal_ip(&config.vpn_subnet)?;
        let reserved_client_ipv6 =
            allocate_client_ipv6(&config.vpn_subnet_ipv6, &self.sessions).ok();
        let server_internal_ipv6 = server_internal_ipv6(&config.vpn_subnet_ipv6).ok();
        let lease = ControlSessionLease {
            session_id: Uuid::new_v4().to_string(),
            server_id: config.server_id.clone(),
            client_id: client_id.to_owned(),
            client_name: client_name.to_owned(),
            client_public_key: client_public_key.to_owned(),
            reserved_client_ip,
            server_internal_ip,
            reserved_client_ipv6,
            server_internal_ipv6,
            authenticated_at_unix: now_unix,
            last_seen_unix: now_unix,
            expires_at_unix: now_unix + SESSION_TTL_SECS,
        };

        self.sessions.insert(
            lease.session_id.clone(),
            SessionRecord {
                lease: lease.clone(),
                remote,
                server_peer_config_path: None,
            },
        );

        Ok(lease)
    }

    fn attach_server_peer_config_path(&mut self, session_id: &str, path: String) {
        if let Some(record) = self.sessions.get_mut(session_id) {
            record.server_peer_config_path = Some(path);
        }
    }

    fn refresh_session(
        &mut self,
        remote: SocketAddr,
        client_id: Option<&str>,
        session_id: Option<&str>,
        now_unix: u64,
    ) -> Option<ControlSessionLease> {
        self.cleanup_expired_sessions(now_unix);
        let session_id = session_id?;
        let record = self.sessions.get_mut(session_id)?;
        if record.remote.ip() != remote.ip() {
            return None;
        }
        if let Some(client_id) = client_id {
            if record.lease.client_id != client_id {
                return None;
            }
        }

        record.lease.last_seen_unix = now_unix;
        record.lease.expires_at_unix = now_unix + SESSION_TTL_SECS;
        Some(record.lease.clone())
    }

    fn remove_session(
        &mut self,
        remote: SocketAddr,
        client_id: &str,
        session_id: Option<&str>,
    ) -> Option<ControlSessionLease> {
        if let Some(session_id) = session_id {
            let record = self.sessions.get(session_id)?;
            if record.remote.ip() != remote.ip() || record.lease.client_id != client_id {
                return None;
            }
            return self.sessions.remove(session_id).map(|record| record.lease);
        }

        let matching = self
            .sessions
            .iter()
            .find(|(_, record)| {
                record.remote.ip() == remote.ip() && record.lease.client_id == client_id
            })
            .map(|(session_id, _)| session_id.clone())?;

        self.sessions.remove(&matching).map(|record| record.lease)
    }

    fn snapshot(&self) -> ServerRuntimeSnapshot {
        ServerRuntimeSnapshot {
            started_at_unix: self.started_at_unix,
            connected_peers: self.connected_peers(),
            locked_down: self.journal.is_locked_down(),
            last_banner: self.last_banner.clone(),
            sessions: self
                .sessions
                .values()
                .map(|record| RuntimeSessionSnapshot {
                    session_id: record.lease.session_id.clone(),
                    client_id: record.lease.client_id.clone(),
                    client_name: record.lease.client_name.clone(),
                    client_public_key: record.lease.client_public_key.clone(),
                    remote_endpoint: record.remote.to_string(),
                    reserved_client_ip: record.lease.reserved_client_ip.clone(),
                    server_internal_ip: record.lease.server_internal_ip.clone(),
                    server_peer_config_path: record.server_peer_config_path.clone(),
                    authenticated_at_unix: record.lease.authenticated_at_unix,
                    last_seen_unix: record.lease.last_seen_unix,
                    expires_at_unix: record.lease.expires_at_unix,
                })
                .collect(),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .init();

    let cli = Cli::parse();
    let paths = server_paths()?;
    let bootstrap = ServerBootstrapOptions {
        name: cli.name,
        country_code: cli.country_code,
        country_name: cli.country_name,
        listen_port: cli.port,
    };
    let mut config = load_or_create_server_config(&paths, &bootstrap)?;
    let mut journal = load_or_create_journal(&paths)?;

    match cli.command.unwrap_or(Command::Run) {
        Command::Run => run_server(&paths, &config, journal).await,
        Command::ShowConfig => {
            let net_info = HostNetInfo::query();
            let selected_ipv4 =
                net_info.effective_selected_global_ipv4(&config.selected_global_ipv4);
            let selected_ipv6 =
                net_info.effective_selected_global_ipv6(&config.selected_global_ipv6);
            println!("Server: {}", config.name);
            println!("Server ID: {}", config.server_id);
            println!("Country: {} ({})", config.country_name, config.country_code);
            println!("Port: {}", config.listen_port);
            println!("WireGuard port: {}", config.wireguard_port);
            println!("Subnet: {}", config.vpn_subnet);
            println!(
                "Password protection: {}",
                if config.password_hash.is_some() {
                    "enabled"
                } else {
                    "disabled"
                }
            );
            println!(
                "Selected public IPv4: {}",
                if selected_ipv4.is_empty() {
                    String::from("none")
                } else {
                    selected_ipv4.join(", ")
                }
            );
            println!(
                "Selected public IPv6: {}",
                if selected_ipv6.is_empty() {
                    String::from("none")
                } else {
                    selected_ipv6.join(", ")
                }
            );
            println!("Public key: {}", config.wireguard_keys.public_key);
            println!("Config path: {}", paths.config_file.display());
            println!("State path: {}", paths.state_file.display());
            println!("Runtime path: {}", paths.runtime_file.display());
            println!("Profiles path: {}", paths.profiles_dir.display());
            println!("Event log: {}", paths.log_file.display());
            Ok(())
        }
        Command::SetupCheck => {
            let report = server_setup_report(&paths, &config);
            print!("{}", format_setup_report(&report));
            print_platform_setup(&config);
            Ok(())
        }
        Command::HostSetup { action } => {
            run_host_setup(&config, action);
            Ok(())
        }
        Command::ShowRuntime => {
            let runtime = load_or_create_server_runtime(&paths)?;
            println!("Started At: {}", runtime.started_at_unix);
            println!("Connected Peers: {}", runtime.connected_peers);
            println!("Locked Down: {}", runtime.locked_down);
            println!(
                "Last Banner: {}",
                runtime.last_banner.unwrap_or_else(|| String::from("none"))
            );
            if runtime.sessions.is_empty() {
                println!("Sessions: none");
            } else {
                println!("Sessions:");
                for session in runtime.sessions {
                    println!(
                        "  {} {} {} {} {}",
                        session.session_id,
                        session.client_name,
                        session.remote_endpoint,
                        session.reserved_client_ip,
                        session
                            .server_peer_config_path
                            .unwrap_or_else(|| String::from("no-artifact"))
                    );
                }
            }
            Ok(())
        }
        Command::SetPassword { password } => {
            config.password_hash = Some(hash_password(Zeroizing::new(password))?);
            save_server_config(&paths, &config)?;
            println!("Password protection enabled.");
            Ok(())
        }
        Command::DisablePassword => {
            config.password_hash = None;
            save_server_config(&paths, &config)?;
            println!("Password protection disabled.");
            Ok(())
        }
        Command::Unlock => {
            journal.unlock();
            save_journal(&paths, &journal)?;
            println!("Server lockdown cleared.");
            Ok(())
        }
        Command::ListBans => {
            let bans = journal.list_bans();
            if bans.is_empty() {
                println!("No banned IPs.");
            } else {
                for ip in bans {
                    println!("{ip}");
                }
            }
            Ok(())
        }
        Command::Unban { ip } => {
            if journal.unban(&ip) {
                save_journal(&paths, &journal)?;
                println!("Removed {ip} from the denylist.");
            } else {
                println!("{ip} was not present in the denylist.");
            }
            Ok(())
        }
        Command::Gui | Command::Dashboard => {
            let runtime_snapshot = load_or_create_server_runtime(&paths)?;
            gui::run_dashboard(paths, config, journal, runtime_snapshot)
        }
    }
}

#[derive(Clone)]
struct BoundServerSocket {
    label: String,
    socket: Arc<UdpSocket>,
    public: bool,
}

struct IncomingPacket {
    socket_index: usize,
    bytes: Vec<u8>,
    remote: SocketAddr,
}

async fn run_server(
    paths: &AppPaths,
    config: &ServerConfig,
    journal: RateLimitJournal,
) -> Result<()> {
    let bound_sockets = bind_server_sockets(config).await?;
    let public_bindings = bound_sockets
        .iter()
        .filter(|socket| socket.public)
        .map(|socket| socket.label.clone())
        .collect::<Vec<_>>();
    let loopback_bindings = bound_sockets
        .iter()
        .filter(|socket| !socket.public)
        .map(|socket| socket.label.clone())
        .collect::<Vec<_>>();

    let mut runtime = ServerRuntime::new(journal);
    persist_runtime(paths, &runtime)?;

    info!(
        "{} listening on {}",
        config.name,
        public_bindings.join(", ")
    );
    if !loopback_bindings.is_empty() {
        info!("local admin sockets on {}", loopback_bindings.join(", "));
    }
    info!("public key {}", config.wireguard_keys.public_key);
    info!("config stored at {}", paths.config_file.display());
    append_event(
        paths,
        &format!(
            "server-start name={} port={} public-bindings={} loopback-bindings={} public-key={}",
            config.name,
            config.listen_port,
            public_bindings.join(","),
            loopback_bindings.join(","),
            config.wireguard_keys.public_key
        ),
    )?;
    apply_platform_server_tunnel(paths, config)?;

    let (packet_tx, mut packet_rx) = mpsc::unbounded_channel();
    for (socket_index, bound) in bound_sockets.iter().enumerate() {
        let socket = Arc::clone(&bound.socket);
        let packet_tx = packet_tx.clone();
        tokio::spawn(async move {
            let mut buffer = [0_u8; MAX_PACKET_SIZE];
            loop {
                match socket.recv_from(&mut buffer).await {
                    Ok((bytes_read, remote)) => {
                        if packet_tx
                            .send(IncomingPacket {
                                socket_index,
                                bytes: buffer[..bytes_read].to_vec(),
                                remote,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
    }
    drop(packet_tx);

    loop {
        tokio::select! {
            _ = signal::ctrl_c() => {
                info!("shutdown requested");
                append_event(paths, "server-stop reason=ctrl-c")?;
                break;
            }
            packet = packet_rx.recv() => {
                let Some(packet) = packet else {
                    break;
                };
                let socket = &bound_sockets[packet.socket_index].socket;
                if let Err(error) = handle_packet(
                    socket,
                    config,
                    paths,
                    &mut runtime,
                    &packet.bytes,
                    packet.remote,
                ).await {
                    warn!("packet handling failed for {}: {error:#}", packet.remote);
                    append_event(paths, &format!("packet-error remote={} error={error:#}", packet.remote))?;
                }
            }
        }
    }

    Ok(())
}

async fn bind_server_sockets(config: &ServerConfig) -> Result<Vec<BoundServerSocket>> {
    let net_info = HostNetInfo::query();
    if !net_info.has_global_connectivity() {
        bail!("no global/public IPv4 or IPv6 address detected; local-only hosting is disabled");
    }

    let selected_ipv4 = net_info.effective_selected_global_ipv4(&config.selected_global_ipv4);
    let selected_ipv6 = net_info.effective_selected_global_ipv6(&config.selected_global_ipv6);
    if selected_ipv4.is_empty() && selected_ipv6.is_empty() {
        bail!("no selected public IPv4 or IPv6 address is currently available for server hosting");
    }

    let mut sockets = Vec::new();
    for ip in &selected_ipv4 {
        let address = SocketAddr::new(ip.parse::<IpAddr>()?, config.listen_port);
        sockets.push(BoundServerSocket {
            label: address.to_string(),
            socket: Arc::new(bind_udp_socket(address)?),
            public: true,
        });
    }
    for ip in &selected_ipv6 {
        let address = SocketAddr::new(ip.parse::<IpAddr>()?, config.listen_port);
        sockets.push(BoundServerSocket {
            label: address.to_string(),
            socket: Arc::new(bind_udp_socket(address)?),
            public: true,
        });
    }

    for address in [
        SocketAddr::new(Ipv4Addr::LOCALHOST.into(), config.listen_port),
        SocketAddr::new(Ipv6Addr::LOCALHOST.into(), config.listen_port),
    ] {
        if let Ok(socket) = bind_udp_socket(address) {
            sockets.push(BoundServerSocket {
                label: address.to_string(),
                socket: Arc::new(socket),
                public: false,
            });
        }
    }

    if !sockets.iter().any(|socket| socket.public) {
        bail!("failed to bind any selected public control socket");
    }

    Ok(sockets)
}

fn bind_udp_socket(address: SocketAddr) -> Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};

    let domain = if address.is_ipv4() {
        Domain::IPV4
    } else {
        Domain::IPV6
    };
    let socket = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))
        .with_context(|| format!("failed to create UDP socket for {address}"))?;
    if address.is_ipv6() {
        socket
            .set_only_v6(true)
            .with_context(|| format!("failed to set IPV6_V6ONLY on {address}"))?;
    }
    socket
        .set_nonblocking(true)
        .with_context(|| format!("failed to set nonblocking mode on {address}"))?;
    socket
        .bind(&address.into())
        .with_context(|| format!("failed to bind UDP socket on {address}"))?;

    let std_socket: std::net::UdpSocket = socket.into();
    UdpSocket::from_std(std_socket)
        .with_context(|| format!("failed to create tokio UDP socket for {address}"))
}

async fn handle_packet(
    socket: &UdpSocket,
    config: &ServerConfig,
    paths: &AppPaths,
    runtime: &mut ServerRuntime,
    bytes: &[u8],
    remote: SocketAddr,
) -> Result<()> {
    let packet = match vpn_suite_core::protocol::decode_packet(bytes) {
        Ok(packet) => packet,
        Err(error) => {
            warn!("discarded undecodable packet from {remote}: {error:#}");
            return Ok(());
        }
    };

    let now_unix = unix_now();
    runtime.cleanup_expired_sessions(now_unix);

    match packet {
        Packet::DiscoveryRequest(request) => {
            if request.protocol_version != PROTOCOL_VERSION || runtime.journal.is_locked_down() {
                persist_runtime(paths, runtime)?;
                return Ok(());
            }

            let response = Packet::DiscoveryResponse(DiscoveryResponse {
                protocol_version: PROTOCOL_VERSION,
                server: AnnouncedServer {
                    server_id: config.server_id.clone(),
                    name: config.name.clone(),
                    country_code: config.country_code.clone(),
                    country_name: config.country_name.clone(),
                    listen_port: config.listen_port,
                    wireguard_port: config.wireguard_port,
                    openvpn_port: config.openvpn_port,
                    has_password: config.password_hash.is_some(),
                    public_key: config.wireguard_keys.public_key.clone(),
                    observed_at_unix: now_unix,
                },
            });

            send_packet(socket, remote, &response).await?;
        }
        Packet::AuthAttempt(attempt) => {
            if attempt.protocol_version != PROTOCOL_VERSION || attempt.server_id != config.server_id
            {
                persist_runtime(paths, runtime)?;
                return Ok(());
            }

            let ip = remote.ip().to_string();
            let access = runtime.journal.check(&ip, now_unix);
            if !access.allowed && access.silently_drop {
                save_journal(paths, &runtime.journal)?;
                persist_runtime(paths, runtime)?;
                return Ok(());
            }

            let accepted = if let Some(hash) = config.password_hash.as_deref() {
                match attempt.password {
                    Some(password) => verify_password(Zeroizing::new(password), hash)?,
                    None => false,
                }
            } else {
                true
            };

            if accepted {
                runtime.journal.register_success(&ip);
                let lease = runtime.issue_lease(
                    config,
                    &attempt.client_id,
                    &attempt.client_name,
                    &attempt.client_public_key,
                    remote,
                    now_unix,
                )?;
                let peer_artifact = build_server_peer_artifact(config, &lease)?;
                let peer_path = write_server_peer_artifact(paths, &peer_artifact)?;
                runtime.attach_server_peer_config_path(&lease.session_id, peer_path.clone());
                apply_platform_server_peer(paths, config, &lease)?;
                runtime.last_banner = Some(format!(
                    "{} authenticated from {}",
                    attempt.client_name,
                    remote.ip()
                ));
                save_journal(paths, &runtime.journal)?;
                append_event(
                    paths,
                    &format!(
                        "auth-accepted client={} ip={} session={} reserved-ip={} client-key={} peer-artifact={}",
                        attempt.client_name,
                        remote.ip(),
                        lease.session_id,
                        lease.reserved_client_ip,
                        lease.client_public_key,
                        peer_path
                    ),
                )?;

                let packet = Packet::AuthResult(AuthResult {
                    protocol_version: PROTOCOL_VERSION,
                    accepted: true,
                    message: format!("Control session accepted for {}", config.name),
                    session_id: Some(lease.session_id.clone()),
                    lease: Some(lease),
                    cooldown_until_unix: None,
                    locked_down: false,
                });
                send_packet(socket, remote, &packet).await?;
            } else {
                let outcome = runtime.journal.register_failure(&ip, now_unix);
                if outcome.locked_down {
                    runtime.clear_sessions();
                    runtime.last_banner = Some(String::from(
                        "Server locked down after repeated authentication failures.",
                    ));
                    append_event(
                        paths,
                        &format!(
                            "lockdown-triggered remote={} offenders={}",
                            remote.ip(),
                            runtime.journal.list_bans().join(",")
                        ),
                    )?;
                } else if let Some(until) = outcome.cooldown_until_unix {
                    append_event(
                        paths,
                        &format!(
                            "auth-cooldown remote={} until={} after repeated failures",
                            remote.ip(),
                            until
                        ),
                    )?;
                } else {
                    append_event(
                        paths,
                        &format!(
                            "auth-rejected remote={} client={}",
                            remote.ip(),
                            attempt.client_name
                        ),
                    )?;
                }

                save_journal(paths, &runtime.journal)?;

                let message = if outcome.locked_down {
                    String::from("Server locked down after repeated authentication failures.")
                } else if let Some(until) = outcome.cooldown_until_unix {
                    format!("Too many failed attempts. Access blocked until {until}.")
                } else {
                    String::from("Password rejected.")
                };

                let packet = Packet::AuthResult(AuthResult {
                    protocol_version: PROTOCOL_VERSION,
                    accepted: false,
                    message,
                    session_id: None,
                    lease: None,
                    cooldown_until_unix: outcome.cooldown_until_unix,
                    locked_down: outcome.locked_down,
                });
                send_packet(socket, remote, &packet).await?;
            }
        }
        Packet::StatusQuery(query) => {
            if query.protocol_version != PROTOCOL_VERSION || query.server_id != config.server_id {
                persist_runtime(paths, runtime)?;
                return Ok(());
            }
            if !remote.ip().is_loopback() && query.session_id.is_none() {
                persist_runtime(paths, runtime)?;
                return Ok(());
            }

            let active_session = if remote.ip().is_loopback() && query.session_id.is_none() {
                None
            } else {
                runtime.refresh_session(
                    remote,
                    query.client_id.as_deref(),
                    query.session_id.as_deref(),
                    now_unix,
                )
            };

            let banner_message = if remote.ip().is_loopback() && query.session_id.is_none() {
                runtime.last_banner.clone()
            } else if active_session.is_none() {
                Some(String::from("No active session matched the request."))
            } else {
                runtime.last_banner.clone()
            };

            let response = Packet::StatusResponse(StatusResponse {
                protocol_version: PROTOCOL_VERSION,
                server_id: config.server_id.clone(),
                server_name: config.name.clone(),
                locked_down: runtime.journal.is_locked_down(),
                requires_password: config.password_hash.is_some(),
                connected_peers: runtime.connected_peers(),
                uptime_secs: now_unix.saturating_sub(runtime.started_at_unix),
                active_session,
                banner_message,
            });
            send_packet(socket, remote, &response).await?;
        }
        Packet::Disconnect(DisconnectNotice {
            protocol_version,
            server_id,
            client_id,
            session_id,
        }) => {
            if protocol_version != PROTOCOL_VERSION || server_id != config.server_id {
                persist_runtime(paths, runtime)?;
                return Ok(());
            }

            if let Some(lease) = runtime.remove_session(remote, &client_id, session_id.as_deref()) {
                remove_platform_server_peer(paths, config, &lease)?;
                runtime.last_banner = Some(format!(
                    "{} disconnected from {}",
                    lease.client_name,
                    remote.ip()
                ));
                append_event(
                    paths,
                    &format!(
                        "disconnect client={} session={} remote={}",
                        lease.client_name, lease.session_id, remote
                    ),
                )?;
            }
        }
        Packet::DiscoveryResponse(_) | Packet::AuthResult(_) | Packet::StatusResponse(_) => {}
    }

    persist_runtime(paths, runtime)?;
    Ok(())
}

async fn send_packet(socket: &UdpSocket, remote: SocketAddr, packet: &Packet) -> Result<()> {
    let payload = encode_packet(packet)?;
    socket
        .send_to(&payload, remote)
        .await
        .with_context(|| format!("failed to send packet to {remote}"))?;
    Ok(())
}

fn persist_runtime(paths: &AppPaths, runtime: &ServerRuntime) -> Result<()> {
    save_server_runtime(paths, &runtime.snapshot())
}

fn write_server_peer_artifact(
    paths: &AppPaths,
    artifact: &vpn_suite_core::wireguard::WireguardServerPeerArtifact,
) -> Result<String> {
    let path = paths
        .profiles_dir
        .join(format!("server-peer-{}.conf", artifact.session_id));
    fs::write(&path, &artifact.contents)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path.display().to_string())
}

fn append_event(paths: &AppPaths, event: &str) -> Result<()> {
    let timestamp = unix_now();
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.log_file)
        .with_context(|| format!("failed to open {}", paths.log_file.display()))?;
    writeln!(file, "[{timestamp}] {event}")
        .with_context(|| format!("failed to append {}", paths.log_file.display()))
}

#[cfg(target_os = "linux")]
fn apply_platform_server_tunnel(paths: &AppPaths, config: &ServerConfig) -> Result<()> {
    for check in vpn_platform_linux::apply_server_tunnel(config) {
        append_event(
            paths,
            &format!(
                "linux-tunnel-setup status={:?} name={} detail={}",
                check.status, check.name, check.detail
            ),
        )?;
        print_platform_check(&check);
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn apply_platform_server_tunnel(paths: &AppPaths, config: &ServerConfig) -> Result<()> {
    for check in vpn_platform_windows::apply_server_tunnel(paths, config) {
        append_event(
            paths,
            &format!(
                "windows-tunnel-setup status={:?} name={} detail={}",
                check.status, check.name, check.detail
            ),
        )?;
        print_platform_check(&check);
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn apply_platform_server_tunnel(_paths: &AppPaths, _config: &ServerConfig) -> Result<()> {
    Ok(())
}

#[cfg(target_os = "linux")]
fn apply_platform_server_peer(
    paths: &AppPaths,
    _config: &ServerConfig,
    lease: &ControlSessionLease,
) -> Result<()> {
    let check = vpn_platform_linux::apply_server_peer(lease);
    append_event(
        paths,
        &format!(
            "linux-peer-apply status={:?} name={} detail={}",
            check.status, check.name, check.detail
        ),
    )?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn apply_platform_server_peer(
    paths: &AppPaths,
    config: &ServerConfig,
    lease: &ControlSessionLease,
) -> Result<()> {
    let check = vpn_platform_windows::apply_server_peer(paths, config, lease);
    append_event(
        paths,
        &format!(
            "windows-peer-apply status={:?} name={} detail={}",
            check.status, check.name, check.detail
        ),
    )?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn remove_platform_server_peer(
    paths: &AppPaths,
    _config: &ServerConfig,
    lease: &ControlSessionLease,
) -> Result<()> {
    let check = vpn_platform_linux::remove_server_peer(lease);
    append_event(
        paths,
        &format!(
            "linux-peer-remove status={:?} name={} detail={}",
            check.status, check.name, check.detail
        ),
    )?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn remove_platform_server_peer(
    paths: &AppPaths,
    config: &ServerConfig,
    lease: &ControlSessionLease,
) -> Result<()> {
    let check = vpn_platform_windows::remove_server_peer(paths, config, lease);
    append_event(
        paths,
        &format!(
            "windows-peer-remove status={:?} name={} detail={}",
            check.status, check.name, check.detail
        ),
    )?;
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn apply_platform_server_peer(
    _paths: &AppPaths,
    _config: &ServerConfig,
    _lease: &ControlSessionLease,
) -> Result<()> {
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn remove_platform_server_peer(
    _paths: &AppPaths,
    _config: &ServerConfig,
    _lease: &ControlSessionLease,
) -> Result<()> {
    Ok(())
}

#[cfg(target_os = "linux")]
fn run_host_setup(config: &ServerConfig, action: HostSetupAction) {
    match action {
        HostSetupAction::Plan => {
            println!("Host setup plan:");
            for (index, step) in vpn_platform_linux::server_host_setup_plan(config)
                .iter()
                .enumerate()
            {
                println!("{}. {step}", index + 1);
            }
            println!("Run `sudo vpn-server host-setup apply` to apply these changes.");
        }
        HostSetupAction::Apply => {
            println!("Applying Linux host setup...");
            for check in vpn_platform_linux::apply_server_host_setup(config) {
                print_platform_check(&check);
            }
        }
        HostSetupAction::Remove => {
            println!("Removing Linux host setup...");
            for check in vpn_platform_linux::remove_server_host_setup(config) {
                print_platform_check(&check);
            }
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn run_host_setup(_config: &ServerConfig, _action: HostSetupAction) {
    println!("Host setup automation is currently implemented for Linux server hosts.");
}

#[cfg(target_os = "windows")]
fn run_host_setup(config: &ServerConfig, action: HostSetupAction) {
    match action {
        HostSetupAction::Plan => {
            println!("Host setup plan:");
            for (index, step) in vpn_platform_windows::server_host_setup_plan(config)
                .iter()
                .enumerate()
            {
                println!("{}. {step}", index + 1);
            }
            println!(
                "Run `vpn-server.exe host-setup apply` from an elevated Administrator terminal."
            );
        }
        HostSetupAction::Apply => {
            println!("Applying Windows host setup...");
            for check in vpn_platform_windows::apply_server_host_setup(config) {
                print_platform_check(&check);
            }
        }
        HostSetupAction::Remove => {
            println!("Removing Windows host setup...");
            for check in vpn_platform_windows::remove_server_host_setup(config) {
                print_platform_check(&check);
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn print_platform_setup(config: &ServerConfig) {
    let summary = vpn_platform_linux::describe_server_platform(config);
    println!("Platform Service: {}", summary.service_model);
    println!("Platform Tunnel: {}", summary.tunnel_backend);
    println!("Platform Firewall: {}", summary.firewall_strategy);
    for check in vpn_platform_linux::server_platform_checks(config) {
        print_platform_check(&check);
    }
    print_platform_check(&vpn_platform_linux::server_tunnel_status());
}

#[cfg(target_os = "windows")]
fn print_platform_setup(config: &ServerConfig) {
    let summary = vpn_platform_windows::describe_server_platform(config);
    println!("Platform Service: {}", summary.service_model);
    println!("Platform Tunnel: {}", summary.tunnel_backend);
    println!("Platform Firewall: {}", summary.firewall_strategy);
    for check in vpn_platform_windows::wireguard_asset_checks() {
        print_platform_check(&check);
    }
    print_platform_check(&vpn_platform_windows::server_tunnel_status());
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn print_platform_setup(_config: &ServerConfig) {
    println!("Platform Service: unsupported server platform");
    println!("Platform Tunnel: unsupported server platform");
    println!("Platform Firewall: unsupported server platform");
}

fn print_platform_check(check: &vpn_suite_core::setup::SetupCheck) {
    let status = match &check.status {
        vpn_suite_core::setup::SetupStatus::Pass => "PASS",
        vpn_suite_core::setup::SetupStatus::Warn => "WARN",
        vpn_suite_core::setup::SetupStatus::Fail => "FAIL",
    };
    println!("[{status}] {} - {}", check.name, check.detail);
    if let Some(remedy) = check.remedy.as_deref() {
        println!("      remedy: {remedy}");
    }
}

fn server_internal_ip(subnet: &str) -> Result<String> {
    let prefix = subnet_prefix(subnet)?;
    Ok(format!("{}.{}.{}.1", prefix[0], prefix[1], prefix[2]))
}

fn allocate_client_ip(subnet: &str, sessions: &BTreeMap<String, SessionRecord>) -> Result<String> {
    let prefix = subnet_prefix(subnet)?;
    let used = sessions
        .values()
        .map(|record| record.lease.reserved_client_ip.clone())
        .collect::<BTreeSet<_>>();

    for host in 2..=254_u8 {
        let candidate = format!("{}.{}.{}.{}", prefix[0], prefix[1], prefix[2], host);
        if !used.contains(&candidate) {
            return Ok(candidate);
        }
    }

    bail!("no free addresses left in {subnet}")
}

fn subnet_prefix(subnet: &str) -> Result<[u8; 3]> {
    let (base, mask) = subnet
        .split_once('/')
        .context("vpn subnet must be provided in CIDR format")?;
    if mask != "24" {
        bail!("only /24 subnets are supported in the current control-plane build");
    }

    let octets = base
        .parse::<Ipv4Addr>()
        .with_context(|| format!("invalid subnet base address {base}"))?
        .octets();
    Ok([octets[0], octets[1], octets[2]])
}

fn server_internal_ipv6(subnet: &str) -> Result<String> {
    let seg = get_ipv6_subnet_segments(subnet)?;
    Ok(format!(
        "{:x}:{:x}:{:x}:{:x}::1",
        seg[0], seg[1], seg[2], seg[3]
    ))
}

fn allocate_client_ipv6(
    subnet: &str,
    sessions: &BTreeMap<String, SessionRecord>,
) -> Result<String> {
    let seg = get_ipv6_subnet_segments(subnet)?;
    let used = sessions
        .values()
        .filter_map(|record| record.lease.reserved_client_ipv6.clone())
        .collect::<BTreeSet<_>>();

    for host in 2..=254 {
        let candidate = format!(
            "{:x}:{:x}:{:x}:{:x}::{:x}",
            seg[0], seg[1], seg[2], seg[3], host
        );
        if !used.contains(&candidate) {
            return Ok(candidate);
        }
    }

    bail!("no free IPv6 addresses left in {subnet}")
}

fn get_ipv6_subnet_segments(subnet: &str) -> Result<[u16; 8]> {
    let (base, mask) = subnet
        .split_once('/')
        .context("vpn subnet ipv6 must be provided in CIDR format")?;
    if mask != "64" {
        bail!("only /64 IPv6 subnets are supported in the current control-plane build");
    }

    let addr: Ipv6Addr = base
        .trim()
        .parse()
        .with_context(|| format!("invalid IPv6 base address '{base}'"))?;
    Ok(addr.segments())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_first_free_address_in_subnet() {
        let mut sessions = BTreeMap::new();
        sessions.insert(
            String::from("session-1"),
            SessionRecord {
                lease: ControlSessionLease {
                    session_id: String::from("session-1"),
                    server_id: String::from("server"),
                    client_id: String::from("client-1"),
                    client_name: String::from("Client 1"),
                    client_public_key: String::from("client-pub"),
                    reserved_client_ip: String::from("10.44.0.2"),
                    server_internal_ip: String::from("10.44.0.1"),
                    reserved_client_ipv6: None,
                    server_internal_ipv6: None,
                    authenticated_at_unix: 0,
                    last_seen_unix: 0,
                    expires_at_unix: 100,
                },
                remote: "127.0.0.1:9999".parse().unwrap(),
                server_peer_config_path: None,
            },
        );

        let next = allocate_client_ip("10.44.0.0/24", &sessions).unwrap();
        assert_eq!(next, "10.44.0.3");
    }

    #[test]
    fn rejects_non_24_subnets() {
        assert!(subnet_prefix("10.44.0.0/16").is_err());
    }

    #[test]
    fn allocates_ipv6_addresses_correctly() {
        let sessions = BTreeMap::new();
        let next = allocate_client_ipv6("fd44::/64", &sessions).unwrap();
        assert_eq!(next, "fd44:0:0:0::2");

        let internal = server_internal_ipv6("fd44::/64").unwrap();
        assert_eq!(internal, "fd44:0:0:0::1");
    }
}
