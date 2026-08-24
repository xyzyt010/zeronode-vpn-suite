use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use vpn_suite_core::{
    app_paths::client_paths,
    config::{
        load_or_create_client_config, load_or_create_client_state, save_client_config,
        save_client_state,
    },
    control_plane::{
        attempt_auth, client_public_key, discover_servers, discover_servers_on_hosts,
        query_server_status, send_disconnect_notice, write_client_tunnel_artifact,
    },
    model::{ActiveConnection, ClientState, ConnectionPhase, ControlSessionLease, ServerSummary, VpnProtocol},
    setup::{client_setup_report, format_setup_report},
    unix_now,
    wireguard::build_client_artifact,
};

#[derive(Parser, Debug)]
#[command(author, version, about = "ZeroNode VPN client CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    ShowConfig,
    SetupCheck,
    AddHost {
        host: String,
    },
    Discover {
        #[arg(long = "host")]
        hosts: Vec<String>,
    },
    Connect {
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        server_id: Option<String>,
        #[arg(long)]
        password: Option<String>,
    },
    Status,
    TunnelApply,
    TunnelStatus,
    TunnelRemove,
    Disconnect,
    Cooldowns,
}

#[tokio::main]
async fn main() -> Result<()> {
    #[cfg(target_os = "windows")]
    if let Some(exit_code) = vpn_platform_windows::run_wireguard_tunnel_service_if_requested()? {
        std::process::exit(exit_code);
    }

    let cli = Cli::parse();
    let paths = client_paths()?;
    let mut config = load_or_create_client_config(&paths)?;
    let mut state = load_or_create_client_state(&paths)?;
    prune_cooldowns(&mut state);

    match cli.command {
        Command::ShowConfig => {
            println!("Client ID: {}", config.client_id);
            println!("Display Name: {}", config.display_name);
            println!("Known Hosts: {}", config.known_hosts.len());
            for host in &config.known_hosts {
                println!("  {host}");
            }
            println!("Public Key: {}", client_public_key(&config)?);
            println!("Config Path: {}", paths.config_file.display());
            println!("State Path: {}", paths.state_file.display());
            println!("Profiles Path: {}", paths.profiles_dir.display());
            if let Some(profile) = state.last_tunnel_profile_path.as_deref() {
                println!("Last Tunnel Profile: {profile}");
            }
            if let Some(active) = state.last_active_connection.as_ref() {
                println!(
                    "Cached Active Connection: {} {} {}",
                    active.server_name,
                    active.endpoint,
                    phase_label(&active.phase)
                );
            }
        }
        Command::SetupCheck => {
            let report = client_setup_report(&paths, &config);
            print!("{}", format_setup_report(&report));
        }
        Command::AddHost { host } => {
            let host = host.trim().to_owned();
            if host.is_empty() {
                bail!("host cannot be empty");
            }
            if !config.known_hosts.iter().any(|known| known == &host) {
                config.known_hosts.push(host.clone());
                save_client_config(&paths, &config)?;
            }
            println!("Saved host: {host}");
        }
        Command::Discover { hosts } => {
            let servers = if hosts.is_empty() {
                discover_servers(&config).await?
            } else {
                discover_servers_on_hosts(&config, &hosts).await?
            };
            print_servers(&servers);
        }
        Command::Connect {
            host,
            server_id,
            password,
        } => {
            let server =
                resolve_server(&config, &state, host.as_deref(), server_id.as_deref()).await?;
            if let Some(entry) = state.cooldowns.get(&server.server_id) {
                let now = unix_now();
                if entry.until_unix > now {
                    bail!(
                        "Access Denied — Too many failed attempts. Try again in {}.",
                        format_remaining(entry.until_unix.saturating_sub(now))
                    );
                }
            }

            let result =
                attempt_auth(&config, &server.endpoint, &server.server_id, password).await?;
            if result.accepted {
                let lease = result
                    .lease
                    .as_ref()
                    .context("server accepted the session without returning a lease")?;
                let artifact = build_client_artifact(&config, &server, lease)?;
                let profile_path = write_client_tunnel_artifact(
                    &paths.profiles_dir,
                    &server.server_id,
                    &artifact.contents,
                )?;

                state.last_connected_server_id = Some(server.server_id.clone());
                state.last_tunnel_profile_path = Some(profile_path.clone());
                state.last_active_connection = Some(connection_from_lease(
                    &server,
                    lease,
                    Some(profile_path.clone()),
                ));
                state.cooldowns.remove(&server.server_id);
                save_client_state(&paths, &state)?;

                println!("Connected to {}", server.name);
                println!("Control Endpoint: {}", server.endpoint);
                println!("WireGuard Endpoint: {}", server.wireguard_endpoint);
                println!("Session ID: {}", lease.session_id);
                println!("Reserved Client IP: {}", lease.reserved_client_ip);
                println!("Server Internal IP: {}", lease.server_internal_ip);
                println!("Tunnel Profile: {profile_path}");
            } else if let Some(until) = result.cooldown_until_unix {
                state.cooldowns.insert(
                    server.server_id.clone(),
                    vpn_suite_core::model::CooldownEntry {
                        server_id: server.server_id.clone(),
                        endpoint: server.endpoint.clone(),
                        until_unix: until,
                        reason: result.message.clone(),
                    },
                );
                save_client_state(&paths, &state)?;
                bail!("{}", result.message);
            } else {
                bail!("{}", result.message);
            }
        }
        Command::Status => {
            let active = state
                .last_active_connection
                .clone()
                .context("no cached active connection; connect first")?;

            if active.phase == ConnectionPhase::Cooldown {
                if let Some(until) = active.cooldown_until_unix {
                    println!(
                        "Cooldown active for {}. {} remaining.",
                        active.server_name,
                        format_remaining(until.saturating_sub(unix_now()))
                    );
                }
                return Ok(());
            }

            let session_id = active
                .session_id
                .clone()
                .context("cached active connection does not have a session id")?;
            let status = query_server_status(
                &active.endpoint,
                &active.server_id,
                Some(config.client_id.clone()),
                Some(session_id),
            )
            .await?;

            if status.locked_down {
                let mut updated = active.clone();
                updated.phase = ConnectionPhase::Error;
                updated.last_status_unix = Some(unix_now());
                state.last_active_connection = Some(updated);
                save_client_state(&paths, &state)?;
                println!("SERVER LOCKED DOWN — Contact the server owner.");
                return Ok(());
            }

            if let Some(lease) = status.active_session.as_ref() {
                let profile_path = active.tunnel_profile_path.clone();
                state.last_active_connection = Some(connection_from_lease_from_active(
                    &active,
                    &status.server_name,
                    lease,
                    profile_path,
                ));
                save_client_state(&paths, &state)?;

                println!("Connected to {}", status.server_name);
                println!("Endpoint: {}", active.endpoint);
                println!("Session ID: {}", lease.session_id);
                println!("Reserved Client IP: {}", lease.reserved_client_ip);
                println!("Server Internal IP: {}", lease.server_internal_ip);
                println!("Connected Peers: {}", status.connected_peers);
                if let Some(profile) = state
                    .last_active_connection
                    .as_ref()
                    .and_then(|conn| conn.tunnel_profile_path.as_ref())
                {
                    println!("Tunnel Profile: {profile}");
                }
            } else {
                state.last_active_connection = None;
                save_client_state(&paths, &state)?;
                println!("No active session matched the cached connection.");
            }
        }
        Command::TunnelApply => {
            apply_tunnel(&config, &state).await?;
        }
        Command::TunnelStatus => {
            print_tunnel_status();
        }
        Command::TunnelRemove => {
            remove_tunnel();
        }
        Command::Disconnect => {
            let active = state
                .last_active_connection
                .clone()
                .context("no cached active connection to disconnect")?;

            if let Some(session_id) = active.session_id.as_deref() {
                send_disconnect_notice(
                    &active.endpoint,
                    &active.server_id,
                    &config.client_id,
                    session_id,
                )
                .await?;
            }

            state.last_active_connection = None;
            save_client_state(&paths, &state)?;
            println!("Disconnected from {}", active.server_name);
        }
        Command::Cooldowns => {
            if state.cooldowns.is_empty() {
                println!("No client-side cooldowns recorded.");
            } else {
                let now = unix_now();
                for entry in state.cooldowns.values() {
                    println!(
                        "{}  {}  {} remaining",
                        entry.server_id,
                        entry.endpoint,
                        format_remaining(entry.until_unix.saturating_sub(now))
                    );
                }
            }
        }
    }

    Ok(())
}

