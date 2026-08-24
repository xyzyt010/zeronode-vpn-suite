use anyhow::{Context, Result};
use directories::ProjectDirs;
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug)]
pub enum AppRole {
    Server,
    Client,
}

impl AppRole {
    fn slug(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::Client => "client",
        }
    }
}

#[derive(Clone, Debug)]
pub struct AppPaths {
    pub base_dir: PathBuf,
    pub config_file: PathBuf,
    pub state_file: PathBuf,
    pub runtime_file: PathBuf,
    pub log_file: PathBuf,
    pub profiles_dir: PathBuf,
}

pub fn server_paths() -> Result<AppPaths> {
    app_paths(AppRole::Server)
}

pub fn client_paths() -> Result<AppPaths> {
    app_paths(AppRole::Client)
}

pub fn app_paths(role: AppRole) -> Result<AppPaths> {
    let dirs = ProjectDirs::from("io", "ZeroNode", "VpnSuite")
        .context("could not resolve a writable application data directory")?;
    let base_dir = dirs.data_local_dir().join(role.slug());
    fs::create_dir_all(&base_dir)
        .with_context(|| format!("failed to create {}", base_dir.display()))?;
    let profiles_dir = base_dir.join("profiles");
    fs::create_dir_all(&profiles_dir)
        .with_context(|| format!("failed to create {}", profiles_dir.display()))?;

    Ok(AppPaths {
        base_dir: base_dir.clone(),
        config_file: base_dir.join("config.toml"),
        state_file: base_dir.join("state.toml"),
        runtime_file: base_dir.join("runtime.toml"),
        log_file: base_dir.join("events.log"),
        profiles_dir,
    })
}
