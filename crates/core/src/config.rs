use crate::app_paths::AppPaths;
use crate::auth::RateLimitJournal;
use crate::crypto::{generate_key_material, WireguardKeyMaterial};
use crate::model::{ClientState, ServerRuntimeSnapshot};
use crate::unix_now;
use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::env;
use std::fs;
use uuid::Uuid;

pub const DEFAULT_CONTROL_PORT: u16 = 51820;
pub const DEFAULT_WIREGUARD_PORT: u16 = 51821;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerConfig {
    pub server_id: String,
    pub name: String,
    pub country_code: String,
    pub country_name: String,
    pub listen_port: u16,
    #[serde(default = "default_wireguard_port")]
    pub wireguard_port: u16,
    pub openvpn_port: Option<u16>,
    pub openvpn_endpoint: Option<String>,
    pub vpn_subnet: String,
    #[serde(default = "default_vpn_subnet_ipv6")]
    pub vpn_subnet_ipv6: String,
    #[serde(default)]
    pub selected_global_ipv4: Vec<String>,
    #[serde(default)]
    pub selected_global_ipv6: Vec<String>,
    pub created_at_unix: u64,
    pub password_hash: Option<String>,
    pub wireguard_keys: WireguardKeyMaterial,
}

#[derive(Clone, Debug)]
pub struct ServerBootstrapOptions {
    pub name: Option<String>,
    pub country_code: Option<String>,
    pub country_name: Option<String>,
    pub listen_port: Option<u16>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientConfig {
    pub client_id: String,
    pub display_name: String,
    pub known_hosts: Vec<String>,
    pub reduced_motion: bool,
    #[serde(default)]
    pub wireguard_keys: Option<WireguardKeyMaterial>,
}

pub fn load_or_create_server_config(
    paths: &AppPaths,
    bootstrap: &ServerBootstrapOptions,
) -> Result<ServerConfig> {
    if paths.config_file.exists() {
        return load_toml(&paths.config_file);
    }

    let config = ServerConfig {
        server_id: Uuid::new_v4().to_string(),
        name: bootstrap.name.clone().unwrap_or_else(default_server_name),
        country_code: bootstrap
            .country_code
            .clone()
            .unwrap_or_else(|| String::from("LAN")),
        country_name: bootstrap
            .country_name
            .clone()
            .unwrap_or_else(|| String::from("Local Network")),
        listen_port: bootstrap.listen_port.unwrap_or(DEFAULT_CONTROL_PORT),
        wireguard_port: DEFAULT_WIREGUARD_PORT,
        openvpn_port: Some(1194),
        openvpn_endpoint: None,
        vpn_subnet: String::from("10.44.0.0/24"),
        vpn_subnet_ipv6: String::from("fd44::/64"),
        selected_global_ipv4: Vec::new(),
        selected_global_ipv6: Vec::new(),
        created_at_unix: unix_now(),
        password_hash: None,
        wireguard_keys: generate_key_material(),
    };

    save_server_config(paths, &config)?;
    Ok(config)
}

pub fn save_server_config(paths: &AppPaths, config: &ServerConfig) -> Result<()> {
    save_toml(&paths.config_file, config)
}

pub fn load_or_create_client_config(paths: &AppPaths) -> Result<ClientConfig> {
    if paths.config_file.exists() {
        let mut config: ClientConfig = load_toml(&paths.config_file)?;
        let needs_upgrade = config
            .wireguard_keys
            .as_ref()
            .map(|keys| !keys.is_complete())
            .unwrap_or(true);
        if needs_upgrade {
            config.wireguard_keys = Some(generate_key_material());
            save_client_config(paths, &config)?;
        }
        return Ok(config);
    }

    let config = ClientConfig {
        client_id: Uuid::new_v4().to_string(),
        display_name: default_client_name(),
        known_hosts: Vec::new(),
        reduced_motion: false,
        wireguard_keys: Some(generate_key_material()),
    };

    save_client_config(paths, &config)?;
    Ok(config)
}

pub fn save_client_config(paths: &AppPaths, config: &ClientConfig) -> Result<()> {
    save_toml(&paths.config_file, config)
}

pub fn load_or_create_server_runtime(paths: &AppPaths) -> Result<ServerRuntimeSnapshot> {
    if paths.runtime_file.exists() {
        return load_toml(&paths.runtime_file);
    }

    let runtime = ServerRuntimeSnapshot::default();
    save_server_runtime(paths, &runtime)?;
    Ok(runtime)
}

pub fn save_server_runtime(paths: &AppPaths, runtime: &ServerRuntimeSnapshot) -> Result<()> {
    save_toml(&paths.runtime_file, runtime)
}

pub fn load_or_create_client_state(paths: &AppPaths) -> Result<ClientState> {
    if paths.state_file.exists() {
        return load_toml(&paths.state_file);
    }

    let state = ClientState::default();
    save_client_state(paths, &state)?;
    Ok(state)
}

pub fn save_client_state(paths: &AppPaths, state: &ClientState) -> Result<()> {
    save_toml(&paths.state_file, state)
}

pub fn load_or_create_journal(paths: &AppPaths) -> Result<RateLimitJournal> {
    if paths.state_file.exists() {
        return load_toml(&paths.state_file);
    }

    let journal = RateLimitJournal::default();
    save_journal(paths, &journal)?;
    Ok(journal)
}

pub fn save_journal(paths: &AppPaths, journal: &RateLimitJournal) -> Result<()> {
    save_toml(&paths.state_file, journal)
}

fn load_toml<T: DeserializeOwned>(path: &std::path::Path) -> Result<T> {
    let data =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    toml::from_str(&data).with_context(|| format!("failed to parse {}", path.display()))
}

fn save_toml<T: Serialize>(path: &std::path::Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let data = toml::to_string_pretty(value).context("failed to serialize toml")?;
    fs::write(path, data).with_context(|| format!("failed to write {}", path.display()))
}

fn default_server_name() -> String {
    env::var("COMPUTERNAME")
        .or_else(|_| env::var("HOSTNAME"))
        .map(|name| format!("{name} Node"))
        .unwrap_or_else(|_| String::from("ZeroNode Host"))
}

fn default_client_name() -> String {
    env::var("USERNAME")
        .or_else(|_| env::var("USER"))
        .map(|name| format!("{name}'s client"))
        .unwrap_or_else(|_| String::from("Desktop client"))
}

fn default_wireguard_port() -> u16 {
    DEFAULT_WIREGUARD_PORT
}

fn default_vpn_subnet_ipv6() -> String {
    String::from("fd44::/64")
}