async fn apply_tunnel(
    config: &vpn_suite_core::config::ClientConfig,
    state: &ClientState,
) -> Result<()> {
    let active = state
        .last_active_connection
        .clone()
        .context("no cached active connection; connect first")?;
    let session_id = active
        .session_id
        .clone()
        .context("cached active connection does not have a session id")?;
    let status = query_server_status(
        &active.endpoint,
        &active.server_id,
        Some(config.client_id.clone()),
        Some(session_id),
    )
    .await?;
    let lease = status
        .active_session
        .context("server did not return an active session; reconnect first")?;
    let servers = discover_servers_on_hosts(config, std::slice::from_ref(&active.endpoint)).await?;
    let server = servers
        .into_iter()
        .find(|server| server.server_id == active.server_id)
        .context("could not rediscover the active server for tunnel setup")?;

    apply_platform_tunnel(
        config,
        &server,
        &lease,
        active.tunnel_profile_path.as_deref(),
    );
    Ok(())
}

#[cfg(target_os = "linux")]
fn apply_platform_tunnel(
    config: &vpn_suite_core::config::ClientConfig,
    server: &ServerSummary,
    lease: &ControlSessionLease,
    _profile_path: Option<&str>,
) {
    for check in vpn_platform_linux::apply_client_tunnel(config, server, lease) {
        print_setup_check(&check);
    }
}

#[cfg(target_os = "windows")]
fn apply_platform_tunnel(
    _config: &vpn_suite_core::config::ClientConfig,
    _server: &ServerSummary,
    _lease: &ControlSessionLease,
    profile_path: Option<&str>,
) {
    for check in vpn_platform_windows::apply_client_tunnel_service(profile_path) {
        print_setup_check(&check);
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn apply_platform_tunnel(
    _config: &vpn_suite_core::config::ClientConfig,
    _server: &ServerSummary,
    _lease: &ControlSessionLease,
    _profile_path: Option<&str>,
) {
    println!("Client tunnel apply is currently implemented for Linux clients.");
}

#[cfg(target_os = "linux")]
fn print_tunnel_status() {
    print_setup_check(&vpn_platform_linux::client_tunnel_status());
}

#[cfg(not(target_os = "linux"))]
#[cfg(not(target_os = "windows"))]
fn print_tunnel_status() {
    println!("Client tunnel status is currently implemented for Linux clients.");
}

#[cfg(target_os = "windows")]
fn print_tunnel_status() {
    print_setup_check(&vpn_platform_windows::client_tunnel_service_status());
}

#[cfg(target_os = "linux")]
fn remove_tunnel() {
    print_setup_check(&vpn_platform_linux::remove_client_tunnel());
}

#[cfg(not(target_os = "linux"))]
#[cfg(not(target_os = "windows"))]
fn remove_tunnel() {
    println!("Client tunnel removal is currently implemented for Linux clients.");
}

#[cfg(target_os = "windows")]
fn remove_tunnel() {
    for check in vpn_platform_windows::remove_client_tunnel_service() {
        print_setup_check(&check);
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn print_setup_check(check: &vpn_suite_core::setup::SetupCheck) {
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

async fn resolve_server(
    config: &vpn_suite_core::config::ClientConfig,
    state: &ClientState,
    host: Option<&str>,
    server_id: Option<&str>,
) -> Result<ServerSummary> {
    let servers = if let Some(host) = host {
        discover_servers_on_hosts(config, &[host.to_owned()]).await?
    } else {
        discover_servers(config).await?
    };

    if servers.is_empty() {
        bail!("no reachable ZeroNode servers matched the current request");
    }

    if let Some(server_id) = server_id {
        return servers
            .into_iter()
            .find(|server| server.server_id == server_id)
            .with_context(|| format!("no discovered server matched {server_id}"));
    }

    if servers.len() == 1 {
        return servers
            .into_iter()
            .next()
            .context("server list unexpectedly empty after length check");
    }

    if let Some(last) = state.last_connected_server_id.as_deref() {
        if let Some(server) = servers.iter().find(|server| server.server_id == last) {
            return Ok(server.clone());
        }
    }

    let options = servers
        .iter()
        .map(|server| format!("{} ({})", server.name, server.server_id))
        .collect::<Vec<_>>()
        .join(", ");
    Err(anyhow!(
        "multiple servers matched; rerun with --server-id. Candidates: {options}"
    ))
}

fn connection_from_lease(
    server: &ServerSummary,
    lease: &ControlSessionLease,
    tunnel_profile_path: Option<String>,
) -> ActiveConnection {
    ActiveConnection {
        server_id: server.server_id.clone(),
        server_name: server.name.clone(),
        endpoint: server.endpoint.clone(),
        protocol: VpnProtocol::WireGuard,
        phase: ConnectionPhase::Connected,
        connected_at_unix: Some(unix_now()),
        attempt_count: 0,
        session_id: Some(lease.session_id.clone()),
        reserved_client_ip: Some(lease.reserved_client_ip.clone()),
        server_internal_ip: Some(lease.server_internal_ip.clone()),
        tunnel_profile_path,
        cooldown_until_unix: None,
        tor_exit_info: None,
        country_code: None,
        last_status_unix: Some(unix_now()),
    }
}

fn connection_from_lease_from_active(
    previous: &ActiveConnection,
    server_name: &str,
    lease: &ControlSessionLease,
    tunnel_profile_path: Option<String>,
) -> ActiveConnection {
    ActiveConnection {
        server_id: lease.server_id.clone(),
        server_name: server_name.to_owned(),
        endpoint: previous.endpoint.clone(),
        protocol: VpnProtocol::WireGuard,
        phase: ConnectionPhase::Connected,
        connected_at_unix: previous.connected_at_unix.or(Some(unix_now())),
        attempt_count: 0,
        session_id: Some(lease.session_id.clone()),
        reserved_client_ip: Some(lease.reserved_client_ip.clone()),
        server_internal_ip: Some(lease.server_internal_ip.clone()),
        tunnel_profile_path,
        cooldown_until_unix: None,
        tor_exit_info: None,
        country_code: None,
        last_status_unix: Some(unix_now()),
    }
}

fn prune_cooldowns(state: &mut ClientState) {
    let now = unix_now();
    state.cooldowns.retain(|_, entry| entry.until_unix > now);
}

fn print_servers(servers: &[ServerSummary]) {
    if servers.is_empty() {
        println!("No servers discovered.");
        return;
    }

    for server in servers {
        println!(
            "{}  {}  control={}  wg={}  {}  {}",
            server.server_id,
            server.name,
            server.endpoint,
            server.wireguard_endpoint,
            if server.has_password {
                "protected"
            } else {
                "open"
            },
            if server.online { "online" } else { "offline" }
        );
    }
}

fn phase_label(phase: &ConnectionPhase) -> &'static str {
    match phase {
        ConnectionPhase::Disconnected => "disconnected",
        ConnectionPhase::Connecting => "connecting",
        ConnectionPhase::Connected => "connected",
        ConnectionPhase::Reconnecting => "reconnecting",
        ConnectionPhase::Cooldown => "cooldown",
        ConnectionPhase::Error => "error",
    }
}

fn format_remaining(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else {
        let secs = seconds % 60;
        format!("{minutes}m {secs:02}s")
    }
}
