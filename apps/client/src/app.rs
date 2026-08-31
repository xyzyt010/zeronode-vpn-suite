use anyhow::{anyhow, Context, Result};
use eframe::{
    egui::{
        self, Align, Color32, FontFamily, FontId, Layout, Margin, Pos2, RichText, Sense, Stroke,
        Vec2,
    },
    App, NativeOptions,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    env,
    io::Cursor,
    path::{Path, PathBuf},
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::Duration,
};
use tokio::{runtime::Builder, sync::mpsc as tokio_mpsc, time};
#[cfg(target_os = "linux")]
use vpn_platform_linux;
#[cfg(target_os = "windows")]
use vpn_platform_windows;
#[cfg(target_os = "windows")]
use vpn_platform_windows as platform;
#[cfg(target_os = "linux")]
use vpn_platform_linux as platform;
#[cfg(not(any(target_os = "windows", target_os = "linux")))]
use vpn_platform_linux as platform;
use vpn_suite_core::{
    app_paths::{client_paths, server_paths, AppPaths},
    config::{
        load_or_create_client_config, load_or_create_client_state, load_or_create_server_config,
        save_client_config, save_client_state, save_server_config, ClientConfig,
        ServerBootstrapOptions,
    },
    control_plane::{
        attempt_auth, discover_servers, query_server_status, send_disconnect_notice,
    },
    geoip::GeoIpStack,
    model::{
        ActiveConnection, ClientSnapshot, ClientState, ConnectionPhase, ControlSessionLease,
        CooldownEntry, ServerSummary, VpnProtocol, VpnUiProtocol,
    },
    net_info::HostNetInfo,
    protocol::StatusResponse,
    unix_now,
    APP_NAME,
};
#[cfg(target_os = "android")]
use winit::platform::android::{activity::AndroidApp, EventLoopBuilderExtAndroid};

pub fn run_desktop() -> Result<()> {
    run_desktop_with_options(false)
}

/// Launch the desktop GUI.
/// - `auto_connect_tor`: after elevated UAC relaunch, start Tor system tunnel.
/// - `auto_connect_ovpn`: after elevated UAC relaunch, connect this OpenVPN profile id.
pub fn run_desktop_with_options(auto_connect_tor: bool) -> Result<()> {
    run_desktop_with_auto(auto_connect_tor, None)
}

pub fn run_desktop_with_auto(auto_connect_tor: bool, auto_connect_ovpn: Option<i64>) -> Result<()> {
    run_desktop_with_auto_ex(DesktopAutoConnect {
        tor: auto_connect_tor,
        ovpn: auto_connect_ovpn,
        wg: None,
        outline: None,
    })
}

/// Auto-connect flags used after elevated UAC relaunch.
#[derive(Clone, Debug, Default)]
pub struct DesktopAutoConnect {
    pub tor: bool,
    pub ovpn: Option<i64>,
    pub wg: Option<i64>,
    pub outline: Option<i64>,
}

pub fn run_desktop_with_auto_ex(auto: DesktopAutoConnect) -> Result<()> {
    crate::db::init_db().context("Failed to initialize database")?;
    let paths = client_paths()?;
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(true)
        .open(paths.base_dir.join("vpn-client.log"))?;

    tracing_subscriber::fmt()
        .with_writer(log_file)
        .with_env_filter("info")
        .with_target(false)
        .init();

    let config = load_or_create_client_config(&paths)?;
    let (command_tx, command_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    start_backend(paths.clone(), config, command_rx, event_tx);

    if auto.tor {
        tracing::info!("--auto-connect-tor: scheduling Tor system-wide connect");
        let _ = command_tx.send(ClientCommand::ConnectTor);
    }
    if let Some(id) = auto.ovpn {
        tracing::info!("--auto-connect-ovpn={id}: scheduling OpenVPN system-wide connect");
        let _ = command_tx.send(ClientCommand::ConnectOvpnFile {
            id,
            username: None,
            password: None,
        });
    }
    if let Some(id) = auto.wg {
        tracing::info!("--auto-connect-wg={id}: scheduling WireGuard system-wide connect");
        let _ = command_tx.send(ClientCommand::ConnectWireGuard { id });
    }
    if let Some(id) = auto.outline {
        tracing::info!("--auto-connect-outline={id}: scheduling Outline system-wide connect");
        let _ = command_tx.send(ClientCommand::ConnectOutline {
            id,
            system_wide: true,
        });
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1160.0, 720.0])
        .with_min_inner_size([820.0, 580.0])
        .with_title(APP_NAME)
        .with_app_id("io.zeronode.vpn");

    if let Some(icon) = load_icon_data() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }

    // Linux DE adaptation: detect backend and log
    #[cfg(target_os = "linux")]
    {
        let backend = std::env::var("ZERONODE_BACKEND").unwrap_or_default();
        let session_type = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
        let wayland_display = std::env::var("WAYLAND_DISPLAY").unwrap_or_default();
        tracing::info!(
            "DE backend: ZERONODE_BACKEND={} XDG_SESSION_TYPE={} WAYLAND_DISPLAY={} DISPLAY={}",
            backend,
            session_type,
            if wayland_display.is_empty() { "(none)" } else { &wayland_display },
            std::env::var("DISPLAY").unwrap_or_else(|_| "(none)".to_string())
        );
        if wayland_display.is_empty() {
            tracing::info!("Running on X11/XFCE4");
        } else {
            tracing::info!("Running on Wayland/GNOME (elevated will use X11 via XWayland)");
        }
    }

    let mut options = NativeOptions {
        viewport,
        ..Default::default()
    };

    // Linux: force winit backend via ZERONODE_BACKEND=x11|wayland when set.
    // winit 0.30 removed WINIT_UNIX_BACKEND, so we must use EventLoopBuilderExtX11/ExtWayland.
    // Auto-detect (WAYLAND_DISPLAY/DISPLAY) is used when the env is absent. The pkexec helper
    // already forces ZERONODE_BACKEND=x11 for the elevated child on Wayland (root cannot open
    // the user Wayland socket), so that child will use XWayland correctly.
    #[cfg(target_os = "linux")]
    {
        let backend = std::env::var("ZERONODE_BACKEND")
            .unwrap_or_default()
            .to_ascii_lowercase();
        if backend == "x11" || backend == "wayland" {
            let forced = backend.clone();
            options.event_loop_builder = Some(Box::new(move |builder| {
                use winit::platform::wayland::EventLoopBuilderExtWayland;
                use winit::platform::x11::EventLoopBuilderExtX11;
                if forced == "x11" {
                    builder.with_x11();
                } else {
                    builder.with_wayland();
                }
            }));
            tracing::info!("Forcing winit backend to {} via ZERONODE_BACKEND", backend);
        }
    }

    run_client_window(options, command_tx, event_rx)
}

fn load_icon_data() -> Option<egui::IconData> {
    let image_bytes = include_bytes!("../../../assets/icon.png");
    let image = image::load_from_memory(image_bytes).ok()?;
    let rgba = image.to_rgba8().into_raw();
    let width = image.width();
    let height = image.height();
    Some(egui::IconData {
        rgba,
        width,
        height,
    })
}

#[cfg(target_os = "android")]
pub fn run_android(android_app: AndroidApp) -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .try_init();

    let paths = client_paths()?;
    let config = load_or_create_client_config(&paths)?;
    let (command_tx, command_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    start_backend(paths.clone(), config, command_rx, event_tx);

    let options = NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title(APP_NAME),
        event_loop_builder: Some(Box::new(move |builder| {
            builder.with_android_app(android_app);
        })),
        ..Default::default()
    };

    run_client_window(options, command_tx, event_rx)
}

fn run_client_window(
    options: NativeOptions,
    command_tx: Sender<ClientCommand>,
    event_rx: Receiver<ClientEvent>,
) -> Result<()> {
    #[cfg(target_os = "linux")]
    let _tray = tray::create_tray(command_tx.clone());

    eframe::run_native(
        APP_NAME,
        options,
        Box::new(|_cc| {
            egui_extras::install_image_loaders(&_cc.egui_ctx);
            let renderer = crate::globe::renderer::GlobeRenderer::new();
            Ok(Box::new(VpnClientApp::new(command_tx, event_rx, renderer)))
        }),
    )
    .map_err(|error| anyhow!(error.to_string()))?;

    Ok(())
}

/// Shared egui context for tray/signal handlers (set on first frame).
static APP_EGUI_CTX: std::sync::OnceLock<egui::Context> = std::sync::OnceLock::new();

#[derive(Clone)]
enum ClientCommand {
    RefreshNow,
    AddHost(String),
    ApplyTunnel,
    RemoveTunnel,
    Connect {
        server_id: String,
        endpoint: String,
        server_name: String,
        password: Option<String>,
        protocol: vpn_suite_core::model::VpnProtocol,
    },
    /// Bring up the local Tor SOCKS5 proxy only — does NOT require
    /// Administrator. The connection succeeds with the SOCKS5 port up, the
    /// GeoIP right-pane card populated, and the right pane updated. The
    /// system-wide route (Wintun + tun2proxy) is a separate explicit step
    /// triggered by `ApplyTorSystemRoute`.
    ConnectTor,
    /// Bring up the system-wide route on top of an already-running Tor
    /// SOCKS5 connection. On Windows this needs Administrator; we ask
    /// politely via the relaunch helper but NEVER kill this process on UAC
    /// failure (the old behaviour silently exited when UAC was declined).
    ApplyTorSystemRoute,
    /// Tear down the system-wide route (if any) while leaving the Tor
    /// SOCKS5 proxy itself running. Useful for letting the user switch back
    /// to direct connections without dropping the Tor session.
    RemoveTorSystemRoute,
    /// Fired by the Tor bootstrap thread once the GeoIP lookup resolves.
    /// Replaces the legacy message that always attempted to start the
    /// system tunnel (and therefore required elevation). System tunnel is
    /// now opt-in via `ApplyTorSystemRoute`.
    TorConnected { ip: String, country: String, socks_port: u16, exit_info: Option<vpn_suite_core::model::TorExitInfo> },
    GeoIpReady(Arc<GeoIpStack>),
    /// Hit ip-api.com directly (NOT through Tor) to populate the right-pane
    /// "Your IP Details" card with the user's real public IP. The backend
    /// runs this on startup, on demand, and every 5 minutes thereafter.
    RefreshLocalIp,
    /// Connect a specific OpenVPN profile (optional user/pass for auth-user-pass).
    ConnectOvpnFile {
        id: i64,
        username: Option<String>,
        password: Option<String>,
    },
    /// Connect the currently selected OpenVPN profile (same idea as Tor Connect).
    ConnectSelectedOvpn {
        username: Option<String>,
        password: Option<String>,
    },
    /// Parse + GeoIP-enrich a newly imported profile in the background.
    EnrichOvpnProfile(i64),
    /// Ensure OpenVPN binary exists (system or managed strip-down).
    #[allow(dead_code)]
    EnsureOpenVpnBinary,
    /// Connect imported WireGuard profile (embedded boringtun + Wintun on Windows).
    ConnectWireGuard { id: i64 },
    EnrichWgProfile(i64),
    /// Connect PPTP profile via OS RAS.
    ConnectPptp { id: i64 },
    /// Connect Outline access key (sslocal + optional system TUN).
    ConnectOutline { id: i64, system_wide: bool },
    EnrichOutlineProfile(i64),
    Disconnect,
    SaveLocalServerSelection {
        ipv4: Vec<String>,
        ipv6: Vec<String>,
    },
    StartLocalServer,
    StopLocalServer,
    /// Persist Tor isolation mode: "system" (Wintun 0.0.0.0/0) or "apps" (SOCKS5 only).
    SetTorIsolationMode(String),
    /// Launch an app with SOCKS5 env pointed at the local Tor listener.
    LaunchTorIsolatedApp(i64),
    /// Tray menu / re-launch: un-hide the main window.
    ShowMainWindow,
    /// Quit for real: tear down every tunnel (helper + direct), kill Tor,
    /// then exit the process.
    QuitApp,
    /// SIGTERM/SIGINT received — same as QuitApp but triggered externally
    /// (task manager "End Task" sends SIGTERM first).
    SignalQuit,
}

#[derive(Clone)]
enum ClientEvent {
    Snapshot(ClientSnapshot),
    ReloadOvpnConfigs,
    /// Reload WireGuard / PPTP / Outline profile lists from SQLite.
    ReloadProtocolProfiles,
    ReloadTorApps,
    #[allow(dead_code)]
    Notice(String),
}

struct VpnClientApp {
    command_tx: Sender<ClientCommand>,
    event_rx: Receiver<ClientEvent>,
    snapshot: ClientSnapshot,
    manual_host_input: String,
    password_dialog: Option<PasswordDialog>,
    ovpn_auth_dialog: Option<OvpnAuthDialog>,
    server_settings_dialog: Option<ServerSettingsDialog>,
    globe_renderer: crate::globe::renderer::GlobeRenderer,
    ovpn_configs: Vec<crate::db::OvpnConfig>,
    /// Selected OpenVPN profile id (combo + connect target).
    selected_ovpn_id: Option<i64>,
    /// Right-pane multi-protocol dropdown (default OpenVPN).
    selected_vpn_protocol: VpnUiProtocol,
    wg_configs: Vec<crate::db::WgConfig>,
    selected_wg_id: Option<i64>,
    /// Paste buffer for WireGuard `.conf` text.
    wg_draft_paste: String,
    wg_draft_name: String,
    pptp_configs: Vec<crate::db::PptpConfig>,
    selected_pptp_id: Option<i64>,
    /// Draft fields for "Add PPTP server" form.
    pptp_draft_name: String,
    pptp_draft_host: String,
    pptp_draft_user: String,
    pptp_draft_pass: String,
    pptp_draft_domain: String,
    outline_configs: Vec<crate::db::OutlineConfig>,
    selected_outline_id: Option<i64>,
    /// Paste buffer for Outline access key / JSON.
    outline_draft_key: String,
    outline_draft_name: String,
    /// Tor SOCKS5 isolation app list.
    tor_isolated_apps: Vec<crate::db::TorIsolatedApp>,
    /// "system" = full-PC Wintun route; "apps" = SOCKS5 only for chosen apps.
    tor_isolation_mode: String,
    /// Track previous active server to detect connection changes.
    prev_active_server_id: Option<String>,
    /// Track previous connection phase.
    prev_phase: Option<ConnectionPhase>,
    /// Track previous exit country code so the globe pan animation can fire
    /// when the Tor GeoIP lookup resolves (which happens after the connection
    /// server_id has already been set to "tor_local").
    prev_country_code: Option<String>,
    /// Previous public-IP country (direct lookup) so we re-pan on launch/refresh.
    prev_local_country: Option<String>,
    /// Previous public-IP coords for change detection on Refresh.
    prev_local_coords: Option<(f64, f64)>,
    /// Previous public IP string — re-trigger pin/glow even if country is unchanged.
    prev_local_ip: Option<String>,
    /// Last seen `globe_pan_token` from the backend — force pan on every Refresh.
    prev_globe_pan_token: u64,
    /// Whether the current process is running elevated. Populated once on
    /// startup; used to decide whether to render the
    /// "Enable System-Wide Routing" button (vs. just showing "Admin only").
    elevated: bool,
    /// User-controlled right pane width (pixels). Content never drives this —
    /// only the left-edge drag grip does. Prevents the classic egui feedback
    /// loop where PanelState stores content rect width and the pane creeps wider.
    side_panel_width: f32,
    /// True while the user is dragging the side-panel resize grip.
    side_panel_resizing: bool,
    /// Wall-clock (egui time) when Tor bootstrap started — drives the fill bar.
    tor_bootstrap_started: Option<f64>,
    /// Wall-clock (egui time) when OpenVPN connect started — green fill bar.
    ovpn_bootstrap_started: Option<f64>,
    /// Progress bars for other imported protocols.
    wg_bootstrap_started: Option<f64>,
    pptp_bootstrap_started: Option<f64>,
    outline_bootstrap_started: Option<f64>,
    /// Fonts installed once (Segoe UI / Consolas when available).
    fonts_installed: bool,
    /// Collapsed state for optional side-pane sections.
    net_section_open: bool,
    host_section_open: bool,
}

/// Side pane width bounds (user-resizable within this range).
const SIDE_PANEL_MIN: f32 = 240.0;
const SIDE_PANEL_MAX: f32 = 560.0;
/// Always open at full width on launch (user can still drag narrower).
const SIDE_PANEL_DEFAULT: f32 = SIDE_PANEL_MAX;
/// Below this content width, Tor/OpenVPN headers stack buttons vertically.
const PANE_NARROW_BREAK: f32 = 300.0;

struct PasswordDialog {
    server_id: String,
    server_name: String,
    endpoint: String,
    value: String,
}

/// Credentials for OpenVPN profiles that declare `auth-user-pass`.
struct OvpnAuthDialog {
    profile_id: i64,
    profile_name: String,
    username: String,
    password: String,
}

struct ServerSettingsDialog {
    selected_ipv4: BTreeSet<String>,
    selected_ipv6: BTreeSet<String>,
}

impl VpnClientApp {
    fn new(command_tx: Sender<ClientCommand>, event_rx: Receiver<ClientEvent>, globe_renderer: crate::globe::renderer::GlobeRenderer) -> Self {
        let _ = command_tx.send(ClientCommand::RefreshNow);
        let _ = command_tx.send(ClientCommand::RefreshLocalIp);
        Self {
            command_tx,
            event_rx,
            snapshot: ClientSnapshot::default(),
            manual_host_input: String::new(),
            password_dialog: None,
            ovpn_auth_dialog: None,
            server_settings_dialog: None,
            globe_renderer,
            ovpn_configs: crate::db::get_ovpn_configs().unwrap_or_default(),
            selected_ovpn_id: crate::db::get_selected_ovpn_id(),
            selected_vpn_protocol: crate::db::get_selected_vpn_protocol(),
            wg_configs: crate::db::get_wg_configs().unwrap_or_default(),
            selected_wg_id: crate::db::get_selected_wg_id(),
            wg_draft_paste: String::new(),
            wg_draft_name: String::new(),
            pptp_configs: crate::db::get_pptp_configs().unwrap_or_default(),
            selected_pptp_id: crate::db::get_selected_pptp_id(),
            pptp_draft_name: String::new(),
            pptp_draft_host: String::new(),
            pptp_draft_user: String::new(),
            pptp_draft_pass: String::new(),
            pptp_draft_domain: String::new(),
            outline_configs: crate::db::get_outline_configs().unwrap_or_default(),
            selected_outline_id: crate::db::get_selected_outline_id(),
            outline_draft_key: String::new(),
            outline_draft_name: String::new(),
            tor_isolated_apps: crate::db::list_tor_isolated_apps().unwrap_or_default(),
            tor_isolation_mode: crate::db::get_tor_isolation_mode(),
            prev_active_server_id: None,
            prev_phase: None,
            prev_country_code: None,
            prev_local_country: None,
            prev_local_coords: None,
            prev_local_ip: None,
            prev_globe_pan_token: 0,
            elevated: platform::is_elevated(),
            side_panel_width: SIDE_PANEL_DEFAULT,
            side_panel_resizing: false,
            tor_bootstrap_started: None,
            ovpn_bootstrap_started: None,
            wg_bootstrap_started: None,
            pptp_bootstrap_started: None,
            outline_bootstrap_started: None,
            fonts_installed: false,
            net_section_open: false,
            host_section_open: true,
        }
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                ClientEvent::Snapshot(snapshot) => self.snapshot = snapshot,
                ClientEvent::ReloadOvpnConfigs => {
                    self.ovpn_configs = crate::db::get_ovpn_configs().unwrap_or_default();
                    // Keep selection valid after deletes.
                    if let Some(id) = self.selected_ovpn_id {
                        if !self.ovpn_configs.iter().any(|c| c.id == id) {
                            self.selected_ovpn_id = self.ovpn_configs.first().map(|c| c.id);
                            crate::db::set_selected_ovpn_id(self.selected_ovpn_id);
                        }
                    } else if let Some(first) = self.ovpn_configs.first() {
                        self.selected_ovpn_id = Some(first.id);
                        crate::db::set_selected_ovpn_id(self.selected_ovpn_id);
                    }
                }
                ClientEvent::ReloadProtocolProfiles => {
                    self.wg_configs = crate::db::get_wg_configs().unwrap_or_default();
                    self.pptp_configs = crate::db::get_pptp_configs().unwrap_or_default();
                    self.outline_configs = crate::db::get_outline_configs().unwrap_or_default();
                    if let Some(id) = self.selected_wg_id {
                        if !self.wg_configs.iter().any(|c| c.id == id) {
                            self.selected_wg_id = self.wg_configs.first().map(|c| c.id);
                            crate::db::set_selected_wg_id(self.selected_wg_id);
                        }
                    }
                    if let Some(id) = self.selected_pptp_id {
                        if !self.pptp_configs.iter().any(|c| c.id == id) {
                            self.selected_pptp_id = self.pptp_configs.first().map(|c| c.id);
                            crate::db::set_selected_pptp_id(self.selected_pptp_id);
                        }
                    }
                    if let Some(id) = self.selected_outline_id {
                        if !self.outline_configs.iter().any(|c| c.id == id) {
                            self.selected_outline_id =
                                self.outline_configs.first().map(|c| c.id);
                            crate::db::set_selected_outline_id(self.selected_outline_id);
                        }
                    }
                }
                ClientEvent::ReloadTorApps => {
                    self.tor_isolated_apps =
                        crate::db::list_tor_isolated_apps().unwrap_or_default();
                    self.tor_isolation_mode = crate::db::get_tor_isolation_mode();
                }
                ClientEvent::Notice(msg) => {
                    self.snapshot.notice = Some(msg);
                }
            }
        }
    }

    /// Start OpenVPN connect: prompt for credentials when the profile needs
    /// `auth-user-pass`, otherwise send the backend command immediately.
    fn request_openvpn_connect(&mut self, id: Option<i64>) {
        let Some(id) = id else {
            self.snapshot.notice =
                Some(String::from("Select an OpenVPN server first."));
            return;
        };
        let Some(cfg) = self.ovpn_configs.iter().find(|c| c.id == id) else {
            self.snapshot.notice = Some(String::from("OpenVPN profile not found."));
            return;
        };
        // If credentials were saved for this profile, connect without a dialog.
        let auth_path = vpn_suite_core::app_paths::client_paths()
            .ok()
            .map(|p| crate::ovpn::auth_file_path(&p.profiles_dir, id));
        let has_saved_auth = auth_path
            .as_ref()
            .map(|p| p.is_file())
            .unwrap_or(false);
        if crate::ovpn::needs_auth_user_pass(&cfg.content) && !has_saved_auth {
            self.ovpn_auth_dialog = Some(OvpnAuthDialog {
                profile_id: id,
                profile_name: cfg.name.clone(),
                username: String::new(),
                password: String::new(),
            });
            return;
        }
        let _ = self.command_tx.send(ClientCommand::ConnectOvpnFile {
            id,
            username: None,
            password: None,
        });
    }

    /// Import a `.ovpn` path: store profile, parse remotes, kick GeoIP enrich.
    fn import_ovpn_path(&mut self, path: &std::path::Path) {
        let Ok(content) = std::fs::read_to_string(path) else {
            return;
        };
        let name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let parsed = crate::ovpn::parse_ovpn(&content);
        let resolved = if parsed.remote_host.is_empty() {
            None
        } else {
            crate::ovpn::resolve_remote_ip(&parsed.remote_host)
        };
        let mut cfg = crate::db::OvpnConfig {
            id: 0,
            name: name.clone(),
            content,
            country_code: None,
            fail_count: 0,
            remote_host: if parsed.remote_host.is_empty() {
                None
            } else {
                Some(parsed.remote_host.clone())
            },
            remote_port: Some(parsed.remote_port),
            proto: Some(parsed.proto.clone()),
            resolved_ip: resolved,
            city: None,
            region: None,
            country: None,
            isp: None,
            org: None,
            as_name: None,
            timezone: None,
            lat: None,
            lon: None,
            cipher: if parsed.cipher.is_empty() {
                None
            } else {
                Some(parsed.cipher)
            },
            auth: if parsed.auth.is_empty() {
                None
            } else {
                Some(parsed.auth)
            },
        };
        if let Ok(id) = crate::db::add_ovpn_config_full(&cfg) {
            cfg.id = id;
            self.ovpn_configs.push(cfg);
            self.selected_ovpn_id = Some(id);
            crate::db::set_selected_ovpn_id(Some(id));
            let _ = self.command_tx.send(ClientCommand::EnrichOvpnProfile(id));
            self.snapshot.notice = Some(format!("Imported OpenVPN server '{name}'. Looking up location…"));
        }
    }

    /// Import a WireGuard `.conf` profile into the multi-protocol store.
    fn import_wg_path(&mut self, path: &std::path::Path) {
        let Ok(content) = std::fs::read_to_string(path) else {
            self.snapshot.notice = Some(String::from("Could not read WireGuard config file."));
            return;
        };
        let name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        self.import_wg_content(&name, &content);
    }

    fn import_wg_content(&mut self, name: &str, content: &str) {
        let content = content.trim();
        if content.is_empty() {
            self.snapshot.notice = Some(String::from("WireGuard config is empty."));
            return;
        }
        let summary = crate::protocols::parse_wg_summary(content);
        // Require Interface PrivateKey + Peer PublicKey + Endpoint for a usable client conf.
        let has_private = content.lines().any(|l| {
            let t = l.trim();
            t.starts_with("PrivateKey") && t.contains('=')
        });
        if !has_private || summary.endpoint.is_none() || summary.public_key.is_none() {
            self.snapshot.notice = Some(String::from(
                "Invalid WireGuard config: need [Interface] PrivateKey and [Peer] PublicKey + Endpoint.",
            ));
            return;
        }
        let name = if name.trim().is_empty() {
            summary
                .endpoint
                .clone()
                .unwrap_or_else(|| String::from("WireGuard"))
        } else {
            name.trim().to_string()
        };
        let mut cfg = crate::db::WgConfig {
            id: 0,
            name: name.clone(),
            content: content.to_string(),
            endpoint: summary.endpoint,
            public_key: summary.public_key,
            address: summary.address,
            country_code: None,
            resolved_ip: None,
            city: None,
            region: None,
            country: None,
            isp: None,
            lat: None,
            lon: None,
        };
        match crate::db::add_wg_config(&cfg) {
            Ok(id) => {
                cfg.id = id;
                self.wg_configs.push(cfg);
                self.selected_wg_id = Some(id);
                crate::db::set_selected_wg_id(Some(id));
                let _ = self.command_tx.send(ClientCommand::EnrichWgProfile(id));
                self.snapshot.notice =
                    Some(format!("Imported WireGuard profile '{name}'. Looking up location…"));
            }
            Err(e) => {
                self.snapshot.notice = Some(format!("Could not save WireGuard profile: {e:#}"));
            }
        }
    }

    fn render_wireguard_panel(&mut self, ui: &mut egui::Ui, narrow: bool, panel_w: f32) {
        use crate::protocols::{VPN_CARD_BG, VPN_GREEN, VPN_GREEN_DIM};

        let wg_conn = self
            .snapshot
            .active_connection
            .as_ref()
            .filter(|a| a.server_id.starts_with("wg_"));
        let is_connecting = wg_conn
            .map(|a| a.phase == ConnectionPhase::Connecting)
            .unwrap_or(false);
        let is_connected = wg_conn
            .map(|a| a.phase == ConnectionPhase::Connected)
            .unwrap_or(false);
        let is_busy = is_connecting || is_connected;

        egui::Frame::none()
            .fill(VPN_CARD_BG)
            .stroke(Stroke::new(1.0, VPN_GREEN_DIM))
            .rounding(8.0)
            .inner_margin(Margin::symmetric(if narrow { 8.0 } else { 10.0 }, 8.0))
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width());
                let title = RichText::new(if narrow {
                    "WireGuard"
                } else {
                    "WireGuard tunnel"
                })
                .color(Color32::WHITE)
                .strong()
                .font(FontId::new(
                    if narrow { 14.0 } else { 15.0 },
                    FontFamily::Proportional,
                ));
                let mut actions = |ui: &mut egui::Ui| {
                    if is_busy {
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Disconnect").color(Color32::WHITE),
                                )
                                .fill(Color32::from_rgb(48, 48, 48))
                                .min_size(Vec2::new(
                                    if narrow { ui.available_width() } else { 0.0 },
                                    26.0,
                                )),
                            )
                            .clicked()
                        {
                            let _ = self.command_tx.send(ClientCommand::Disconnect);
                        }
                    } else {
                        let can = self.selected_wg_id.is_some() && !self.wg_configs.is_empty();
                        if ui
                            .add_enabled(
                                can,
                                egui::Button::new(
                                    RichText::new("Connect").color(Color32::BLACK),
                                )
                                .fill(VPN_GREEN)
                                .min_size(Vec2::new(
                                    if narrow { ui.available_width() } else { 0.0 },
                                    26.0,
                                )),
                            )
                            .clicked()
                        {
                            if let Some(id) = self.selected_wg_id {
                                self.wg_bootstrap_started = Some(ui.input(|i| i.time));
                                let _ =
                                    self.command_tx.send(ClientCommand::ConnectWireGuard { id });
                            }
                        }
                    }
                };
                if narrow {
                    ui.label(title);
                    ui.add_space(4.0);
                    actions(ui);
                } else {
                    ui.horizontal(|ui| {
                        ui.label(title);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            actions(ui);
                        });
                    });
                }
                let disconnecting = self.snapshot.op_progress_kind.as_deref() == Some("disconnect")
                    && self.snapshot.op_progress > 0.0
                    && self.snapshot.op_progress < 1.0;
                if is_connecting || disconnecting {
                    ui.add_space(10.0);
                    let started = self
                        .wg_bootstrap_started
                        .unwrap_or(ui.input(|i| i.time));
                    let elapsed = (ui.input(|i| i.time) - started).max(0.0) as f32;
                    let kind = if disconnecting { "disconnect" } else { "connect" };
                    let (progress, label) = self.real_op_progress(kind, elapsed);
                    ui.label(
                        RichText::new(label)
                            .color(VPN_GREEN)
                            .font(FontId::new(13.0, FontFamily::Proportional))
                            .strong(),
                    );
                    ui.add_space(6.0);
                    let bar_width = (ui.available_width() - 4.0).max(80.0);
                    paint_connect_progress_bar(ui, progress, bar_width, VPN_GREEN);
                    ui.label(
                        RichText::new(format!("{:.0}%", progress * 100.0))
                            .color(Color32::from_rgb(140, 220, 170))
                            .font(FontId::new(12.0, FontFamily::Proportional)),
                    );
                    ui.ctx().request_repaint();
                } else if is_connected {
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("WireGuard system tunnel ACTIVE")
                            .color(VPN_GREEN)
                            .font(FontId::new(12.0, FontFamily::Proportional))
                            .strong(),
                    );
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new("Select profile:")
                        .color(Color32::from_rgb(170, 170, 170))
                        .font(FontId::new(11.0, FontFamily::Proportional)),
                );
                let selected_label = self
                    .selected_wg_id
                    .and_then(|id| self.wg_configs.iter().find(|c| c.id == id))
                    .map(|c| {
                        let loc = c
                            .country
                            .as_deref()
                            .or(c.country_code.as_deref())
                            .unwrap_or("");
                        if loc.is_empty() {
                            c.name.clone()
                        } else {
                            format!("{} · {}", c.name, loc)
                        }
                    })
                    .unwrap_or_else(|| String::from("(import a .conf profile)"));
                let combo_w = (ui.available_width() - 4.0).clamp(80.0, panel_w.max(80.0));
                egui::ComboBox::from_id_salt("wg_server_select")
                    .selected_text(selected_label)
                    .width(combo_w)
                    .show_ui(ui, |ui| {
                        for c in &self.wg_configs {
                            let label = {
                                let loc = c
                                    .country
                                    .as_deref()
                                    .or(c.country_code.as_deref())
                                    .unwrap_or("");
                                if loc.is_empty() {
                                    c.name.clone()
                                } else {
                                    format!("{} · {}", c.name, loc)
                                }
                            };
                            if ui
                                .selectable_label(self.selected_wg_id == Some(c.id), label)
                                .clicked()
                            {
                                self.selected_wg_id = Some(c.id);
                                crate::db::set_selected_wg_id(Some(c.id));
                            }
                        }
                    });
                if let Some(id) = self.selected_wg_id {
                    if let Some(sel) = self.wg_configs.iter().find(|c| c.id == id) {
                        ui.add_space(4.0);
                        let small = FontId::new(10.0, FontFamily::Proportional);
                        ui.label(
                            RichText::new(sel.endpoint_label())
                                .color(VPN_GREEN)
                                .font(small.clone()),
                        );
                        if let Some(addr) = sel.address.as_ref() {
                            ui.label(
                                RichText::new(format!("Address: {addr}"))
                                    .color(Color32::from_rgb(160, 160, 160))
                                    .font(small.clone()),
                            );
                        }
                        if let Some(pk) = sel.public_key.as_ref() {
                            let short = if pk.len() > 20 {
                                format!("{}…", &pk[..16])
                            } else {
                                pk.clone()
                            };
                            ui.label(
                                RichText::new(format!("Peer: {short}"))
                                    .color(Color32::from_rgb(140, 140, 140))
                                    .font(small),
                            );
                        }
                    }
                }
            });

        ui.add_space(10.0);
        // Paste full .conf text
        egui::Frame::none()
            .fill(VPN_CARD_BG)
            .stroke(Stroke::new(1.0, Color32::from_rgb(50, 50, 50)))
            .rounding(8.0)
            .inner_margin(Margin::symmetric(10.0, 8.0))
            .show(ui, |ui| {
                ui.label(
                    RichText::new("Paste WireGuard config")
                        .color(Color32::WHITE)
                        .strong(),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Name").color(Color32::from_rgb(150, 150, 150)));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.wg_draft_name)
                            .desired_width(ui.available_width())
                            .hint_text("optional label"),
                    );
                });
                ui.add(
                    egui::TextEdit::multiline(&mut self.wg_draft_paste)
                        .desired_width(ui.available_width())
                        .desired_rows(5)
                        .hint_text("[Interface]\nPrivateKey = …\nAddress = …\n\n[Peer]\nPublicKey = …\nEndpoint = host:51820\nAllowedIPs = 0.0.0.0/0"),
                );
                if ui
                    .add(
                        egui::Button::new(RichText::new("Save config").color(Color32::BLACK))
                            .fill(VPN_GREEN),
                    )
                    .clicked()
                {
                    let paste = self.wg_draft_paste.clone();
                    let name = self.wg_draft_name.clone();
                    self.import_wg_content(&name, &paste);
                    if self
                        .snapshot
                        .notice
                        .as_deref()
                        .map(|n| n.contains("Imported"))
                        .unwrap_or(false)
                    {
                        self.wg_draft_paste.clear();
                        self.wg_draft_name.clear();
                    }
                }
            });

        ui.add_space(10.0);
        let drop_resp = egui::Frame::none()
            .fill(Color32::from_rgb(18, 22, 20))
            .stroke(Stroke::new(1.5, VPN_GREEN_DIM))
            .rounding(8.0)
            .inner_margin(Margin::symmetric(10.0, 12.0))
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width());
                ui.set_min_height(52.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new("Drop .conf here  ·  or click to browse")
                            .color(Color32::from_rgb(0, 220, 150))
                            .strong()
                            .font(FontId::new(12.0, FontFamily::Proportional)),
                    );
                    ui.label(
                        RichText::new("WireGuard client config only (.conf files)")
                            .color(Color32::from_rgb(140, 140, 140))
                            .font(FontId::new(11.0, FontFamily::Proportional)),
                    );
                });
            })
            .response
            .interact(Sense::click());
        // Also accept drops while hovering this zone (global drop already handled).
        if drop_resp.hovered() {
            ui.ctx().input(|i| {
                for f in &i.raw.dropped_files {
                    if let Some(path) = &f.path {
                        if path
                            .extension()
                            .map(|e| e.eq_ignore_ascii_case("conf"))
                            .unwrap_or(false)
                        {
                            // Handled globally; keep zone visual only.
                        }
                    }
                }
            });
        }
        if drop_resp.clicked() {
            #[cfg(not(target_os = "android"))]
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Select WireGuard config (.conf)")
                .add_filter("WireGuard config (*.conf)", &["conf"])
                .pick_file()
            {
                // Enforce extension even if the OS dialog allows "All files".
                let ok = path
                    .extension()
                    .map(|e| e.eq_ignore_ascii_case("conf"))
                    .unwrap_or(false);
                if ok {
                    self.import_wg_path(&path);
                } else {
                    self.snapshot.notice = Some(String::from(
                        "Please select a .conf WireGuard configuration file.",
                    ));
                }
            }
            #[cfg(target_os = "android")]
            {
                self.snapshot.notice = Some(String::from(
                    "File picker is desktop-only — paste the .conf contents on Android.",
                ));
            }
        }

        ui.add_space(10.0);
        let mut delete_idx = None;
        for (idx, config) in self.wg_configs.iter().enumerate() {
            let is_sel = self.selected_wg_id == Some(config.id);
            let stroke = if is_sel {
                Stroke::new(1.5, Color32::from_rgb(0, 200, 120))
            } else {
                Stroke::new(1.0, Color32::from_rgb(31, 31, 31))
            };
            egui::Frame::none()
                .fill(VPN_CARD_BG)
                .stroke(stroke)
                .rounding(8.0)
                .inner_margin(Margin::symmetric(10.0, 8.0))
                .show(ui, |ui| {
                    ui.set_max_width(ui.available_width());
                    ui.horizontal(|ui| {
                        if is_sel {
                            ui.label(RichText::new("●").color(VPN_GREEN));
                        }
                        ui.label(RichText::new(&config.name).color(Color32::WHITE).strong());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .button(RichText::new("🗑").color(Color32::from_rgb(180, 180, 180)))
                                .clicked()
                            {
                                let _ = crate::db::delete_wg_config(config.id);
                                delete_idx = Some(idx);
                            }
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(if is_sel { "Selected" } else { "Select" })
                                            .color(Color32::BLACK),
                                    )
                                    .fill(if is_sel {
                                        Color32::from_rgb(0, 180, 100)
                                    } else {
                                        Color32::from_rgb(80, 80, 80)
                                    }),
                                )
                                .clicked()
                            {
                                self.selected_wg_id = Some(config.id);
                                crate::db::set_selected_wg_id(Some(config.id));
                            }
                        });
                    });
                    ui.label(
                        RichText::new(config.endpoint_label())
                            .color(VPN_GREEN)
                            .font(FontId::new(10.0, FontFamily::Proportional)),
                    );
                });
            ui.add_space(8.0);
        }
        if let Some(idx) = delete_idx {
            let removed = self.wg_configs.remove(idx);
            if self.selected_wg_id == Some(removed.id) {
                self.selected_wg_id = self.wg_configs.first().map(|c| c.id);
                crate::db::set_selected_wg_id(self.selected_wg_id);
            }
        }
    }

    fn render_pptp_panel(&mut self, ui: &mut egui::Ui, narrow: bool, _panel_w: f32) {
        use crate::protocols::{VPN_CARD_BG, VPN_GREEN, VPN_GREEN_DIM, WARN_AMBER};
        use vpn_suite_core::pptp::SECURITY_WARNING;

        ui.label(
            RichText::new(SECURITY_WARNING)
                .color(WARN_AMBER)
                .font(FontId::new(10.0, FontFamily::Proportional)),
        );
        ui.add_space(6.0);

        let pptp_conn = self
            .snapshot
            .active_connection
            .as_ref()
            .filter(|a| a.server_id.starts_with("pptp_"));
        let is_connecting = pptp_conn
            .map(|a| a.phase == ConnectionPhase::Connecting)
            .unwrap_or(false);
        let is_connected = pptp_conn
            .map(|a| a.phase == ConnectionPhase::Connected)
            .unwrap_or(false);
        let is_busy = is_connecting || is_connected;

        egui::Frame::none()
            .fill(VPN_CARD_BG)
            .stroke(Stroke::new(1.0, VPN_GREEN_DIM))
            .rounding(8.0)
            .inner_margin(Margin::symmetric(if narrow { 8.0 } else { 10.0 }, 8.0))
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("PPTP tunnel")
                            .color(Color32::WHITE)
                            .strong()
                            .font(FontId::new(15.0, FontFamily::Proportional)),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if is_busy {
                            if ui
                                .button(RichText::new("Disconnect").color(Color32::WHITE))
                                .clicked()
                            {
                                let _ = self.command_tx.send(ClientCommand::Disconnect);
                            }
                        } else {
                            let can = self.selected_pptp_id.is_some();
                            if ui
                                .add_enabled(
                                    can,
                                    egui::Button::new(
                                        RichText::new("Connect").color(Color32::BLACK),
                                    )
                                    .fill(VPN_GREEN),
                                )
                                .clicked()
                            {
                                if let Some(id) = self.selected_pptp_id {
                                    self.pptp_bootstrap_started = Some(ui.input(|i| i.time));
                                    let _ =
                                        self.command_tx.send(ClientCommand::ConnectPptp { id });
                                }
                            }
                        }
                    });
                });
                let disconnecting = self.snapshot.op_progress_kind.as_deref() == Some("disconnect")
                    && self.snapshot.op_progress > 0.0
                    && self.snapshot.op_progress < 1.0;
                if is_connecting || disconnecting {
                    ui.add_space(8.0);
                    let started = self
                        .pptp_bootstrap_started
                        .unwrap_or(ui.input(|i| i.time));
                    let elapsed = (ui.input(|i| i.time) - started).max(0.0) as f32;
                    let kind = if disconnecting { "disconnect" } else { "connect" };
                    let (progress, label) = self.real_op_progress(kind, elapsed);
                    ui.label(
                        RichText::new(label)
                            .color(VPN_GREEN)
                            .font(FontId::new(12.0, FontFamily::Proportional)),
                    );
                    paint_connect_progress_bar(
                        ui,
                        progress,
                        (ui.available_width() - 4.0).max(80.0),
                        VPN_GREEN,
                    );
                    ui.label(
                        RichText::new(format!("{:.0}%", progress * 100.0))
                            .color(Color32::from_rgb(140, 220, 170))
                            .font(FontId::new(11.0, FontFamily::Proportional)),
                    );
                    ui.ctx().request_repaint();
                } else if is_connected {
                    ui.label(
                        RichText::new("PPTP connected (legacy)")
                            .color(VPN_GREEN)
                            .strong(),
                    );
                }
            });

        ui.add_space(10.0);
        egui::Frame::none()
            .fill(VPN_CARD_BG)
            .stroke(Stroke::new(1.0, Color32::from_rgb(50, 50, 50)))
            .rounding(8.0)
            .inner_margin(Margin::symmetric(10.0, 8.0))
            .show(ui, |ui| {
                ui.label(
                    RichText::new("Add PPTP server")
                        .color(Color32::WHITE)
                        .strong(),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Name").color(Color32::from_rgb(150, 150, 150)));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.pptp_draft_name)
                            .desired_width(ui.available_width()),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Host").color(Color32::from_rgb(150, 150, 150)));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.pptp_draft_host)
                            .desired_width(ui.available_width()),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("User").color(Color32::from_rgb(150, 150, 150)));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.pptp_draft_user)
                            .desired_width(ui.available_width()),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Pass").color(Color32::from_rgb(150, 150, 150)));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.pptp_draft_pass)
                            .password(true)
                            .desired_width(ui.available_width()),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Domain").color(Color32::from_rgb(150, 150, 150)));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.pptp_draft_domain)
                            .desired_width(ui.available_width())
                            .hint_text("optional"),
                    );
                });
                ui.add_space(4.0);
                if ui
                    .add(
                        egui::Button::new(RichText::new("Save profile").color(Color32::BLACK))
                            .fill(VPN_GREEN),
                    )
                    .clicked()
                {
                    let host = self.pptp_draft_host.trim().to_string();
                    let user = self.pptp_draft_user.trim().to_string();
                    if host.is_empty() || user.is_empty() {
                        self.snapshot.notice =
                            Some(String::from("PPTP requires host and username."));
                    } else {
                        let name = if self.pptp_draft_name.trim().is_empty() {
                            host.clone()
                        } else {
                            self.pptp_draft_name.trim().to_string()
                        };
                        let cfg = crate::db::PptpConfig {
                            id: 0,
                            name,
                            host,
                            port: 1723,
                            username: user,
                            password: self.pptp_draft_pass.clone(),
                            domain: self.pptp_draft_domain.trim().to_string(),
                            country_code: None,
                            resolved_ip: None,
                            city: None,
                            region: None,
                            country: None,
                            lat: None,
                            lon: None,
                        };
                        if let Ok(id) = crate::db::add_pptp_config(&cfg) {
                            let mut saved = cfg;
                            saved.id = id;
                            self.pptp_configs.push(saved);
                            self.selected_pptp_id = Some(id);
                            crate::db::set_selected_pptp_id(Some(id));
                            self.pptp_draft_name.clear();
                            self.pptp_draft_host.clear();
                            self.pptp_draft_user.clear();
                            self.pptp_draft_pass.clear();
                            self.pptp_draft_domain.clear();
                            self.snapshot.notice =
                                Some(String::from("PPTP profile saved."));
                        }
                    }
                }
            });

        ui.add_space(8.0);
        let mut delete_idx = None;
        for (idx, config) in self.pptp_configs.iter().enumerate() {
            let is_sel = self.selected_pptp_id == Some(config.id);
            egui::Frame::none()
                .fill(VPN_CARD_BG)
                .stroke(Stroke::new(
                    1.0,
                    if is_sel {
                        Color32::from_rgb(0, 200, 120)
                    } else {
                        Color32::from_rgb(31, 31, 31)
                    },
                ))
                .rounding(8.0)
                .inner_margin(Margin::symmetric(10.0, 8.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(&config.name)
                                .color(Color32::WHITE)
                                .strong(),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button("🗑").clicked() {
                                let _ = crate::db::delete_pptp_config(config.id);
                                delete_idx = Some(idx);
                            }
                            if ui
                                .button(if is_sel { "Selected" } else { "Select" })
                                .clicked()
                            {
                                self.selected_pptp_id = Some(config.id);
                                crate::db::set_selected_pptp_id(Some(config.id));
                            }
                        });
                    });
                    ui.label(
                        RichText::new(format!(
                            "{} · {}",
                            config.endpoint_label(),
                            config.username
                        ))
                        .color(Color32::from_rgb(160, 160, 160))
                        .font(FontId::new(10.0, FontFamily::Proportional)),
                    );
                });
            ui.add_space(6.0);
        }
        if let Some(idx) = delete_idx {
            let removed = self.pptp_configs.remove(idx);
            if self.selected_pptp_id == Some(removed.id) {
                self.selected_pptp_id = self.pptp_configs.first().map(|c| c.id);
                crate::db::set_selected_pptp_id(self.selected_pptp_id);
            }
        }
    }

    fn render_outline_panel(&mut self, ui: &mut egui::Ui, narrow: bool, _panel_w: f32) {
        use crate::protocols::{VPN_CARD_BG, VPN_GREEN, VPN_GREEN_DIM, WARN_AMBER};

        ui.label(
            RichText::new(
                "Paste an Outline access key (ss://…) or dynamic-key JSON. \
                 Shadowsocks runs embedded in ZeroNode — no external sslocal.exe.",
            )
            .color(Color32::from_rgb(150, 150, 150))
            .font(FontId::new(11.0, FontFamily::Proportional)),
        );
        ui.add_space(6.0);

        let outline_conn = self
            .snapshot
            .active_connection
            .as_ref()
            .filter(|a| a.server_id.starts_with("outline_"));
        let is_connecting = outline_conn
            .map(|a| a.phase == ConnectionPhase::Connecting)
            .unwrap_or(false);
        let is_connected = outline_conn
            .map(|a| a.phase == ConnectionPhase::Connected)
            .unwrap_or(false);
        let is_busy = is_connecting || is_connected;

        egui::Frame::none()
            .fill(VPN_CARD_BG)
            .stroke(Stroke::new(1.0, VPN_GREEN_DIM))
            .rounding(8.0)
            .inner_margin(Margin::symmetric(if narrow { 8.0 } else { 10.0 }, 8.0))
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Outline tunnel")
                            .color(Color32::WHITE)
                            .strong()
                            .font(FontId::new(15.0, FontFamily::Proportional)),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if is_busy {
                            if ui
                                .button(RichText::new("Disconnect").color(Color32::WHITE))
                                .clicked()
                            {
                                let _ = self.command_tx.send(ClientCommand::Disconnect);
                            }
                        } else {
                            let can = self.selected_outline_id.is_some();
                            if ui
                                .add_enabled(
                                    can,
                                    egui::Button::new(
                                        RichText::new("Connect").color(Color32::BLACK),
                                    )
                                    .fill(VPN_GREEN),
                                )
                                .on_hover_text("System-wide Outline (sslocal + Wintun)")
                                .clicked()
                            {
                                if let Some(id) = self.selected_outline_id {
                                    self.outline_bootstrap_started = Some(ui.input(|i| i.time));
                                    let _ = self.command_tx.send(ClientCommand::ConnectOutline {
                                        id,
                                        system_wide: true,
                                    });
                                }
                            }
                        }
                    });
                });
                let disconnecting = self.snapshot.op_progress_kind.as_deref() == Some("disconnect")
                    && self.snapshot.op_progress > 0.0
                    && self.snapshot.op_progress < 1.0;
                if is_connecting || disconnecting {
                    ui.add_space(8.0);
                    let started = self
                        .outline_bootstrap_started
                        .unwrap_or(ui.input(|i| i.time));
                    let elapsed = (ui.input(|i| i.time) - started).max(0.0) as f32;
                    let kind = if disconnecting { "disconnect" } else { "connect" };
                    let (progress, label) = self.real_op_progress(kind, elapsed);
                    ui.label(
                        RichText::new(label)
                            .color(VPN_GREEN)
                            .font(FontId::new(12.0, FontFamily::Proportional))
                            .strong(),
                    );
                    ui.add_space(4.0);
                    paint_connect_progress_bar(
                        ui,
                        progress,
                        (ui.available_width() - 4.0).max(80.0),
                        VPN_GREEN,
                    );
                    ui.label(
                        RichText::new(format!("{:.0}%", progress * 100.0))
                            .color(Color32::from_rgb(140, 220, 170))
                            .font(FontId::new(11.0, FontFamily::Proportional)),
                    );
                    ui.ctx().request_repaint();
                } else if is_connected {
                    ui.label(
                        RichText::new("Outline system tunnel ACTIVE")
                            .color(VPN_GREEN)
                            .strong(),
                    );
                }
            });

        ui.add_space(10.0);
        egui::Frame::none()
            .fill(VPN_CARD_BG)
            .stroke(Stroke::new(1.0, Color32::from_rgb(50, 50, 50)))
            .rounding(8.0)
            .inner_margin(Margin::symmetric(10.0, 8.0))
            .show(ui, |ui| {
                ui.label(
                    RichText::new("Import access key")
                        .color(Color32::WHITE)
                        .strong(),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Name").color(Color32::from_rgb(150, 150, 150)));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.outline_draft_name)
                            .desired_width(ui.available_width())
                            .hint_text("optional label"),
                    );
                });
                ui.label(
                    RichText::new("Access key / JSON")
                        .color(Color32::from_rgb(150, 150, 150)),
                );
                ui.add(
                    egui::TextEdit::multiline(&mut self.outline_draft_key)
                        .desired_width(ui.available_width())
                        .desired_rows(3)
                        .hint_text("ss://… or {\"server\":…}"),
                );
                if ui
                    .add(
                        egui::Button::new(RichText::new("Save key").color(Color32::BLACK))
                            .fill(VPN_GREEN),
                    )
                    .clicked()
                {
                    match vpn_suite_core::outline::parse_outline_input(&self.outline_draft_key) {
                        Ok(ep) => {
                            if let Some(w) = ep.security_warning() {
                                self.snapshot.notice = Some(w.to_string());
                            }
                            let name = if self.outline_draft_name.trim().is_empty() {
                                ep.name.clone().unwrap_or_else(|| ep.endpoint_label())
                            } else {
                                self.outline_draft_name.trim().to_string()
                            };
                            let cfg = crate::db::OutlineConfig {
                                id: 0,
                                name,
                                access_key: self.outline_draft_key.trim().to_string(),
                                method: Some(ep.method.clone()),
                                host: Some(ep.host.clone()),
                                port: Some(ep.port),
                                country_code: None,
                                resolved_ip: None,
                                city: None,
                                region: None,
                                country: None,
                                lat: None,
                                lon: None,
                            };
                            if let Ok(id) = crate::db::add_outline_config(&cfg) {
                                let mut saved = cfg;
                                saved.id = id;
                                self.outline_configs.push(saved);
                                self.selected_outline_id = Some(id);
                                crate::db::set_selected_outline_id(Some(id));
                                self.outline_draft_key.clear();
                                self.outline_draft_name.clear();
                                let _ = self
                                    .command_tx
                                    .send(ClientCommand::EnrichOutlineProfile(id));
                                self.snapshot.notice =
                                    Some(String::from("Outline key saved. Looking up location…"));
                            }
                        }
                        Err(e) => {
                            self.snapshot.notice =
                                Some(format!("Invalid Outline key: {e:#}"));
                        }
                    }
                }
                #[cfg(target_os = "windows")]
                {
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new("Embedded Shadowsocks · system tunnel via Wintun")
                            .color(Color32::from_rgb(100, 180, 130))
                            .font(FontId::new(10.0, FontFamily::Proportional)),
                    );
                    let _ = WARN_AMBER; // keep import used if other warns remain
                }
            });

        ui.add_space(8.0);
        let mut delete_idx = None;
        for (idx, config) in self.outline_configs.iter().enumerate() {
            let is_sel = self.selected_outline_id == Some(config.id);
            egui::Frame::none()
                .fill(VPN_CARD_BG)
                .stroke(Stroke::new(
                    1.0,
                    if is_sel {
                        Color32::from_rgb(0, 200, 120)
                    } else {
                        Color32::from_rgb(31, 31, 31)
                    },
                ))
                .rounding(8.0)
                .inner_margin(Margin::symmetric(10.0, 8.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(&config.name)
                                .color(Color32::WHITE)
                                .strong(),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button("🗑").clicked() {
                                let _ = crate::db::delete_outline_config(config.id);
                                delete_idx = Some(idx);
                            }
                            if ui
                                .button(if is_sel { "Selected" } else { "Select" })
                                .clicked()
                            {
                                self.selected_outline_id = Some(config.id);
                                crate::db::set_selected_outline_id(Some(config.id));
                            }
                        });
                    });
                    let method = config.method.as_deref().unwrap_or("?");
                    ui.label(
                        RichText::new(format!("{} · {}", config.endpoint_label(), method))
                            .color(Color32::from_rgb(160, 160, 160))
                            .font(FontId::new(10.0, FontFamily::Proportional)),
                    );
                });
            ui.add_space(6.0);
        }
        if let Some(idx) = delete_idx {
            let removed = self.outline_configs.remove(idx);
            if self.selected_outline_id == Some(removed.id) {
                self.selected_outline_id = self.outline_configs.first().map(|c| c.id);
                crate::db::set_selected_outline_id(self.selected_outline_id);
            }
        }
    }

    /// Drive globe pan/zoom exclusively from **Your IP** (`local_ip_info`).
    ///
    /// That card is reverse-IP/GeoIP refreshed on launch, manual Refresh, and
    /// immediately after Tor/OpenVPN reach Connected — so the globe always
    /// tracks the public IP the world currently sees.
    fn update_globe_animation(&mut self, time: f64) {
        let current = self.snapshot.active_connection.as_ref();
        let current_id = current.map(|a| a.server_id.clone());
        let current_phase = current.map(|a| a.phase.clone());

        let local_country = self
            .snapshot
            .local_ip_info
            .as_ref()
            .map(|i| i.country_code.clone())
            .filter(|c| !c.is_empty());
        let local_coords = self.snapshot.local_ip_info.as_ref().and_then(|i| {
            if i.lat != 0.0 || i.lon != 0.0 {
                Some((i.lat, i.lon))
            } else {
                None
            }
        });
        let local_ip = self
            .snapshot
            .local_ip_info
            .as_ref()
            .map(|i| i.ip.clone())
            .filter(|s| !s.is_empty());

        let local_country_changed =
            local_country != self.prev_local_country && local_country.is_some();
        let local_coords_changed = match (local_coords, self.prev_local_coords) {
            (Some((a, b)), Some((c, d))) => (a - c).abs() > 0.05 || (b - d).abs() > 0.05,
            (Some(_), None) => true,
            _ => false,
        };
        let local_ip_changed =
            local_ip.is_some() && local_ip != self.prev_local_ip;
        let first_resolve = (local_coords.is_some() || local_country.is_some())
            && self.prev_local_country.is_none()
            && self.prev_local_coords.is_none();
        // Backend bumps globe_pan_token on every successful IP refresh (including
        // manual Refresh) so we always pan/tilt from wherever the user left the
        // globe to the current public-IP location.
        let pan_token = self.snapshot.globe_pan_token;
        let token_bumped =
            pan_token != 0 && pan_token != self.prev_globe_pan_token;
        let needs_pan = local_country_changed
            || local_coords_changed
            || local_ip_changed
            || first_resolve
            || token_bumped;

        if needs_pan {
            if let Some((lat, lon)) = local_coords {
                self.globe_renderer
                    .trigger_connection_anim_coords("your_ip", lat, lon, time);
            } else if let Some(ref cc) = local_country {
                self.globe_renderer
                    .trigger_connection_anim("your_ip", cc, time);
            }
            self.globe_renderer.mark_connected(time);
        } else if matches!(current_phase, Some(ConnectionPhase::Connected))
            && self.prev_phase.as_ref() != Some(&ConnectionPhase::Connected)
        {
            // Connected with same geo already shown — still pulse the beacon.
            if let Some((lat, lon)) = local_coords {
                self.globe_renderer
                    .trigger_connection_anim_coords("your_ip", lat, lon, time);
            } else if let Some(ref cc) = local_country {
                self.globe_renderer
                    .trigger_connection_anim("your_ip", cc, time);
            }
            self.globe_renderer.mark_connected(time);
        }

        // Keep tunnel country field for side-card UI; globe no longer keys off it.
        let tunnel_country = current.and_then(|a| a.country_code.clone());

        self.prev_active_server_id = current_id;
        self.prev_phase = current_phase;
        self.prev_country_code = tunnel_country;
        self.prev_local_country = local_country;
        self.prev_local_coords = local_coords;
        self.prev_local_ip = local_ip;
        self.prev_globe_pan_token = pan_token;
    }

    /// Real progress from backend stages; falls back to a mild time ramp only
    /// while waiting for the first stage update.
    fn real_op_progress(&self, kind: &str, elapsed: f32) -> (f32, String) {
        let snap_kind = self.snapshot.op_progress_kind.as_deref().unwrap_or("");
        if snap_kind == kind && self.snapshot.op_progress > 0.0 {
            let label = self
                .snapshot
                .op_progress_label
                .clone()
                .unwrap_or_else(|| {
                    if kind == "disconnect" {
                        String::from("Disconnecting…")
                    } else {
                        String::from("Connecting…")
                    }
                });
            return (self.snapshot.op_progress.clamp(0.0, 1.0), label);
        }
        // Soft bootstrap until backend publishes first stage (max ~18%).
        let soft = (1.0 - (-elapsed / 6.0).exp()) * 0.18;
        let label = if kind == "disconnect" {
            String::from("Disconnecting…")
        } else {
            String::from("Starting…")
        };
        (soft, label)
    }

    fn open_server_settings(&mut self) {
        let net_info = HostNetInfo::query();
        let selected_ipv4 = net_info
            .effective_selected_global_ipv4(&self.snapshot.local_server_selected_ipv4)
            .into_iter()
            .collect();
        let selected_ipv6 = net_info
            .effective_selected_global_ipv6(&self.snapshot.local_server_selected_ipv6)
            .into_iter()
            .collect();
        self.server_settings_dialog = Some(ServerSettingsDialog {
            selected_ipv4,
            selected_ipv6,
        });
    }
}

impl App for VpnClientApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Publish context once for tray Show / quit repaints.
        let _ = APP_EGUI_CTX.set(ctx.clone());
        // Remember the context for command handlers (tray Show / Quit).
        #[cfg(target_os = "linux")]
        install_signal_handlers(self.command_tx.clone());

        // Close (X) hides to tray — VPN keeps running. Real exit is via
        // Close (X) → hide to tray, VPN stays up. Real exit via tray Quit /
        // Disconnect&Quit / SIGTERM. If tray never initialized (gtk::init
        // failed, no display), fall back to real quit with teardown so we
        // don't orphan Tor/TUN.
        let close_requested = ctx.input(|i| i.viewport().close_requested());
        if close_requested {
            #[cfg(target_os = "linux")]
            let tray_ok = tray::is_available();
            #[cfg(not(target_os = "linux"))]
            let tray_ok = true;
            if tray_ok {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                let is_wayland = std::env::var("WAYLAND_DISPLAY")
                    .map(|v| !v.is_empty())
                    .unwrap_or(false);
                if is_wayland {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                    tracing::info!("close requested: Minimized on Wayland (Visible not implemented)");
                } else {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                    tracing::info!("close requested: hid to tray (Visible false) — VPN stays up");
                }
                let _ = self.command_tx.send(ClientCommand::RefreshNow);
                ctx.request_repaint();
            } else {
                tracing::warn!("close requested but tray not available — quitting with teardown");
                let _ = self.command_tx.send(ClientCommand::QuitApp);
                // Keep window alive until teardown's process::exit
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
        }

        // OS signals (SIGTERM/SIGINT) → graceful teardown.
        // The dedicated `zn-signal` thread already forwards SignalQuit via
        // the std channel; this backup poll guarantees delivery even if the
        // thread raced or the first send was dropped (direct std send, no
        // tokio::spawn needed — we are on the GUI thread outside any runtime).
        #[cfg(target_os = "linux")]
        if SIGNAL_QUIT.load(std::sync::atomic::Ordering::SeqCst)
            && !SIGNAL_QUIT_TAKEN.swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            tracing::warn!("GUI polled SIGTERM/SIGINT — forwarding SignalQuit");
            let _ = self.command_tx.send(ClientCommand::SignalQuit);
        }

        let dropped_files = ctx.input(|i| i.raw.dropped_files.clone());
        for file in dropped_files {
            if let Some(path) = &file.path {
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                match ext.as_str() {
                    "ovpn" => {
                        self.selected_vpn_protocol = VpnUiProtocol::OpenVPN;
                        crate::db::set_selected_vpn_protocol(VpnUiProtocol::OpenVPN);
                        self.import_ovpn_path(path);
                    }
                    "conf" | "wg" => {
                        self.selected_vpn_protocol = VpnUiProtocol::WireGuard;
                        crate::db::set_selected_vpn_protocol(VpnUiProtocol::WireGuard);
                        self.import_wg_path(path);
                    }
                    _ => {}
                }
            }
        }

        self.drain_events();
        // Keep UI live while real connect/disconnect stages advance.
        if self.snapshot.op_progress_kind.is_some()
            && self.snapshot.op_progress > 0.0
            && self.snapshot.op_progress < 1.0
        {
            ctx.request_repaint();
        }
        let time = ctx.input(|i| i.time);
        self.update_globe_animation(time);

        // Clear bootstrap timers when no longer in Connecting phase.
        let phase_connecting = |prefix: &str| {
            self.snapshot
                .active_connection
                .as_ref()
                .map(|a| {
                    a.server_id.starts_with(prefix) && a.phase == ConnectionPhase::Connecting
                })
                .unwrap_or(false)
        };
        let tor_still_connecting = self
            .snapshot
            .active_connection
            .as_ref()
            .map(|a| a.server_id == "tor_local" && a.phase == ConnectionPhase::Connecting)
            .unwrap_or(false);
        if !tor_still_connecting {
            self.tor_bootstrap_started = None;
        }
        let ovpn_still_connecting = phase_connecting("ovpn_");
        if !ovpn_still_connecting {
            self.ovpn_bootstrap_started = None;
        }
        let wg_still_connecting = phase_connecting("wg_");
        if !wg_still_connecting {
            self.wg_bootstrap_started = None;
        }
        let pptp_still_connecting = phase_connecting("pptp_");
        if !pptp_still_connecting {
            self.pptp_bootstrap_started = None;
        }
        let outline_still_connecting = phase_connecting("outline_");
        if !outline_still_connecting {
            self.outline_bootstrap_started = None;
        }

        // Continuous repaint only while something is actively animating.
        if self.globe_renderer.is_animating()
            || tor_still_connecting
            || ovpn_still_connecting
            || wg_still_connecting
            || pptp_still_connecting
            || outline_still_connecting
            || self.globe_renderer.is_dragging
            || self.side_panel_resizing
        {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        if !self.fonts_installed {
            install_crisp_fonts(ctx);
            self.fonts_installed = true;
        }
        install_theme(ctx);

        // Keep side-panel width under our control even while resizing mid-frame.
        if self.side_panel_resizing {
            if let Some(pos) = ctx.pointer_interact_pos() {
                let right = ctx.screen_rect().right();
                self.side_panel_width = (right - pos.x).clamp(SIDE_PANEL_MIN, SIDE_PANEL_MAX);
            }
            if ctx.input(|i| !i.pointer.any_down()) {
                self.side_panel_resizing = false;
            }
            ctx.request_repaint();
        }
        self.side_panel_width = self.side_panel_width.clamp(SIDE_PANEL_MIN, SIDE_PANEL_MAX);

        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("ZeroNode Client")
                        .font(FontId::new(24.0, FontFamily::Proportional))
                        .strong()
                        .color(Color32::WHITE),
                );
            });
            ui.add_space(6.0);
        });

        let mut open_server_settings = false;
        // exact_width: content can NEVER grow the panel. User resizes via grip only.
        // Right margin keeps cards off the window edge; left room for the resize grip.
        egui::SidePanel::right("details")
            .exact_width(self.side_panel_width)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                egui::Frame::none()
                    .fill(Color32::from_rgb(8, 8, 8))
                    .inner_margin(Margin {
                        left: 14.0,
                        right: 18.0,
                        top: 12.0,
                        bottom: 12.0,
                    })
                    .stroke(Stroke::new(1.0, Color32::from_rgb(32, 32, 32))),
            )
            .show(ctx, |ui| {
                // Left-edge resize grip (professional, stable width ownership).
                let full = ui.max_rect();
                let grip = egui::Rect::from_x_y_ranges(
                    (full.left() - 2.0)..=(full.left() + 6.0),
                    full.y_range(),
                );
                let grip_resp =
                    ui.interact(grip, egui::Id::new("details_width_grip"), Sense::click_and_drag());
                if grip_resp.hovered() || grip_resp.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                }
                if grip_resp.drag_started() || grip_resp.dragged() {
                    self.side_panel_resizing = true;
                    if let Some(pos) = grip_resp.interact_pointer_pos() {
                        let right = ui.ctx().screen_rect().right();
                        self.side_panel_width =
                            (right - pos.x).clamp(SIDE_PANEL_MIN, SIDE_PANEL_MAX);
                    }
                }
                // Subtle grip line
                let grip_color = if grip_resp.hovered() || self.side_panel_resizing {
                    Color32::from_rgb(0, 255, 127)
                } else {
                    Color32::from_rgb(48, 48, 48)
                };
                ui.painter().vline(
                    full.left() + 0.5,
                    full.y_range(),
                    Stroke::new(1.0, grip_color),
                );

                // Content width = available after frame margins, minus scrollbar gutter.
                // Never force set_width to the full outer panel — that is what caused
                // cards/buttons to spill past the right edge of the window.
                // Leave room for scrollbar + frame stroke so cards never paint past the pane.
                let panel_w = (ui.available_width() - 16.0).max(120.0);
                ui.set_max_width(panel_w);
                let narrow = panel_w < PANE_NARROW_BREAK;

                egui::ScrollArea::vertical()
                    .id_salt("details_scroll")
                    .auto_shrink([false, false])
                    .max_width(panel_w)
                    .show(ui, |ui| {
                        ui.set_max_width(panel_w);
                        ui.set_width(panel_w);

                        render_session_section(ui, &self.snapshot, panel_w);

                        ui.add_space(14.0);
                        render_ip_details_card(
                            ui,
                            &self.snapshot,
                            &self.command_tx,
                            panel_w,
                        );
                        
                        ui.add_space(16.0);
                        section_title(ui, "Tor", Some(Color32::from_rgb(168, 85, 247)));
                        {
                            let label = if self.elevated {
                                "System VPN available (Admin)"
                            } else {
                                #[cfg(target_os = "windows")]
                                { "System VPN needs Admin / UAC" }
                                #[cfg(not(target_os = "windows"))]
                                { "System VPN needs Admin (pkexec)" }
                            };
                            ui.label(
                                RichText::new(label)
                                .font(FontId::new(12.0, FontFamily::Proportional))
                                .color(Color32::from_rgb(150, 150, 150)),
                            );
                        }
                        ui.add_space(8.0);

                        // Clone so later UI can mutably borrow `self` without
                        // holding a reference into snapshot.active_connection.
                        let tor_conn = self
                            .snapshot
                            .active_connection
                            .clone()
                            .filter(|a| a.server_id == "tor_local");
                        let is_tor_connecting = tor_conn
                            .as_ref()
                            .map_or(false, |a| a.phase == vpn_suite_core::model::ConnectionPhase::Connecting);
                        let is_tor_connected = tor_conn
                            .as_ref()
                            .map_or(false, |a| a.phase == vpn_suite_core::model::ConnectionPhase::Connected);
                        let tor_purple = Color32::from_rgb(168, 85, 247);
                        let tor_disconnecting = self.snapshot.op_progress_kind.as_deref()
                            == Some("disconnect")
                            && self.snapshot.op_progress > 0.0
                            && self.snapshot.op_progress < 1.0;
                        let now_t = ui.input(|i| i.time);
                        if (is_tor_connecting || tor_disconnecting)
                            && self.tor_bootstrap_started.is_none()
                        {
                            self.tor_bootstrap_started = Some(now_t);
                        }
                        let tor_elapsed = self
                            .tor_bootstrap_started
                            .map(|s| (now_t - s).max(0.0) as f32)
                            .unwrap_or(0.0);
                        let tor_kind = if tor_disconnecting {
                            "disconnect"
                        } else {
                            "connect"
                        };
                        let (tor_progress, mut tor_progress_label) =
                            self.real_op_progress(tor_kind, tor_elapsed);
                        if tor_progress_label == "Starting…" && !tor_disconnecting {
                            tor_progress_label = String::from("Bootstrapping Tor circuit…");
                        }

                        ui.push_id("tor_card", |ui| {
                        egui::Frame::none()
                            .fill(Color32::from_rgb(12, 10, 16))
                            .stroke(Stroke::new(1.0, tor_purple))
                            .rounding(8.0)
                            .inner_margin(Margin::symmetric(if narrow { 8.0 } else { 10.0 }, 10.0))
                            .show(ui, |ui| {
                                // Stay inside the frame; do not force outer panel width.
                                ui.set_max_width(ui.available_width());
                                let title = RichText::new("Tor VPN")
                                    .font(FontId::new(if narrow { 14.0 } else { 16.0 }, FontFamily::Proportional))
                                    .color(Color32::WHITE)
                                    .strong();
                                let mut connect_btn = |ui: &mut egui::Ui| {
                                    if is_tor_connected {
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    RichText::new("Disconnect").color(Color32::WHITE),
                                                )
                                                .fill(Color32::from_rgb(42, 42, 48))
                                                .min_size(Vec2::new(if narrow { ui.available_width() } else { 0.0 }, 26.0)),
                                            )
                                            .clicked()
                                        {
                                            let _ = self.command_tx.send(ClientCommand::Disconnect);
                                        }
                                    } else if ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new("Connect").color(Color32::WHITE),
                                            )
                                            .fill(tor_purple)
                                            .min_size(Vec2::new(if narrow { ui.available_width() } else { 0.0 }, 26.0)),
                                        )
                                        .clicked()
                                    {
                                        if !is_tor_connecting {
                                            self.tor_bootstrap_started = Some(ui.input(|i| i.time));
                                            let _ = self.command_tx.send(ClientCommand::ConnectTor);
                                        }
                                    }
                                };
                                if narrow {
                                    ui.label(title);
                                    ui.add_space(4.0);
                                    connect_btn(ui);
                                } else {
                                    ui.horizontal(|ui| {
                                        ui.label(title);
                                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                            connect_btn(ui);
                                        });
                                    });
                                }
                                
                                if is_tor_connecting || tor_disconnecting {
                                    ui.add_space(10.0);
                                    let progress = tor_progress;
                                    let label = tor_progress_label.clone();
                                    ui.label(
                                        RichText::new(label)
                                            .color(tor_purple)
                                            .font(FontId::new(13.0, FontFamily::Proportional))
                                            .strong(),
                                    );
                                    ui.add_space(6.0);
                                    let bar_width = (ui.available_width() - 4.0).max(80.0);
                                    paint_connect_progress_bar(ui, progress, bar_width, tor_purple);
                                    ui.add_space(2.0);
                                    ui.label(
                                        RichText::new(format!("{:.0}%", progress * 100.0))
                                            .color(Color32::from_rgb(180, 160, 210))
                                            .font(FontId::new(12.0, FontFamily::Proportional)),
                                    );
                                    ui.ctx().request_repaint();
                                } else if is_tor_connected {
                                    ui.add_space(4.0);
                                    ui.label(
                                        RichText::new("Tor circuit established")
                                            .color(tor_purple)
                                            .font(FontId::new(12.0, FontFamily::Proportional))
                                            .strong(),
                                    );
                                }

                                // Connected state: show exit-country flag, country name, exit IP,
                                // and full GeoIP details (city, region, ISP, AS, lat/lon, timezone).
                                if let Some(active) = tor_conn {
                                    if is_tor_connected {
                                        ui.add_space(8.0);
                                        let cc = active
                                            .country_code
                                            .as_deref()
                                            .or_else(|| {
                                                active
                                                    .tor_exit_info
                                                    .as_ref()
                                                    .map(|t| t.country_code.as_str())
                                                    .filter(|c| !c.is_empty())
                                            })
                                            .unwrap_or("");
                                        // Flag + country name row
                                        ui.horizontal(|ui| {
                                            if !cc.is_empty() {
                                                show_flag(ui, cc, Vec2::new(48.0, 34.0));
                                            }
                                            ui.vertical(|ui| {
                                                let display_name = active
                                                    .tor_exit_info
                                                    .as_ref()
                                                    .map(|t| t.country.as_str())
                                                    .filter(|n| !n.is_empty() && *n != "Resolving…")
                                                    .map(|n| n.to_string())
                                                    .or_else(|| {
                                                        if !cc.is_empty() {
                                                            self.globe_renderer
                                                                .centroids
                                                                .get(cc)
                                                                .map(|c| c.name.clone())
                                                                .or_else(|| Some(cc.to_string()))
                                                        } else {
                                                            None
                                                        }
                                                    })
                                                    .unwrap_or_else(|| String::from("Unknown"));
                                                ui.label(
                                                    RichText::new(format!("Tor Exit: {display_name}"))
                                                        .color(Color32::WHITE)
                                                        .strong(),
                                                );
                                                if !cc.is_empty() {
                                                    ui.label(
                                                        RichText::new(format!("Exit country: {cc}"))
                                                            .color(Color32::from_rgb(170, 170, 170))
                                                            .font(FontId::new(10.0, FontFamily::Proportional)),
                                                    );
                                                }
                                            });
                                        });
                                        // Exit IP row
                                        ui.add_space(2.0);
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                RichText::new("Exit IP:")
                                                    .color(Color32::from_rgb(170, 170, 170))
                                                    .font(FontId::new(10.0, FontFamily::Proportional)),
                                            );
                                            let exit_ip = active
                                                .tor_exit_info
                                                .as_ref()
                                                .map(|t| t.ip.as_str())
                                                .filter(|ip| !ip.is_empty())
                                                .unwrap_or(active.endpoint.as_str());
                                            ui.label(
                                                RichText::new(exit_ip)
                                                    .color(Color32::from_rgb(0, 255, 127))
                                                    .font(FontId::new(10.0, FontFamily::Proportional)),
                                            );
                                        });
                                        if let Some(port) = self.snapshot.tor_socks_port {
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    RichText::new("Local SOCKS5:")
                                                        .color(Color32::from_rgb(170, 170, 170))
                                                        .font(FontId::new(10.0, FontFamily::Proportional)),
                                                );
                                                ui.label(
                                                    RichText::new(format!("127.0.0.1:{port}"))
                                                        .color(Color32::from_rgb(200, 180, 255))
                                                        .font(FontId::new(10.0, FontFamily::Proportional)),
                                                );
                                            });
                                        }

                                        // Full GeoIP detail block. Rendered only when the
                                        // TorConnected handler populated tor_exit_info (i.e. the
                                        // ip-api lookup succeeded). This is display-only and does
                                        // NOT feed the globe animation — the globe still uses
                                        // country_code -> centroid above.
                                        if let Some(info) = active.tor_exit_info.as_ref() {
                                            ui.add_space(6.0);
                                            let label_col = Color32::from_rgb(140, 140, 140);
                                            let value_col = Color32::from_rgb(210, 210, 210);
                                            let small = FontId::new(10.0, FontFamily::Proportional);

                                            // Build "City, Region" line, skipping empties.
                                            let mut location_parts: Vec<String> = Vec::new();
                                            if !info.city.is_empty() { location_parts.push(info.city.clone()); }
                                            if !info.region.is_empty() { location_parts.push(info.region.clone()); }
                                            if !location_parts.is_empty() {
                                                ui.horizontal(|ui| {
                                                    ui.label(RichText::new("Location:").color(label_col).font(small.clone()));
                                                    ui.label(RichText::new(location_parts.join(", "))
                                                        .color(value_col).font(small.clone()));
                                                });
                                            }
                                            // ISP
                                            if !info.isp.is_empty() {
                                                ui.horizontal(|ui| {
                                                    ui.label(RichText::new("ISP:").color(label_col).font(small.clone()));
                                                    ui.label(RichText::new(&info.isp)
                                                        .color(value_col).font(small.clone()));
                                                });
                                            }
                                            // Organization (skip if identical to ISP to avoid noise)
                                            if !info.org.is_empty() && info.org != info.isp {
                                                ui.horizontal(|ui| {
                                                    ui.label(RichText::new("Org:").color(label_col).font(small.clone()));
                                                    ui.label(RichText::new(&info.org)
                                                        .color(value_col).font(small.clone()));
                                                });
                                            }
                                            // AS number/name
                                            if !info.as_name.is_empty() {
                                                ui.horizontal(|ui| {
                                                    ui.label(RichText::new("AS:").color(label_col).font(small.clone()));
                                                    ui.label(RichText::new(&info.as_name)
                                                        .color(value_col).font(small.clone()));
                                                });
                                            }
                                            // Coordinates (lat, lon)
                                            if info.lat != 0.0 || info.lon != 0.0 {
                                                ui.horizontal(|ui| {
                                                    ui.label(RichText::new("Coords:").color(label_col).font(small.clone()));
                                                    ui.label(RichText::new(format!("{:.4}, {:.4}", info.lat, info.lon))
                                                        .color(value_col).font(small.clone()));
                                                });
                                            }
                                            // Timezone
                                            if !info.timezone.is_empty() {
                                                ui.horizontal(|ui| {
                                                    ui.label(RichText::new("Timezone:").color(label_col).font(small.clone()));
                                                    ui.label(RichText::new(&info.timezone)
                                                        .color(value_col).font(small.clone()));
                                                });
                                            }
                                            // ZIP code (mostly US; skip if empty)
                                            if !info.zip.is_empty() {
                                                ui.horizontal(|ui| {
                                                    ui.label(RichText::new("ZIP:").color(label_col).font(small.clone()));
                                                    ui.label(RichText::new(&info.zip)
                                                        .color(value_col).font(small.clone()));
                                                });
                                            }
                                        }
                                    }
                                }

                                // System-wide route state: tun2proxy + TUN (Wintun on Windows)
                                // routes system traffic through Tor SOCKS5. Available on Windows
                                // and Linux (pkexec on Linux).
                                {
                                    ui.add_space(8.0);
                                    ui.separator();
                                    ui.add_space(4.0);
                                    ui.label(
                                        RichText::new("System-Wide Route:")
                                            .font(FontId::new(10.0, FontFamily::Proportional))
                                            .color(Color32::from_rgb(170, 170, 170)),
                                    );
                                    if is_tor_connected {
                                        if self.snapshot.tor_system_route_active {
                                            let active_label = if cfg!(target_os = "windows") {
                                                "ACTIVE (0.0.0.0/0 via Wintun)"
                                            } else {
                                                "ACTIVE (0.0.0.0/0 via TUN)"
                                            };
                                            ui.label(
                                                RichText::new(active_label)
                                                    .color(Color32::from_rgb(0, 255, 127))
                                                    .strong()
                                                    .font(FontId::new(10.0, FontFamily::Proportional)),
                                            );
                                            if ui
                                                .add(
                                                    egui::Button::new(
                                                        RichText::new("Disable").color(Color32::BLACK),
                                                    )
                                                    .fill(Color32::from_rgb(180, 60, 60))
                                                    .min_size(Vec2::new(ui.available_width().min(panel_w), 24.0)),
                                                )
                                                .clicked()
                                            {
                                                let _ = self
                                                    .command_tx
                                                    .send(ClientCommand::RemoveTorSystemRoute);
                                            }
                                        } else {
                                            ui.label(
                                                RichText::new("OFF (Tor is SOCKS5 only)")
                                                    .color(Color32::from_rgb(180, 180, 180))
                                                    .font(FontId::new(10.0, FontFamily::Proportional)),
                                            );
                                            let btn_label = if self.elevated {
                                                if narrow {
                                                    "Enable System Route"
                                                } else {
                                                    "Enable System-Wide Routing"
                                                }
                                            } else if narrow {
                                                "Enable (Admin)"
                                            } else {
                                                if cfg!(target_os = "windows") {
                                                    "Enable (UAC / Admin)"
                                                } else {
                                                    "Enable (Admin)"
                                                }
                                            };
                                            if ui
                                                .add(
                                                    egui::Button::new(
                                                        RichText::new(btn_label).color(Color32::WHITE),
                                                    )
                                                    .fill(Color32::from_rgb(138, 43, 226))
                                                    .min_size(Vec2::new(ui.available_width().min(panel_w), 26.0)),
                                                )
                                                .clicked()
                                            {
                                                let _ = self
                                                    .command_tx
                                                    .send(ClientCommand::ApplyTorSystemRoute);
                                            }
                                        }
                                    } else if is_tor_connecting {
                                        ui.label(
                                            RichText::new("waiting for SOCKS5...")
                                                .color(Color32::from_rgb(150, 150, 150))
                                                .font(FontId::new(10.0, FontFamily::Proportional)),
                                        );
                                    } else {
                                        ui.label(
                                            RichText::new("—")
                                                .color(Color32::from_rgb(120, 120, 120))
                                                .font(FontId::new(10.0, FontFamily::Proportional)),
                                        );
                                    }
                                    if is_tor_connected
                                        && !self.snapshot.tor_system_route_active
                                        && self.tor_isolation_mode == "system"
                                    {
                                        ui.add_space(2.0);
                                        let help = if self.elevated {
                                            "Elevated but tunnel is OFF — click Enable System-Wide Routing (or Disconnect and Connect again)."
                                        } else {
                                            if cfg!(target_os = "windows") {
                                                "Not running as Administrator. Click Connect (or Enable) and accept the UAC prompt so Wintun can route every app through Tor."
                                            } else {
                                                "Not running as Administrator. Click Enable and authenticate via pkexec so TUN can route every app through Tor."
                                            }
                                        };
                                        ui.label(
                                            RichText::new(help)
                                                .font(FontId::new(9.0, FontFamily::Proportional))
                                                .color(Color32::from_rgb(140, 140, 140)),
                                        );
                                    }

                                    // Tor isolation mode: full system vs SOCKS5 for selected apps.
                                    ui.add_space(8.0);
                                    ui.separator();
                                    ui.add_space(4.0);
                                    ui.label(
                                        RichText::new("Isolation mode")
                                            .color(Color32::WHITE)
                                            .strong()
                                            .font(FontId::new(11.0, FontFamily::Proportional)),
                                    );
                                    ui.horizontal(|ui| {
                                        let system_sel = self.tor_isolation_mode == "system";
                                        let system_hover = if cfg!(target_os = "windows") {
                                            "Wintun routes all traffic through Tor SOCKS5 (needs Admin)."
                                        } else {
                                            "TUN routes all traffic through Tor SOCKS5 (needs Admin)."
                                        };
                                        if ui
                                            .selectable_label(system_sel, "Whole PC (system VPN)")
                                            .on_hover_text(system_hover)
                                            .clicked()
                                        {
                                            self.tor_isolation_mode = String::from("system");
                                            crate::db::set_tor_isolation_mode("system");
                                            let _ = self.command_tx.send(
                                                ClientCommand::SetTorIsolationMode(String::from(
                                                    "system",
                                                )),
                                            );
                                        }
                                        let apps_sel = self.tor_isolation_mode == "apps";
                                        if ui
                                            .selectable_label(apps_sel, "Selected apps (SOCKS5)")
                                            .on_hover_text(
                                                "Tor stays local SOCKS5 only. Launch chosen apps through the proxy (single or multiple).",
                                            )
                                            .clicked()
                                        {
                                            self.tor_isolation_mode = String::from("apps");
                                            crate::db::set_tor_isolation_mode("apps");
                                            let _ = self.command_tx.send(
                                                ClientCommand::SetTorIsolationMode(String::from(
                                                    "apps",
                                                )),
                                            );
                                            // If system route is active, offer disable when switching to apps.
                                            if self.snapshot.tor_system_route_active {
                                                let _ = self
                                                    .command_tx
                                                    .send(ClientCommand::RemoveTorSystemRoute);
                                            }
                                        }
                                    });

                                    if self.tor_isolation_mode == "apps" {
                                        ui.add_space(6.0);
                                        if let Some(port) = self.snapshot.tor_socks_port {
                                            ui.label(
                                                RichText::new(format!(
                                                    "Apps use SOCKS5 127.0.0.1:{port} (DNS via socks5h when supported)."
                                                ))
                                                .color(Color32::from_rgb(200, 180, 255))
                                                .font(FontId::new(
                                                    10.0,
                                                    FontFamily::Proportional,
                                                )),
                                            );
                                        } else {
                                            ui.label(
                                                RichText::new(
                                                    "Connect Tor first so SOCKS5 is listening, then launch apps.",
                                                )
                                                .color(Color32::from_rgb(180, 180, 180))
                                                .font(FontId::new(
                                                    10.0,
                                                    FontFamily::Proportional,
                                                )),
                                            );
                                        }
                                        ui.add_space(4.0);
                                        ui.horizontal(|ui| {
                                            if ui
                                                .add(
                                                    egui::Button::new(
                                                        RichText::new("+ Add app")
                                                            .color(Color32::WHITE),
                                                    )
                                                    .fill(Color32::from_rgb(90, 50, 160)),
                                                )
                                                .clicked()
                                            {
                                                #[cfg(not(target_os = "android"))]
                                                if let Some(path) = rfd::FileDialog::new()
                                                    .add_filter("Executable", &["exe"])
                                                    .pick_file()
                                                {
                                                    let name = path
                                                        .file_stem()
                                                        .unwrap_or_default()
                                                        .to_string_lossy()
                                                        .to_string();
                                                    let path_s = path.to_string_lossy().to_string();
                                                    if crate::db::add_tor_isolated_app(
                                                        &name, &path_s,
                                                    )
                                                    .is_ok()
                                                    {
                                                        self.tor_isolated_apps = crate::db::list_tor_isolated_apps()
                                                            .unwrap_or_default();
                                                    }
                                                }
                                                #[cfg(target_os = "android")]
                                                {
                                                    self.snapshot.notice = Some(String::from(
                                                        "App isolation picker is desktop-only on this build.",
                                                    ));
                                                }
                                            }
                                        });
                                        let mut remove_app = None;
                                        for app in &self.tor_isolated_apps {
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    RichText::new(&app.name)
                                                        .color(Color32::WHITE)
                                                        .font(FontId::new(
                                                            11.0,
                                                            FontFamily::Proportional,
                                                        )),
                                                );
                                                ui.with_layout(
                                                    Layout::right_to_left(Align::Center),
                                                    |ui| {
                                                        if ui
                                                            .small_button("🗑")
                                                            .clicked()
                                                        {
                                                            remove_app = Some(app.id);
                                                        }
                                                        let can_launch =
                                                            is_tor_connected
                                                                || self.snapshot.tor_socks_port.is_some();
                                                        if ui
                                                            .add_enabled(
                                                                can_launch,
                                                                egui::Button::new(
                                                                    RichText::new("Launch via Tor")
                                                                        .color(Color32::BLACK)
                                                                        .font(FontId::new(
                                                                            10.0,
                                                                            FontFamily::Proportional,
                                                                        )),
                                                                )
                                                                .fill(Color32::from_rgb(
                                                                    138, 43, 226,
                                                                )),
                                                            )
                                                            .clicked()
                                                        {
                                                            let _ = self.command_tx.send(
                                                                ClientCommand::LaunchTorIsolatedApp(
                                                                    app.id,
                                                                ),
                                                            );
                                                        }
                                                    },
                                                );
                                            });
                                            ui.label(
                                                RichText::new(&app.path)
                                                    .color(Color32::from_rgb(120, 120, 120))
                                                    .font(FontId::new(
                                                        9.0,
                                                        FontFamily::Proportional,
                                                    )),
                                            );
                                        }
                                        if let Some(id) = remove_app {
                                            let _ = crate::db::delete_tor_isolated_app(id);
                                            self.tor_isolated_apps = crate::db::list_tor_isolated_apps()
                                                .unwrap_or_default();
                                        }
                                        if self.tor_isolated_apps.is_empty() {
                                            ui.label(
                                                RichText::new(
                                                    "No apps yet. Add browsers or tools to isolate through Tor SOCKS5.",
                                                )
                                                .color(Color32::from_rgb(140, 140, 140))
                                                .font(FontId::new(
                                                    10.0,
                                                    FontFamily::Proportional,
                                                )),
                                            );
                                        }
                                        ui.add_space(2.0);
                                        ui.label(
                                            RichText::new(
                                                "Note: proxy-aware apps honor ALL_PROXY / SOCKS. Browsers are launched with --proxy-server when detected. Full force-proxy for every binary needs system VPN mode.",
                                            )
                                            .color(Color32::from_rgb(110, 110, 110))
                                            .font(FontId::new(9.0, FontFamily::Proportional)),
                                        );
                                    }
                                }
                            });
                        });

                        ui.add_space(16.0);
                        section_title(
                            ui,
                            "VPN Protocol",
                            Some(Color32::from_rgb(0, 255, 127)),
                        );
                        ui.label(
                            RichText::new(
                                "Choose a protocol, then import a profile and Connect.",
                            )
                            .font(FontId::new(12.0, FontFamily::Proportional))
                            .color(Color32::from_rgb(150, 150, 150)),
                        );
                        ui.add_space(6.0);

                        // Cross-protocol busy banner (switching dropdown does not disconnect).
                        let busy_banner = self.snapshot.active_connection.as_ref().and_then(|a| {
                            let phase_busy = matches!(
                                a.phase,
                                ConnectionPhase::Connecting | ConnectionPhase::Connected
                            );
                            if !phase_busy || a.server_id == "tor_local" {
                                return None;
                            }
                            let active_ui = match a.protocol {
                                VpnProtocol::OpenVPN => VpnUiProtocol::OpenVPN,
                                VpnProtocol::WireGuard => VpnUiProtocol::WireGuard,
                                VpnProtocol::Pptp => VpnUiProtocol::Pptp,
                                VpnProtocol::Outline => VpnUiProtocol::Outline,
                            };
                            if active_ui == self.selected_vpn_protocol {
                                None
                            } else {
                                Some(format!(
                                    "{} is connected — disconnect first to use {}.",
                                    active_ui.display_name(),
                                    self.selected_vpn_protocol.display_name()
                                ))
                            }
                        });
                        let combo_w = (panel_w - 90.0).clamp(120.0, 280.0);
                        if crate::protocols::protocol_combo(
                            ui,
                            &mut self.selected_vpn_protocol,
                            combo_w,
                            busy_banner.as_deref(),
                        ) {
                            crate::db::set_selected_vpn_protocol(self.selected_vpn_protocol);
                        }
                        ui.add_space(8.0);

                        // ---- Protocol-specific body ----
                        if self.selected_vpn_protocol == VpnUiProtocol::OpenVPN {
                        let ovpn_conn = self
                            .snapshot
                            .active_connection
                            .as_ref()
                            .filter(|a| a.server_id.starts_with("ovpn_"));
                        let is_ovpn_connecting = ovpn_conn
                            .map(|a| a.phase == vpn_suite_core::model::ConnectionPhase::Connecting)
                            .unwrap_or(false);
                        let is_ovpn_connected = ovpn_conn
                            .map(|a| a.phase == vpn_suite_core::model::ConnectionPhase::Connected)
                            .unwrap_or(false);
                        let is_ovpn_busy = is_ovpn_connecting || is_ovpn_connected;

                        // Connect card: toggle + server picker (mirrors Tor card flow).
                        egui::Frame::none()
                            .fill(Color32::from_rgb(13, 13, 13))
                            .stroke(Stroke::new(1.0, Color32::from_rgb(0, 180, 120)))
                            .rounding(8.0)
                            .inner_margin(Margin::symmetric(if narrow { 8.0 } else { 10.0 }, 8.0))
                            .show(ui, |ui| {
                                ui.set_max_width(ui.available_width());
                                let ovpn_title = RichText::new(if narrow {
                                    "OpenVPN"
                                } else {
                                    "OpenVPN tunnel"
                                })
                                .color(Color32::WHITE)
                                .strong()
                                .font(FontId::new(
                                    if narrow { 14.0 } else { 15.0 },
                                    FontFamily::Proportional,
                                ));
                                let mut ovpn_actions = |ui: &mut egui::Ui| {
                                    if is_ovpn_busy {
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    RichText::new("Disconnect")
                                                        .color(Color32::WHITE),
                                                )
                                                .fill(Color32::from_rgb(48, 48, 48))
                                                .min_size(Vec2::new(
                                                    if narrow {
                                                        ui.available_width()
                                                    } else {
                                                        0.0
                                                    },
                                                    26.0,
                                                )),
                                            )
                                            .clicked()
                                        {
                                            let _ =
                                                self.command_tx.send(ClientCommand::Disconnect);
                                        }
                                    } else {
                                        let can_connect = self.selected_ovpn_id.is_some()
                                            && !self.ovpn_configs.is_empty();
                                        if ui
                                            .add_enabled(
                                                can_connect,
                                                egui::Button::new(
                                                    RichText::new("Connect")
                                                        .color(Color32::BLACK),
                                                )
                                                .fill(Color32::from_rgb(0, 255, 127))
                                                .min_size(Vec2::new(
                                                    if narrow {
                                                        ui.available_width()
                                                    } else {
                                                        0.0
                                                    },
                                                    26.0,
                                                )),
                                            )
                                            .clicked()
                                        {
                                            if !is_ovpn_connecting {
                                                self.ovpn_bootstrap_started =
                                                    Some(ui.input(|i| i.time));
                                            }
                                            self.request_openvpn_connect(self.selected_ovpn_id);
                                        }
                                    }
                                };
                                if narrow {
                                    ui.label(ovpn_title);
                                    ui.add_space(4.0);
                                    ovpn_actions(ui);
                                } else {
                                    ui.horizontal(|ui| {
                                        ui.label(ovpn_title);
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                ovpn_actions(ui);
                                            },
                                        );
                                    });
                                }
                                let ovpn_green = Color32::from_rgb(0, 255, 127);
                                let ovpn_disconnecting = self.snapshot.op_progress_kind.as_deref()
                                    == Some("disconnect")
                                    && self.snapshot.op_progress > 0.0
                                    && self.snapshot.op_progress < 1.0;
                                if is_ovpn_connecting || ovpn_disconnecting {
                                    ui.add_space(10.0);
                                    if self.ovpn_bootstrap_started.is_none() {
                                        self.ovpn_bootstrap_started =
                                            Some(ui.input(|i| i.time));
                                    }
                                    let started = self
                                        .ovpn_bootstrap_started
                                        .unwrap_or(ui.input(|i| i.time));
                                    let elapsed =
                                        (ui.input(|i| i.time) - started).max(0.0) as f32;
                                    let kind = if ovpn_disconnecting {
                                        "disconnect"
                                    } else {
                                        "connect"
                                    };
                                    let (progress, label) = self.real_op_progress(kind, elapsed);
                                    let label = if label == "Starting…" && !ovpn_disconnecting {
                                        String::from(
                                            "Establishing OpenVPN tunnel… routes + adapter",
                                        )
                                    } else {
                                        label
                                    };
                                    ui.label(
                                        RichText::new(label)
                                        .color(ovpn_green)
                                        .font(FontId::new(13.0, FontFamily::Proportional))
                                        .strong(),
                                    );
                                    ui.add_space(6.0);
                                    let bar_width = (ui.available_width() - 4.0).max(80.0);
                                    paint_connect_progress_bar(
                                        ui, progress, bar_width, ovpn_green,
                                    );
                                    ui.add_space(2.0);
                                    ui.label(
                                        RichText::new(format!("{:.0}%", progress * 100.0))
                                            .color(Color32::from_rgb(140, 220, 170))
                                            .font(FontId::new(12.0, FontFamily::Proportional)),
                                    );
                                    ui.ctx().request_repaint();
                                } else if is_ovpn_connected {
                                    ui.add_space(6.0);
                                    ui.label(
                                        RichText::new("OpenVPN system tunnel ACTIVE")
                                            .color(ovpn_green)
                                            .font(FontId::new(12.0, FontFamily::Proportional))
                                            .strong(),
                                    );
                                    ui.label(
                                        RichText::new(
                                            "Default route via OpenVPN. Your IP refreshes automatically.",
                                        )
                                        .color(Color32::from_rgb(160, 160, 160))
                                        .font(FontId::new(10.0, FontFamily::Proportional)),
                                    );
                                }
                                ui.add_space(6.0);
                                ui.label(
                                    RichText::new("Select server:")
                                        .color(Color32::from_rgb(170, 170, 170))
                                        .font(FontId::new(11.0, FontFamily::Proportional)),
                                );
                                let selected_label = self
                                    .selected_ovpn_id
                                    .and_then(|id| {
                                        self.ovpn_configs.iter().find(|c| c.id == id)
                                    })
                                    .map(|c| {
                                        let loc = c
                                            .country
                                            .as_deref()
                                            .or(c.country_code.as_deref())
                                            .unwrap_or("");
                                        if loc.is_empty() {
                                            c.name.clone()
                                        } else {
                                            format!("{} · {}", c.name, loc)
                                        }
                                    })
                                    .unwrap_or_else(|| String::from("(add a .ovpn profile)"));
                                let combo_w = (ui.available_width() - 4.0)
                                    .clamp(80.0, panel_w.max(80.0));
                                egui::ComboBox::from_id_salt("ovpn_server_select")
                                    .selected_text(selected_label)
                                    .width(combo_w)
                                    .show_ui(ui, |ui| {
                                        for c in &self.ovpn_configs {
                                            let label = {
                                                let loc = c
                                                    .country
                                                    .as_deref()
                                                    .or(c.country_code.as_deref())
                                                    .unwrap_or("");
                                                if loc.is_empty() {
                                                    c.name.clone()
                                                } else {
                                                    format!("{} · {}", c.name, loc)
                                                }
                                            };
                                            if ui
                                                .selectable_label(
                                                    self.selected_ovpn_id == Some(c.id),
                                                    label,
                                                )
                                                .clicked()
                                            {
                                                self.selected_ovpn_id = Some(c.id);
                                                crate::db::set_selected_ovpn_id(Some(c.id));
                                            }
                                        }
                                    });
                                if let Some(id) = self.selected_ovpn_id {
                                    if let Some(sel) =
                                        self.ovpn_configs.iter().find(|c| c.id == id)
                                    {
                                        ui.add_space(4.0);
                                        let info = sel.location_info();
                                        let label_col = Color32::from_rgb(140, 140, 140);
                                        let value_col = Color32::from_rgb(210, 210, 210);
                                        let small = FontId::new(10.0, FontFamily::Proportional);
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                RichText::new("Endpoint:")
                                                    .color(label_col)
                                                    .font(small.clone()),
                                            );
                                            ui.label(
                                                RichText::new(sel.endpoint_label())
                                                    .color(Color32::from_rgb(0, 255, 127))
                                                    .font(small.clone()),
                                            );
                                            if let Some(proto) = sel.proto.as_ref() {
                                                ui.label(
                                                    RichText::new(format!("({proto})"))
                                                        .color(label_col)
                                                        .font(small.clone()),
                                                );
                                            }
                                        });
                                        if !info.ip.is_empty() && Some(info.ip.as_str()) != sel.remote_host.as_deref()
                                        {
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    RichText::new("Resolved IP:")
                                                        .color(label_col)
                                                        .font(small.clone()),
                                                );
                                                ui.label(
                                                    RichText::new(&info.ip)
                                                        .color(value_col)
                                                        .font(small.clone()),
                                                );
                                            });
                                        }
                                        if !info.country_code.is_empty() || !info.country.is_empty()
                                        {
                                            ui.horizontal(|ui| {
                                                if !info.country_code.is_empty() {
                                                    show_flag(
                                                        ui,
                                                        &info.country_code,
                                                        Vec2::new(28.0, 20.0),
                                                    );
                                                }
                                                let name = if info.country.is_empty() {
                                                    info.country_code.clone()
                                                } else {
                                                    info.country.clone()
                                                };
                                                ui.label(
                                                    RichText::new(name)
                                                        .color(Color32::WHITE)
                                                        .font(small.clone()),
                                                );
                                            });
                                        }
                                        let mut loc_parts = Vec::new();
                                        if !info.city.is_empty() {
                                            loc_parts.push(info.city.clone());
                                        }
                                        if !info.region.is_empty() {
                                            loc_parts.push(info.region.clone());
                                        }
                                        if !loc_parts.is_empty() {
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    RichText::new("Location:")
                                                        .color(label_col)
                                                        .font(small.clone()),
                                                );
                                                ui.label(
                                                    RichText::new(loc_parts.join(", "))
                                                        .color(value_col)
                                                        .font(small.clone()),
                                                );
                                            });
                                        }
                                        if !info.isp.is_empty() {
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    RichText::new("ISP:")
                                                        .color(label_col)
                                                        .font(small.clone()),
                                                );
                                                ui.label(
                                                    RichText::new(&info.isp)
                                                        .color(value_col)
                                                        .font(small.clone()),
                                                );
                                            });
                                        }
                                        if let Some(cipher) = sel.cipher.as_ref() {
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    RichText::new("Cipher:")
                                                        .color(label_col)
                                                        .font(small.clone()),
                                                );
                                                ui.label(
                                                    RichText::new(cipher)
                                                        .color(value_col)
                                                        .font(small.clone()),
                                                );
                                            });
                                        }
                                    }
                                }
                            });

                        ui.add_space(10.0);
                        // Drop / click-to-browse import zone.
                        let drop_resp = egui::Frame::none()
                            .fill(Color32::from_rgb(18, 22, 20))
                            .stroke(Stroke::new(1.5, Color32::from_rgb(0, 180, 120)))
                            .rounding(8.0)
                            .inner_margin(Margin::symmetric(10.0, 12.0))
                            .show(ui, |ui| {
                                ui.set_max_width(ui.available_width());
                                ui.set_min_height(52.0);
                                ui.vertical_centered(|ui| {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new("Drop .ovpn here  ·  or click to browse")
                                                .color(Color32::from_rgb(0, 220, 150))
                                                .strong()
                                                .font(FontId::new(12.0, FontFamily::Proportional)),
                                        )
                                        .wrap(),
                                    );
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new("Import remote + location details")
                                                .color(Color32::from_rgb(140, 140, 140))
                                                .font(FontId::new(11.0, FontFamily::Proportional)),
                                        )
                                        .wrap(),
                                    );
                                });
                            })
                            .response
                            .interact(Sense::click());
                        if drop_resp.clicked() {
                            #[cfg(not(target_os = "android"))]
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("OpenVPN profile", &["ovpn"])
                                .pick_file()
                            {
                                self.import_ovpn_path(&path);
                            }
                            #[cfg(target_os = "android")]
                            {
                                self.snapshot.notice = Some(String::from(
                                    "File picker is desktop-only — paste the .ovpn contents on Android.",
                                ));
                            }
                        }

                        ui.add_space(10.0);
                        let mut delete_idx = None;
                        for (idx, config) in self.ovpn_configs.iter().enumerate() {
                            let is_sel = self.selected_ovpn_id == Some(config.id);
                            let stroke = if is_sel {
                                Stroke::new(1.5, Color32::from_rgb(0, 200, 120))
                            } else {
                                Stroke::new(1.0, Color32::from_rgb(31, 31, 31))
                            };
                            egui::Frame::none()
                                .fill(Color32::from_rgb(13, 13, 13))
                                .stroke(stroke)
                                .rounding(8.0)
                                .inner_margin(Margin::symmetric(10.0, 8.0))
                                .show(ui, |ui| {
                                    ui.set_max_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        if is_sel {
                                            ui.label(
                                                RichText::new("●")
                                                    .color(Color32::from_rgb(0, 255, 127)),
                                            );
                                        }
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(&config.name)
                                                    .color(Color32::WHITE)
                                                    .strong(),
                                            )
                                            .wrap(),
                                        );
                                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                            if ui
                                                .button(
                                                    RichText::new("🗑")
                                                        .color(Color32::from_rgb(180, 180, 180)),
                                                )
                                                .on_hover_text("Remove profile")
                                                .clicked()
                                            {
                                                let _ = crate::db::delete_ovpn_config(config.id);
                                                // Also drop written profile file if present.
                                                if let Ok(paths) =
                                                    vpn_suite_core::app_paths::client_paths()
                                                {
                                                    let p = paths
                                                        .profiles_dir
                                                        .join(format!("ovpn_{}.ovpn", config.id));
                                                    let _ = std::fs::remove_file(p);
                                                }
                                                delete_idx = Some(idx);
                                            }
                                            if ui
                                                .add(
                                                    egui::Button::new(
                                                        RichText::new(if is_sel {
                                                            "Selected"
                                                        } else {
                                                            "Select"
                                                        })
                                                        .color(Color32::BLACK),
                                                    )
                                                    .fill(if is_sel {
                                                        Color32::from_rgb(0, 180, 100)
                                                    } else {
                                                        Color32::from_rgb(80, 80, 80)
                                                    }),
                                                )
                                                .clicked()
                                            {
                                                self.selected_ovpn_id = Some(config.id);
                                                crate::db::set_selected_ovpn_id(Some(config.id));
                                            }
                                        });
                                    });
                                    ui.add_space(4.0);
                                    let small = FontId::new(10.0, FontFamily::Proportional);
                                    ui.label(
                                        RichText::new(config.endpoint_label())
                                            .color(Color32::from_rgb(0, 255, 127))
                                            .font(small.clone()),
                                    );
                                    let info = config.location_info();
                                    if !info.country.is_empty() || !info.city.is_empty() {
                                        let mut parts = Vec::new();
                                        if !info.city.is_empty() {
                                            parts.push(info.city);
                                        }
                                        if !info.region.is_empty() {
                                            parts.push(info.region);
                                        }
                                        if !info.country.is_empty() {
                                            parts.push(info.country);
                                        } else if !info.country_code.is_empty() {
                                            parts.push(info.country_code);
                                        }
                                        ui.label(
                                            RichText::new(parts.join(", "))
                                                .color(Color32::from_rgb(180, 180, 180))
                                                .font(small),
                                        );
                                    } else if config.country_code.is_none()
                                        && config.resolved_ip.is_some()
                                    {
                                        ui.label(
                                            RichText::new("Looking up location…")
                                                .color(Color32::from_rgb(180, 180, 180))
                                                .font(small),
                                        );
                                    }
                                });
                            ui.add_space(8.0);
                        }
                        if let Some(idx) = delete_idx {
                            let removed = self.ovpn_configs.remove(idx);
                            if self.selected_ovpn_id == Some(removed.id) {
                                self.selected_ovpn_id =
                                    self.ovpn_configs.first().map(|c| c.id);
                                crate::db::set_selected_ovpn_id(self.selected_ovpn_id);
                            }
                        }
                        } // end OpenVPN panel
                        else if self.selected_vpn_protocol == VpnUiProtocol::WireGuard {
                            self.render_wireguard_panel(ui, narrow, panel_w);
                        } else if self.selected_vpn_protocol == VpnUiProtocol::Pptp {
                            self.render_pptp_panel(ui, narrow, panel_w);
                        } else if self.selected_vpn_protocol == VpnUiProtocol::Outline {
                            self.render_outline_panel(ui, narrow, panel_w);
                        }

                        // --- Hosting (collapsible) ---
                        ui.add_space(16.0);
                        render_hosting_section(
                            ui,
                            &self.snapshot,
                            &self.command_tx,
                            &mut open_server_settings,
                            &mut self.host_section_open,
                            panel_w,
                        );

                        // --- Network (collapsible, closed by default) ---
                        ui.add_space(12.0);
                        render_network_section(ui, &mut self.net_section_open, panel_w);

                        ui.add_space(12.0);
                    }); // ScrollArea
            }); // SidePanel

        egui::CentralPanel::default().show(ctx, |ui| {
            let rect = ui.available_rect_before_wrap();
            let center = crate::globe::renderer::GlobeRenderer::globe_center(rect);
            let radius = rect.width().min(rect.height()) * 0.42 * self.globe_renderer.zoom;
            let interact_radius = radius + 36.0;
            let globe_rect = egui::Rect::from_center_size(
                center,
                egui::vec2(interact_radius * 2.0, interact_radius * 2.0),
            )
            .intersect(rect);

            let is_hovered = ui.rect_contains_pointer(globe_rect);
            // Guard stable_dt against 0 / NaN on first frame or odd Windows timers.
            let raw_dt = ui.input(|i| i.stable_dt);
            let dt = if raw_dt.is_finite() && raw_dt > 0.0 {
                raw_dt.clamp(1.0 / 240.0, 1.0 / 20.0)
            } else {
                0.016
            };

            // Primary button drag only (avoid accidental orbit on other buttons).
            if is_hovered && ui.input(|i| i.pointer.primary_pressed()) {
                self.globe_renderer.is_dragging = true;
                self.globe_renderer.velocity_x = 0.0;
                self.globe_renderer.velocity_y = 0.0;
            }
            if ui.input(|i| !i.pointer.primary_down()) {
                self.globe_renderer.is_dragging = false;
            }

            let scroll_delta = if is_hovered {
                ui.input(|i| i.smooth_scroll_delta)
            } else {
                egui::Vec2::ZERO
            };
            let zoom_delta = if is_hovered {
                ui.input(|i| i.zoom_delta())
            } else {
                1.0
            };

            // Orbit: drag/scroll moves the globe with the pointer (trackball feel).
            // Sensitivity tuned for 60–144 Hz; velocity blending lives in apply_orbit_delta.
            const ORBIT_DRAG: f32 = 0.0065;
            const ORBIT_SCROLL: f32 = 0.0035;

            if scroll_delta.x != 0.0 || scroll_delta.y != 0.0 {
                self.globe_renderer.apply_orbit_delta(
                    scroll_delta.x * ORBIT_SCROLL,
                    scroll_delta.y * ORBIT_SCROLL,
                    dt,
                );
                ui.ctx().request_repaint();
            }
            if zoom_delta != 1.0 {
                // Smooth zoom around a gentle exponential step.
                let z = self.globe_renderer.zoom * zoom_delta.powf(0.85);
                self.globe_renderer.zoom = z.clamp(0.55, 4.5);
                ui.ctx().request_repaint();
            }

            if self.globe_renderer.is_dragging {
                let delta = ui.input(|i| i.pointer.delta());
                if delta.x != 0.0 || delta.y != 0.0 {
                    self.globe_renderer.apply_orbit_delta(
                        delta.x * ORBIT_DRAG,
                        delta.y * ORBIT_DRAG,
                        dt,
                    );
                }
                ui.ctx().request_repaint();
            } else {
                self.globe_renderer.update(dt);
            }
            
            // Globe pin always follows **Your IP** (`local_ip_info`) — same source
            // as the side pane and pan animation. Fall back to active tunnel
            // exit metadata only when Your IP has not resolved yet.
            let beacon = if let Some(info) = self.snapshot.local_ip_info.as_ref() {
                let (lat, lon) = if info.lat != 0.0 || info.lon != 0.0 {
                    (Some(info.lat), Some(info.lon))
                } else if !info.country_code.is_empty() {
                    self.globe_renderer
                        .centroids
                        .get(&info.country_code)
                        .map(|c| (Some(c.lat), Some(c.lng)))
                        .unwrap_or((None, None))
                } else {
                    (None, None)
                };
                let display = if !info.city.is_empty() && !info.country.is_empty() {
                    format!("{}, {}", info.city, info.country)
                } else if !info.country.is_empty() {
                    info.country.clone()
                } else if !info.country_code.is_empty() {
                    info.country_code.clone()
                } else {
                    String::from("Your location")
                };
                let sid = self
                    .snapshot
                    .active_connection
                    .as_ref()
                    .map(|a| a.server_id.clone())
                    .unwrap_or_else(|| String::from("local_ip"));
                crate::globe::renderer::ActiveBeacon {
                    server_id: Some(sid),
                    lat,
                    lon,
                    country_code: if info.country_code.is_empty() {
                        None
                    } else {
                        Some(info.country_code.clone())
                    },
                    display_name: Some(display),
                }
            } else if let Some(a) = self.snapshot.active_connection.as_ref() {
                let (lat, lon) = a
                    .tor_exit_info
                    .as_ref()
                    .map(|t| (t.lat, t.lon))
                    .unwrap_or((0.0, 0.0));
                let (lat, lon) = if lat != 0.0 || lon != 0.0 {
                    (Some(lat), Some(lon))
                } else if let Some(cc) = a.country_code.as_deref() {
                    self.globe_renderer
                        .centroids
                        .get(cc)
                        .map(|c| (Some(c.lat), Some(c.lng)))
                        .unwrap_or((None, None))
                } else {
                    (None, None)
                };
                crate::globe::renderer::ActiveBeacon {
                    server_id: Some(a.server_id.clone()),
                    lat,
                    lon,
                    country_code: a.country_code.clone(),
                    display_name: Some(a.server_name.clone()),
                }
            } else {
                crate::globe::renderer::ActiveBeacon::default()
            };
            let clicked = self.globe_renderer.paint(ui, rect, &self.snapshot.servers, &beacon);
            if let Some(server_id) = clicked {
                if let Some(server) = self.snapshot.servers.iter().find(|s| s.server_id == server_id) {
                    if server.has_password {
                        self.password_dialog = Some(PasswordDialog {
                            server_id: server.server_id.clone(),
                            server_name: server.name.clone(),
                            endpoint: server.endpoint.clone(),
                            value: String::new(),
                        });
                    } else {
                        let _ = self.command_tx.send(ClientCommand::Connect {
                            server_id: server.server_id.clone(),
                            endpoint: server.endpoint.clone(),
                            server_name: server.name.clone(),
                            password: None,
                            protocol: vpn_suite_core::model::VpnProtocol::WireGuard,
                        });
                    }
                }
            }

            // Toolbar under the globe (no discovery notice / Available Nodes list).
            ui.add_space(10.0);
            render_notice(ui, self.snapshot.notice.as_deref());

            ui.horizontal_wrapped(|ui| {
                ui.add_sized(
                    [200.0, 32.0],
                    egui::TextEdit::singleline(&mut self.manual_host_input)
                        .hint_text("Server IP or host")
                        .text_color(Color32::WHITE),
                );
                if ui
                    .add_sized(
                        [80.0, 32.0],
                        egui::Button::new(RichText::new("Add").color(Color32::BLACK))
                            .fill(Color32::from_rgb(0, 255, 127)),
                    )
                    .clicked()
                {
                    let host = self.manual_host_input.trim().to_owned();
                    if !host.is_empty() {
                        let _ = self.command_tx.send(ClientCommand::AddHost(host));
                        self.manual_host_input.clear();
                    }
                }
                if ui
                    .add_sized(
                        [70.0, 32.0],
                        egui::Button::new(RichText::new("Refresh").color(Color32::WHITE))
                            .fill(Color32::from_rgb(26, 26, 26)),
                    )
                    .clicked()
                {
                    let _ = self.command_tx.send(ClientCommand::RefreshNow);
                }
                if ui
                    .add_sized(
                        [70.0, 32.0],
                        egui::Button::new(RichText::new("Tunnel").color(Color32::BLACK))
                            .fill(Color32::from_rgb(210, 210, 210)),
                    )
                    .clicked()
                {
                    let _ = self.command_tx.send(ClientCommand::ApplyTunnel);
                }
                if ui
                    .add_sized(
                        [80.0, 32.0],
                        egui::Button::new(RichText::new("Disconnect").color(Color32::WHITE))
                            .fill(Color32::from_rgb(42, 45, 53)),
                    )
                    .clicked()
                {
                    let _ = self.command_tx.send(ClientCommand::Disconnect);
                }
                if ui
                    .add_sized(
                        [90.0, 32.0],
                        egui::Button::new(RichText::new("Remove Tunnel").color(Color32::WHITE))
                            .fill(Color32::from_rgb(26, 26, 26)),
                    )
                    .clicked()
                {
                    let _ = self.command_tx.send(ClientCommand::RemoveTunnel);
                }
            });
        });

        if open_server_settings {
            self.open_server_settings();
        }

        if let Some(dialog) = &mut self.password_dialog {
            let mut submitted = false;
            let mut should_close = false;
            egui::Window::new("Protected Node")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .frame(
                    egui::Frame::window(&ctx.style())
                        .fill(Color32::from_rgb(13, 13, 13))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(31, 31, 31))),
                )
                .show(ctx, |ui| {
                    ui.label(
                        RichText::new(format!("Authenticate to {}", dialog.server_name))
                            .font(FontId::new(18.0, FontFamily::Proportional))
                            .color(Color32::WHITE),
                    );
                    ui.add_space(8.0);
                    ui.add_sized(
                        [300.0, 36.0],
                        egui::TextEdit::singleline(&mut dialog.value)
                            .password(true)
                            .hint_text("Server password")
                            .text_color(Color32::WHITE),
                    );
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add_sized(
                                [120.0, 34.0],
                                egui::Button::new(RichText::new("Connect").color(Color32::BLACK))
                                    .fill(Color32::from_rgb(0, 255, 127)),
                            )
                            .clicked()
                        {
                            submitted = true;
                        }
                        if ui
                            .add_sized(
                                [120.0, 34.0],
                                egui::Button::new(RichText::new("Cancel").color(Color32::WHITE))
                                    .fill(Color32::from_rgb(42, 45, 53)),
                            )
                            .clicked()
                        {
                            should_close = true;
                        }
                    });
                });

            if submitted {
                let _ = self.command_tx.send(ClientCommand::Connect {
                    server_id: dialog.server_id.clone(),
                    endpoint: dialog.endpoint.clone(),
                    server_name: dialog.server_name.clone(),
                    password: Some(dialog.value.clone()),
                    protocol: vpn_suite_core::model::VpnProtocol::WireGuard,
                });
                should_close = true;
            }

            if should_close {
                self.password_dialog = None;
            }
        }

        if let Some(dialog) = &mut self.ovpn_auth_dialog {
            let mut submitted = false;
            let mut should_close = false;
            egui::Window::new("OpenVPN Credentials")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .frame(
                    egui::Frame::window(&ctx.style())
                        .fill(Color32::from_rgb(13, 13, 13))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(0, 180, 120))),
                )
                .show(ctx, |ui| {
                    ui.label(
                        RichText::new(format!(
                            "Login required for '{}'",
                            dialog.profile_name
                        ))
                        .font(FontId::new(16.0, FontFamily::Proportional))
                        .color(Color32::WHITE),
                    );
                    ui.label(
                        RichText::new(
                            "This .ovpn uses auth-user-pass. Enter VPN username and password.",
                        )
                        .font(FontId::new(11.0, FontFamily::Proportional))
                        .color(Color32::from_rgb(170, 170, 170)),
                    );
                    ui.add_space(10.0);
                    ui.add_sized(
                        [320.0, 32.0],
                        egui::TextEdit::singleline(&mut dialog.username)
                            .hint_text("Username")
                            .text_color(Color32::WHITE),
                    );
                    ui.add_space(6.0);
                    ui.add_sized(
                        [320.0, 32.0],
                        egui::TextEdit::singleline(&mut dialog.password)
                            .password(true)
                            .hint_text("Password")
                            .text_color(Color32::WHITE),
                    );
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add_sized(
                                [140.0, 34.0],
                                egui::Button::new(RichText::new("Connect").color(Color32::BLACK))
                                    .fill(Color32::from_rgb(0, 255, 127)),
                            )
                            .clicked()
                        {
                            submitted = true;
                        }
                        if ui
                            .add_sized(
                                [140.0, 34.0],
                                egui::Button::new(RichText::new("Cancel").color(Color32::WHITE))
                                    .fill(Color32::from_rgb(42, 45, 53)),
                            )
                            .clicked()
                        {
                            should_close = true;
                        }
                    });
                });

            if submitted {
                let id = dialog.profile_id;
                let user = dialog.username.clone();
                let pass = dialog.password.clone();
                let _ = self.command_tx.send(ClientCommand::ConnectOvpnFile {
                    id,
                    username: Some(user),
                    password: Some(pass),
                });
                should_close = true;
            }
            if should_close {
                self.ovpn_auth_dialog = None;
            }
        }

        if let Some(dialog) = &mut self.server_settings_dialog {
            let net_info = HostNetInfo::query();
            let mut should_save = false;
            let mut should_close = false;
            egui::Window::new("Server Address Selection")
                .collapsible(false)
                .resizable(true)
                .default_size([520.0, 420.0])
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .frame(
                    egui::Frame::window(&ctx.style())
                        .fill(Color32::from_rgb(13, 13, 13))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(31, 31, 31))),
                )
                .show(ctx, |ui| {
                    ui.label(
                        RichText::new("Public address selection")
                            .font(FontId::new(20.0, FontFamily::Proportional))
                            .color(Color32::WHITE),
                    );
                    ui.label(
                        RichText::new(
                            "Only globally routable addresses are allowed. Local/LAN IPv4 and private/link-local IPv6 addresses are excluded.",
                        )
                        .color(Color32::from_rgb(170, 170, 170)),
                    );
                    ui.add_space(12.0);

                    ui.columns(2, |columns| {
                        render_ip_selection_group(
                            &mut columns[0],
                            "IPv4",
                            &net_info.global_ipv4,
                            &mut dialog.selected_ipv4,
                            "This machine has no global/public IPv4.",
                        );
                        render_ip_selection_group(
                            &mut columns[1],
                            "IPv6",
                            &net_info.global_ipv6,
                            &mut dialog.selected_ipv6,
                            "This machine has no global/public IPv6.",
                        );
                    });

                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add_sized(
                                [120.0, 34.0],
                                egui::Button::new(RichText::new("Save").color(Color32::BLACK))
                                    .fill(Color32::from_rgb(0, 255, 127)),
                            )
                            .clicked()
                        {
                            should_save = true;
                        }
                        if ui
                            .add_sized(
                                [120.0, 34.0],
                                egui::Button::new(RichText::new("Cancel").color(Color32::WHITE))
                                    .fill(Color32::from_rgb(42, 45, 53)),
                            )
                            .clicked()
                        {
                            should_close = true;
                        }
                    });
                });

            if should_save {
                let _ = self
                    .command_tx
                    .send(ClientCommand::SaveLocalServerSelection {
                        ipv4: dialog.selected_ipv4.iter().cloned().collect(),
                        ipv6: dialog.selected_ipv6.iter().cloned().collect(),
                    });
                should_close = true;
            }

            if should_close {
                self.server_settings_dialog = None;
            }
        }
    }
}

fn install_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.override_text_color = Some(Color32::WHITE);
    visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(0, 0, 0);
    visuals.widgets.noninteractive.fg_stroke.color = Color32::from_rgb(210, 210, 210);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(16, 16, 16);
    visuals.widgets.inactive.fg_stroke.color = Color32::WHITE;
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(28, 28, 28);
    visuals.widgets.hovered.fg_stroke.color = Color32::WHITE;
    visuals.widgets.active.bg_fill = Color32::from_rgb(0, 204, 102);
    visuals.widgets.active.fg_stroke.color = Color32::BLACK;
    visuals.widgets.open.bg_fill = Color32::from_rgb(20, 20, 20);
    visuals.panel_fill = Color32::from_rgb(0, 0, 0);
    visuals.window_fill = Color32::from_rgb(12, 12, 12);
    visuals.extreme_bg_color = Color32::from_rgb(0, 0, 0);
    visuals.faint_bg_color = Color32::from_rgb(18, 18, 18);
    visuals.code_bg_color = Color32::from_rgb(16, 16, 16);
    visuals.selection.bg_fill = Color32::from_rgb(0, 255, 127).linear_multiply(0.35);
    visuals.selection.stroke = Stroke::new(1.0, Color32::from_rgb(0, 255, 127));
    visuals.hyperlink_color = Color32::from_rgb(0, 255, 127);
    visuals.warn_fg_color = Color32::from_rgb(200, 200, 200);
    visuals.error_fg_color = Color32::from_rgb(220, 220, 220);
    visuals.window_stroke = Stroke::new(1.0, Color32::from_rgb(40, 40, 40));
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(36, 36, 36));
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(48, 48, 48));
    ctx.set_visuals(visuals);

    // Slightly larger base text + solid contrast for website-like sharpness.
    ctx.style_mut(|style| {
        style.spacing.item_spacing = Vec2::new(8.0, 6.0);
        style.spacing.button_padding = Vec2::new(12.0, 7.0);
        style.visuals.override_text_color = Some(Color32::from_rgb(245, 245, 245));
        style.text_styles.insert(
            egui::TextStyle::Heading,
            FontId::new(22.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Body,
            FontId::new(15.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Button,
            FontId::new(14.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Small,
            FontId::new(12.5, FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Monospace,
            FontId::new(13.5, FontFamily::Monospace),
        );
    });
}

/// Load system fonts for sharp high-DPI text. Falls back silently to egui defaults.
fn install_crisp_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    // Linux: probe DejaVu/Liberation/Noto first (cheap hit), then Windows fallback.
    // Windows: prefer modern UI fonts before cross-platform fallbacks.
    #[cfg(target_os = "linux")]
    let prop_candidates = [
        // Debian/Ubuntu/Mint
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/opentype/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/truetype/ubuntu/Ubuntu-R.ttf",
        // Arch (TTF)
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/LiberationSans-Regular.ttf",
        "/usr/share/fonts/noto/NotoSans-Regular.ttf",
        // Fedora (dejavu-sans-fonts, google-noto, cantarell)
        "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
        "/usr/share/fonts/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/google-noto-sans-fonts/NotoSans-Regular.ttf",
        "/usr/share/fonts/cantarell/Cantarell-Regular.otf",
        "/usr/share/fonts/cantarell/Cantarell-Regular.ttf",
        "/usr/share/fonts/truetype/freefont/FreeSans.ttf",
        r"C:\Windows\Fonts\segoeui.ttf",
        r"C:\Windows\Fonts\SegoeUI.ttf",
        r"C:\Windows\Fonts\calibri.ttf",
    ];
    #[cfg(not(target_os = "linux"))]
    let prop_candidates = [
        r"C:\Windows\Fonts\segoeui.ttf",
        r"C:\Windows\Fonts\SegoeUI.ttf",
        r"C:\Windows\Fonts\calibri.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/truetype/freefont/FreeSans.ttf",
        "/usr/share/fonts/opentype/noto/NotoSans-Regular.ttf",
    ];
    #[cfg(target_os = "linux")]
    let mono_candidates = [
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoSansMono-Regular.ttf",
        "/usr/share/fonts/truetype/ubuntu/UbuntuMono-R.ttf",
        // Arch
        "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
        "/usr/share/fonts/TTF/LiberationMono-Regular.ttf",
        // Fedora
        "/usr/share/fonts/dejavu-sans-fonts/DejaVuSansMono.ttf",
        "/usr/share/fonts/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/adobe-source-code-pro/SourceCodePro-Regular.otf",
        "/usr/share/fonts/truetype/freefont/FreeMono.ttf",
        r"C:\Windows\Fonts\cascadiamono.ttf",
        r"C:\Windows\Fonts\CascadiaMono.ttf",
        r"C:\Windows\Fonts\consola.ttf",
        r"C:\Windows\Fonts\lucon.ttf",
    ];
    #[cfg(not(target_os = "linux"))]
    let mono_candidates = [
        r"C:\Windows\Fonts\cascadiamono.ttf",
        r"C:\Windows\Fonts\CascadiaMono.ttf",
        r"C:\Windows\Fonts\consola.ttf",
        r"C:\Windows\Fonts\lucon.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoSansMono-Regular.ttf",
        "/usr/share/fonts/truetype/freefont/FreeMono.ttf",
    ];

    for path in prop_candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert("ui_prop".to_owned(), egui::FontData::from_owned(bytes));
            if let Some(prop) = fonts.families.get_mut(&FontFamily::Proportional) {
                prop.insert(0, "ui_prop".to_owned());
            }
            break;
        }
    }
    for path in mono_candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert("ui_mono".to_owned(), egui::FontData::from_owned(bytes));
            if let Some(mono) = fonts.families.get_mut(&FontFamily::Monospace) {
                mono.insert(0, "ui_mono".to_owned());
            }
            break;
        }
    }

    ctx.set_fonts(fonts);

    // Tighter feathering = crisper edges on high-DPI (less soft glow on glyphs).
    ctx.tessellation_options_mut(|opts| {
        opts.feathering = true;
        opts.feathering_size_in_pixels = 0.6;
    });
}

/// Finite-width connection progress bar (Tor purple or OpenVPN neon green).
fn paint_connect_progress_bar(ui: &mut egui::Ui, progress: f32, width: f32, fill: Color32) {
    let height = 10.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter();
    // Track: dark neutral; stroke tinted toward fill.
    let track = Color32::from_rgb(18, 22, 20);
    let stroke_col = Color32::from_rgba_unmultiplied(fill.r(), fill.g(), fill.b(), 90);
    painter.rect_filled(rect, 5.0, track);
    painter.rect_stroke(rect, 5.0, Stroke::new(1.0, stroke_col));
    let p = progress.clamp(0.0, 1.0);
    let fill_w = (rect.width() * p).max(if p > 0.01 { 6.0 } else { 0.0 });
    if fill_w > 0.0 {
        let fill_rect = egui::Rect::from_min_size(rect.min, Vec2::new(fill_w, rect.height()));
        painter.rect_filled(fill_rect, 5.0, fill);
        let edge_x = fill_rect.right().min(rect.right());
        painter.circle_filled(
            Pos2::new(edge_x - 2.0, fill_rect.center().y),
            3.0,
            Color32::from_rgba_unmultiplied(255, 255, 255, 70),
        );
    }
}

fn render_notice(ui: &mut egui::Ui, notice: Option<&str>) {
    let Some(notice) = notice else {
        return;
    };
    // Hide discovery chatter and empty-status noise from the main view.
    let lower = notice.to_ascii_lowercase();
    if lower.starts_with("discovered ")
        || lower.contains("online node")
        || lower.contains("saved node")
        || lower.contains("currently offline")
    {
        return;
    }
    egui::Frame::none()
        .fill(Color32::from_rgb(18, 18, 18))
        .stroke(Stroke::new(1.0, Color32::from_rgb(40, 40, 40)))
        .rounding(8.0)
        .inner_margin(Margin::same(10.0))
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width());
            ui.add(
                egui::Label::new(RichText::new(notice).color(Color32::from_rgb(210, 210, 210)))
                    .wrap(),
            );
        });
    ui.add_space(10.0);
}

#[allow(dead_code)] // kept for optional header status chrome
fn render_connection_pill(ui: &mut egui::Ui, active: &Option<ActiveConnection>) {
    let (label, color, subtitle) = if let Some(active) = active {
        match active.phase {
            ConnectionPhase::Connecting => (
                "CONNECTING",
                Color32::from_rgb(168, 85, 247),
                String::from("Negotiating"),
            ),
            ConnectionPhase::Connected => (
                "CONNECTED",
                Color32::from_rgb(0, 255, 127),
                active
                    .connected_at_unix
                    .map(|started| format_elapsed(unix_now().saturating_sub(started)))
                    .unwrap_or_else(|| String::from("00:00:00")),
            ),
            ConnectionPhase::Reconnecting => (
                "RECONNECTING",
                Color32::from_rgb(200, 200, 200),
                format!("attempt {}", active.attempt_count.max(1)),
            ),
            ConnectionPhase::Cooldown => (
                "COOLDOWN",
                Color32::from_rgb(160, 160, 160),
                active
                    .cooldown_until_unix
                    .map(|until| format_remaining(until.saturating_sub(unix_now())))
                    .unwrap_or_else(|| String::from("Blocked")),
            ),
            ConnectionPhase::Error => (
                "ERROR",
                Color32::from_rgb(200, 200, 200),
                String::from("Connection issue"),
            ),
            ConnectionPhase::Disconnected => (
                "IDLE",
                Color32::from_rgb(68, 68, 68),
                String::from("Not connected"),
            ),
        }
    } else {
        (
            "IDLE",
            Color32::from_rgb(68, 68, 68),
            String::from("Not connected"),
        )
    };

    let (rect, _response) = ui.allocate_exact_size(Vec2::new(150.0, 36.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 12.0, Color32::from_rgb(13, 13, 13));
    painter.circle_filled(rect.left_center() + Vec2::new(14.0, 0.0), 4.0, color);
    painter.text(
        rect.left_center() + Vec2::new(26.0, -7.0),
        egui::Align2::LEFT_TOP,
        label,
        FontId::new(12.0, FontFamily::Proportional),
        Color32::WHITE,
    );
    painter.text(
        rect.left_center() + Vec2::new(26.0, 5.0),
        egui::Align2::LEFT_TOP,
        subtitle,
        FontId::new(10.0, FontFamily::Monospace),
        Color32::from_rgb(170, 170, 170),
    );
}

#[allow(dead_code)]
fn country_code_to_emoji(code: &str) -> String {
    code.to_uppercase()
        .chars()
        .map(|c| std::char::from_u32(c as u32 + 127397).unwrap_or(c))
        .collect()
}

/// Resolve a country flag PNG/SVG path. Prefer PNGs from the w2560 set.
fn resolve_flag_path(country_code: &str) -> Option<PathBuf> {
    if country_code.is_empty() {
        return None;
    }
    let cc = country_code.to_lowercase();
    // Also accept common aliases (e.g. UK → GB).
    let aliases: &[&str] = match cc.as_str() {
        "uk" => &["uk", "gb"],
        "gb" => &["gb", "uk"],
        _ => &[],
    };
    let mut codes: Vec<&str> = vec![cc.as_str()];
    for a in aliases {
        if !codes.contains(a) {
            codes.push(a);
        }
    }

    let mut dirs: Vec<PathBuf> = Vec::new();
    // User-supplied high-res set (primary source you asked for).
    dirs.push(PathBuf::from(r"C:\Users\hemsh_sfya5gq\Downloads\w2560"));
    // Bundled next to the binary (build.rs stages these).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            dirs.push(exe_dir.join("assets").join("flags"));
            dirs.push(exe_dir.join("flags"));
            dirs.push(exe_dir.join("../../apps/client/assets/flags"));
            dirs.push(exe_dir.join("../../../apps/client/assets/flags"));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join("apps/client/assets/flags"));
        dirs.push(cwd.join("assets/flags"));
        dirs.push(cwd.join("flags"));
    }
    dirs.push(PathBuf::from(
        r"C:\Users\hemsh_sfya5gq\Documents\New project\vpn-suite\apps\client\assets\flags",
    ));
    // Linux deb install locations
    dirs.push(PathBuf::from("/usr/share/vpn-client/flags"));
    dirs.push(PathBuf::from("/usr/share/zeronode-vpn-client/flags"));
    dirs.push(PathBuf::from("/usr/share/vpn-suite/flags"));

    for dir in &dirs {
        for code in &codes {
            for ext in ["png", "svg"] {
                let path = dir.join(format!("{code}.{ext}"));
                if path.is_file() {
                    return Some(path);
                }
                if let Ok(canonical) = path.canonicalize() {
                    if canonical.is_file() {
                        return Some(canonical);
                    }
                }
            }
        }
    }
    None
}

/// In-memory cache of Lanczos-resized flag PNGs keyed by country + pixel size.
fn flag_png_cache() -> &'static Mutex<HashMap<String, Arc<[u8]>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<[u8]>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// High-quality downscale of a source flag (often 2560px wide) to the exact
/// physical pixel box the UI will paint. GPU bilinear from 2560→~64px looks
/// soft; Lanczos3 at 1:1 physical pixels stays razor-sharp.
fn prepare_crisp_flag_png(path: &Path, target_w: u32, target_h: u32) -> Option<Arc<[u8]>> {
    use image::imageops::FilterType;
    use image::{DynamicImage, ImageFormat, RgbaImage};

    let img = image::open(path).ok()?;
    // Fit inside the box, preserve aspect (flags are not always 3:2).
    let fitted = img.resize(target_w.max(1), target_h.max(1), FilterType::Lanczos3);
    let rgba = fitted.to_rgba8();
    let (rw, rh) = (rgba.width(), rgba.height());

    // Center on a transparent canvas so the widget size stays exact.
    let mut canvas = RgbaImage::new(target_w.max(1), target_h.max(1));
    let ox = target_w.saturating_sub(rw) / 2;
    let oy = target_h.saturating_sub(rh) / 2;
    image::imageops::overlay(&mut canvas, &rgba, ox as i64, oy as i64);

    let mut buf = Vec::new();
    DynamicImage::ImageRgba8(canvas)
        .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
        .ok()?;
    if buf.is_empty() {
        return None;
    }
    Some(Arc::from(buf.into_boxed_slice()))
}

/// Load a flag as an egui Image from **pre-resized PNG bytes** (not file:// URI).
///
/// Source assets are huge (w2560). We Lanczos3-resize to the physical pixel
/// size of the widget (`logical * pixels_per_point`) so the GPU does almost
/// no minification and the flag stays super crisp. `bytes://` URIs are
/// cacheable; `file://` breaks on Windows paths with spaces.
pub fn flag_image(
    country_code: &str,
    size: Vec2,
    pixels_per_point: f32,
) -> Option<egui::Image<'static>> {
    let path = resolve_flag_path(country_code)?;
    let ppp = pixels_per_point.max(1.0);
    // 3×–4× supersample of logical size so GPU sampling stays sharp on every
    // DPI (w2560 sources are large enough to feed this without upscaling).
    let scale = (ppp * 3.5).clamp(2.0, 6.0);
    let tw = (size.x * scale).ceil().clamp(48.0, 1280.0) as u32;
    let th = (size.y * scale).ceil().clamp(32.0, 1280.0) as u32;

    let cc = country_code.to_lowercase();
    let cache_key = format!("{cc}:{tw}x{th}");
    let uri = format!("bytes://zeronode/flag/crisp/{cc}/{tw}x{th}.png");

    let bytes: Arc<[u8]> = {
        if let Ok(cache) = flag_png_cache().lock() {
            if let Some(hit) = cache.get(&cache_key) {
                hit.clone()
            } else {
                drop(cache);
                let prepared = prepare_crisp_flag_png(&path, tw, th)?;
                if let Ok(mut cache) = flag_png_cache().lock() {
                    cache.insert(cache_key, prepared.clone());
                }
                prepared
            }
        } else {
            prepare_crisp_flag_png(&path, tw, th)?
        }
    };

    // Supersampled texture + linear filter ≈ sharp website-quality flags.
    // Mipmaps keep fine stripes clean when the widget is slightly smaller
    // than the texture.
    let tex_opts = egui::TextureOptions {
        magnification: egui::TextureFilter::Linear,
        minification: egui::TextureFilter::Linear,
        wrap_mode: egui::TextureWrapMode::ClampToEdge,
        mipmap_mode: Some(egui::TextureFilter::Linear),
    };

    Some(
        egui::Image::from_bytes(uri, bytes)
            .texture_options(tex_opts)
            .fit_to_exact_size(size)
            .maintain_aspect_ratio(true)
            .sense(Sense::hover()),
    )
}

/// Back-compat helper used by call sites that still want a URI string.
/// Prefer `flag_image` for rendering.
pub fn get_flag_uri(country_code: &str) -> Option<String> {
    let path = resolve_flag_path(country_code)?;
    // Percent-encode spaces so file:// still works as a fallback path.
    let mut s = path
        .canonicalize()
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    if let Some(rest) = s.strip_prefix("//?/") {
        s = rest.to_string();
    }
    if let Some(rest) = s.strip_prefix("/?/") {
        s = rest.to_string();
    }
    s = s.replace(' ', "%20");
    if s.len() >= 2 && s.as_bytes().get(1) == Some(&b':') {
        Some(format!("file:///{s}"))
    } else if s.starts_with('/') {
        Some(format!("file://{s}"))
    } else {
        Some(format!("file:///{s}"))
    }
}

/// Render a flag image into the UI, or a small ISO code badge as fallback.
pub fn show_flag(ui: &mut egui::Ui, country_code: &str, size: Vec2) {
    if country_code.is_empty() {
        return;
    }
    let ppp = ui.ctx().pixels_per_point();
    // Prefer a slightly larger on-screen flag so the supersampled texture
    // has more physical pixels to work with (still constrained by `size`).
    if let Some(img) = flag_image(country_code, size, ppp) {
        egui::Frame::none()
            .stroke(Stroke::new(
                1.0,
                Color32::from_rgb(0, 255, 127).linear_multiply(0.45),
            ))
            .rounding(1.0)
            .inner_margin(0.5)
            .fill(Color32::from_rgb(6, 6, 6))
            .show(ui, |ui| {
                ui.set_min_size(size);
                ui.add(img.rounding(0.0));
            });
    } else {
        // Visible fallback so missing assets never look like "nothing".
        ui.label(
            RichText::new(country_code.to_uppercase())
                .font(FontId::new(11.0, FontFamily::Monospace))
                .color(Color32::from_rgb(0, 255, 127))
                .strong(),
        );
    }
}

/// Kill a process image without spawning `taskkill.exe` (no console flash).
#[cfg(target_os = "windows")]
fn silent_windows_kill_image(image: &str) {
    let n = platform::kill_process_image(image);
    tracing::info!("terminated {n} process(es) matching {image}");
}

/// Best-effort cleanup of leftover WinINet SOCKS hints (Win32 registry only).
#[cfg(target_os = "windows")]
fn clear_stale_wininet_socks_hint() {
    platform::clear_stale_wininet_socks_hint();
}

#[allow(dead_code)] // kept for optional header status chrome
fn render_hosting_pill(ui: &mut egui::Ui, active: bool, port: u16) {
    let label = if active { "HOST ON" } else { "HOST OFF" };
    let color = if active {
        Color32::from_rgb(0, 255, 127)
    } else {
        Color32::from_rgb(68, 68, 68)
    };
    let subtitle = if active {
        format!("UDP {}", if port == 0 { 51820 } else { port })
    } else {
        String::from("Not running")
    };

    let (rect, _response) = ui.allocate_exact_size(Vec2::new(130.0, 36.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 12.0, Color32::from_rgb(13, 13, 13));
    painter.circle_filled(rect.left_center() + Vec2::new(14.0, 0.0), 4.0, color);
    painter.text(
        rect.left_center() + Vec2::new(26.0, -7.0),
        egui::Align2::LEFT_TOP,
        label,
        FontId::new(12.0, FontFamily::Proportional),
        Color32::WHITE,
    );
    painter.text(
        rect.left_center() + Vec2::new(26.0, 5.0),
        egui::Align2::LEFT_TOP,
        subtitle,
        FontId::new(10.0, FontFamily::Monospace),
        Color32::from_rgb(170, 170, 170),
    );
}

#[allow(dead_code)] // Available Nodes list intentionally removed from main view
fn render_server_card(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    server: &ServerSummary,
    active: Option<&ActiveConnection>,
    password_dialog: &mut Option<PasswordDialog>,
    command_tx: &Sender<ClientCommand>,
) {
    let now = unix_now();
    let is_active = active
        .map(|connection| connection.server_id == server.server_id)
        .unwrap_or(false);
    let cooldown_active = server.cooldown_until_unix.filter(|until| *until > now);
    
    // Theme: neon green / black / greys only (no blue/orange accents).
    let fill = if is_active {
        Color32::from_rgba_unmultiplied(12, 36, 24, 230)
    } else if server.online {
        Color32::from_rgba_unmultiplied(16, 16, 16, 230)
    } else {
        Color32::from_rgba_unmultiplied(10, 10, 10, 200)
    };

    let stroke = if is_active {
        Stroke::new(1.5, Color32::from_rgb(0, 255, 127).linear_multiply(0.7))
    } else if cooldown_active.is_some() {
        Stroke::new(1.0, Color32::from_rgb(140, 140, 140))
    } else if server.online {
        Stroke::new(1.0, Color32::from_rgb(48, 48, 48))
    } else {
        Stroke::new(1.0, Color32::from_rgb(28, 28, 28))
    };

    let time = ctx.input(|input| input.time) as f32;
    let pulse = (time * 2.5).sin().abs();
    let status_color = if server.online {
        Color32::from_rgb(0, (200.0 + pulse * 55.0) as u8, 127)
    } else {
        Color32::from_rgb(68, 68, 68)
    };

    // The old "Connected / cooldown / Connect" label was replaced by the
    // per-protocol `wg_label` / `ovpn_label` buttons below. Keep this
    // expression under `_` so the logic stays readable but doesn't warn on
    // unused bindings — it's still useful as documentation of intent.
    let _button_label = if is_active {
        String::from("Connected")
    } else if let Some(until) = cooldown_active {
        format_remaining(until.saturating_sub(now))
    } else {
        String::from("Connect")
    };

    let button_fill = if cooldown_active.is_some() {
        Color32::from_rgb(90, 90, 90)
    } else if server.online {
        Color32::from_rgb(0, 200, 100)
    } else {
        Color32::from_rgb(48, 48, 48)
    };
    
    let endpoint = if server.has_password {
        server.masked_endpoint.clone()
    } else {
        server.endpoint.clone()
    };
    
    let message = server.last_message.clone();

    let flag = country_code_to_emoji(&server.country_code);

    egui::Frame::none()
        .fill(fill)
        .stroke(stroke)
        .rounding(8.0)
        .inner_margin(Margin::symmetric(14.0, 12.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if get_flag_uri(&server.country_code).is_some()
                    || resolve_flag_path(&server.country_code).is_some()
                {
                    show_flag(ui, &server.country_code, Vec2::new(40.0, 28.0));
                } else {
                    ui.label(RichText::new(flag).font(FontId::new(22.0, FontFamily::Proportional)));
                }
                ui.vertical(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(&server.name)
                                .font(FontId::new(17.0, FontFamily::Proportional))
                                .strong()
                                .color(Color32::WHITE),
                        );
                        let (dot_rect, _) = ui.allocate_exact_size(Vec2::new(10.0, 10.0), Sense::hover());
                        ui.painter().circle_filled(dot_rect.center(), 4.0, status_color);
                        if server.has_password {
                            ui.label(RichText::new("LOCK").color(Color32::from_rgb(180, 180, 180)).font(FontId::new(11.0, FontFamily::Proportional)));
                        }
                    });
                    ui.label(
                        RichText::new(&server.country_name)
                            .font(FontId::new(13.0, FontFamily::Proportional))
                            .color(Color32::from_rgb(190, 190, 190)),
                    );
                });

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    // OpenVPN Button
                    if server.openvpn_endpoint.is_some() {
                        let ovpn_label = "OVPN";
                        let ovpn_active = is_active && active.map(|a| a.protocol == vpn_suite_core::model::VpnProtocol::OpenVPN).unwrap_or(false);
                        let ovpn_fill = if ovpn_active { Color32::from_rgb(0, 200, 100) } else { button_fill };
                        let ovpn_text = if ovpn_active { Color32::BLACK } else { Color32::WHITE };
                        let ovpn_button = egui::Button::new(RichText::new(ovpn_label).color(ovpn_text).strong())
                            .fill(ovpn_fill)
                            .rounding(6.0)
                            .min_size(Vec2::new(60.0, 32.0));
                        let response = ui.add(ovpn_button);
                        let is_enabled = server.online && !is_active && cooldown_active.is_none();
                        if response.clicked() && is_enabled {
                            if server.has_password {
                                *password_dialog = Some(PasswordDialog {
                                    server_id: server.server_id.clone(),
                                    server_name: server.name.clone(),
                                    endpoint: server.endpoint.clone(),
                                    value: String::new(),
                                });
                            } else {
                                let _ = command_tx.send(ClientCommand::Connect {
                                    server_id: server.server_id.clone(),
                                    endpoint: server.endpoint.clone(),
                                    server_name: server.name.clone(),
                                    password: None,
                                    protocol: vpn_suite_core::model::VpnProtocol::OpenVPN,
                                });
                            }
                        }
                    }

                    // WireGuard Button (Default)
                    let wg_label = "WG";

                    let wg_active = is_active && active.map(|a| a.protocol == vpn_suite_core::model::VpnProtocol::WireGuard).unwrap_or(false);
                    let wg_fill = if wg_active {
                        Color32::from_rgb(0, 200, 100)
                    } else if cooldown_active.is_some() {
                        Color32::from_rgb(90, 90, 90)
                    } else {
                        button_fill
                    };
                    let wg_text = if wg_active || (server.online && cooldown_active.is_none()) {
                        if wg_active || button_fill == Color32::from_rgb(0, 200, 100) {
                            Color32::BLACK
                        } else {
                            Color32::WHITE
                        }
                    } else {
                        Color32::from_rgb(180, 180, 180)
                    };
                    let button = egui::Button::new(RichText::new(wg_label).color(wg_text).strong())
                        .fill(wg_fill)
                        .rounding(6.0)
                        .min_size(Vec2::new(60.0, 32.0));
                    let response = ui.add(button);
                    let is_enabled = server.online && !is_active && cooldown_active.is_none();
                    if response.clicked() && is_enabled {
                        if server.has_password {
                            *password_dialog = Some(PasswordDialog {
                                server_id: server.server_id.clone(),
                                server_name: server.name.clone(),
                                endpoint: server.endpoint.clone(),
                                value: String::new(),
                            });
                        } else {
                            let _ = command_tx.send(ClientCommand::Connect {
                                server_id: server.server_id.clone(),
                                endpoint: server.endpoint.clone(),
                                server_name: server.name.clone(),
                                password: None,
                                protocol: vpn_suite_core::model::VpnProtocol::WireGuard,
                            });
                        }
                    }
                });
            });

            ui.add_space(8.0);
            
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{}", endpoint))
                        .font(FontId::new(11.0, FontFamily::Monospace))
                        .color(if server.online {
                            Color32::from_rgb(200, 220, 255)
                        } else {
                            Color32::from_rgb(120, 120, 120)
                        }),
                );
            });
            ui.label(
                RichText::new(format!(
                    "Port: UDP {}  |  {}",
                    server.listen_port,
                    if server.has_password {
                        "Protected"
                    } else {
                        "Open"
                    }
                ))
                .font(FontId::new(11.0, FontFamily::Monospace))
                .color(Color32::from_rgb(170, 170, 170)),
            );

            if let Some(message) = message {
                ui.add_space(4.0);
                ui.label(
                    RichText::new(message)
                        .font(FontId::new(11.0, FontFamily::Monospace))
                        .color(if cooldown_active.is_some() {
                            Color32::from_rgb(180, 180, 180)
                        } else if server.online {
                            Color32::from_rgb(170, 170, 170)
                        } else {
                            Color32::from_rgb(100, 100, 100)
                        }),
                );
            }
        });
}


fn section_title(ui: &mut egui::Ui, title: &str, accent: Option<Color32>) {
    ui.horizontal(|ui| {
        if let Some(c) = accent {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(4.0, 16.0), Sense::hover());
            ui.painter().rect_filled(rect, 2.0, c);
            ui.add_space(6.0);
        }
        ui.label(
            RichText::new(title)
                .font(FontId::new(17.0, FontFamily::Proportional))
                .strong()
                .color(Color32::WHITE),
        );
    });
}

fn pane_card(ui: &mut egui::Ui, stroke: Color32, add_contents: impl FnOnce(&mut egui::Ui)) {
    // Important: do NOT set inner content width to the *outer* available width.
    // Frame margins would then push the card past the panel/window edge.
    egui::Frame::none()
        .fill(Color32::from_rgb(12, 12, 12))
        .stroke(Stroke::new(1.0, stroke))
        .rounding(8.0)
        .inner_margin(Margin::symmetric(10.0, 10.0))
        .show(ui, |ui| {
            let inner = ui.available_width();
            ui.set_max_width(inner);
            add_contents(ui);
        });
}

fn kv_row(ui: &mut egui::Ui, label: &str, value: &str, value_color: Color32) {
    // Vertical stack avoids horizontal overflow for long values on narrow panes.
    ui.vertical(|ui| {
        ui.label(
            RichText::new(label)
                .font(FontId::new(11.0, FontFamily::Proportional))
                .color(Color32::from_rgb(140, 140, 140)),
        );
        ui.add(
            egui::Label::new(
                RichText::new(value)
                    .font(FontId::new(13.0, FontFamily::Proportional))
                    .color(value_color),
            )
            .wrap(),
        );
    });
    ui.add_space(4.0);
}

/// Compact session strip at the top of the side pane — no empty "not connected" cards.
fn render_session_section(ui: &mut egui::Ui, snapshot: &ClientSnapshot, _panel_w: f32) {
    section_title(ui, "Session", Some(Color32::from_rgb(0, 255, 127)));
    ui.add_space(6.0);

    let online = snapshot.servers.iter().filter(|s| s.online).count();
    let refresh = format_elapsed(unix_now().saturating_sub(snapshot.last_refresh_unix));

    pane_card(ui, Color32::from_rgb(40, 40, 40), |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{online} online"))
                    .font(FontId::new(14.0, FontFamily::Proportional))
                    .color(if online > 0 {
                        Color32::from_rgb(0, 255, 127)
                    } else {
                        Color32::from_rgb(160, 160, 160)
                    })
                    .strong(),
            );
            ui.label(
                RichText::new(format!("·  refreshed {refresh}"))
                    .font(FontId::new(12.0, FontFamily::Proportional))
                    .color(Color32::from_rgb(140, 140, 140)),
            );
        });

        if let Some(active) = &snapshot.active_connection {
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);
            ui.label(
                RichText::new(&active.server_name)
                    .font(FontId::new(14.0, FontFamily::Proportional))
                    .strong()
                    .color(Color32::WHITE),
            );
            if let Some(ip) = active.reserved_client_ip.as_deref() {
                kv_row(ui, "Client IP", ip, Color32::from_rgb(0, 255, 127));
            }
            if let Some(ip) = active.server_internal_ip.as_deref() {
                kv_row(ui, "Server IP", ip, Color32::from_rgb(210, 210, 210));
            }
            if let Some(sid) = active.session_id.as_deref().filter(|s| !s.is_empty()) {
                let short = if sid.len() > 28 {
                    format!("{}…", &sid[..28])
                } else {
                    sid.to_string()
                };
                kv_row(ui, "Session", &short, Color32::from_rgb(180, 180, 180));
            }
        } else {
            ui.add_space(4.0);
            ui.label(
                RichText::new("No active tunnel")
                    .font(FontId::new(13.0, FontFamily::Proportional))
                    .color(Color32::from_rgb(130, 130, 130)),
            );
        }
    });
}

/// Direct public-IP card — compact, high contrast, no filler copy.
fn render_ip_details_card(
    ui: &mut egui::Ui,
    snapshot: &ClientSnapshot,
    command_tx: &Sender<ClientCommand>,
    _panel_w: f32,
) {
    section_title(ui, "Your IP", Some(Color32::from_rgb(0, 255, 127)));
    ui.add_space(6.0);

    pane_card(ui, Color32::from_rgb(0, 255, 127).linear_multiply(0.3), |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Public")
                    .font(FontId::new(13.0, FontFamily::Proportional))
                    .color(Color32::from_rgb(160, 160, 160)),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(RichText::new("Refresh").color(Color32::BLACK))
                            .fill(Color32::from_rgb(0, 255, 127))
                            .min_size(Vec2::new(70.0, 24.0)),
                    )
                    .clicked()
                {
                    let _ = command_tx.send(ClientCommand::RefreshLocalIp);
                }
            });
        });

        if let Some(info) = snapshot.local_ip_info.as_ref() {
            let cc = info.country_code.as_str();
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if !cc.is_empty() {
                    show_flag(ui, cc, Vec2::new(52.0, 36.0));
                    ui.add_space(8.0);
                }
                let name = if !cc.is_empty() {
                    crate::tor_geo::country_name(cc).unwrap_or_else(|| cc.to_string())
                } else if !info.country.is_empty() {
                    info.country.clone()
                } else {
                    String::from("Unknown")
                };
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(name)
                            .font(FontId::new(14.0, FontFamily::Proportional))
                            .strong()
                            .color(Color32::WHITE),
                    );
                    let (v4_label, v4_val) = if info.ip.parse::<std::net::Ipv4Addr>().is_ok() {
                        ("IPv4", info.ip.as_str())
                    } else if info.ip.parse::<std::net::Ipv6Addr>().is_ok() {
                        ("IPv6", info.ip.as_str())
                    } else {
                        ("IP", info.ip.as_str())
                    };
                    ui.label(
                        RichText::new(format!("{v4_label}  {v4_val}"))
                            .font(FontId::new(13.0, FontFamily::Monospace))
                            .color(Color32::from_rgb(0, 255, 127)),
                    );
                });
            });
            // Dual-stack: show public IPv6 when we resolved one separately.
            if !info.ipv6.is_empty() && info.ipv6 != info.ip {
                ui.add_space(2.0);
                kv_row(ui, "IPv6", &info.ipv6, Color32::from_rgb(120, 220, 180));
            }

            let mut loc = Vec::new();
            if !info.city.is_empty() {
                loc.push(info.city.clone());
            }
            if !info.region.is_empty() {
                loc.push(info.region.clone());
            }
            if !loc.is_empty() {
                kv_row(ui, "Location", &loc.join(", "), Color32::from_rgb(210, 210, 210));
            }
            if !info.isp.is_empty() {
                kv_row(ui, "ISP", &info.isp, Color32::from_rgb(210, 210, 210));
            }
            if info.lat != 0.0 || info.lon != 0.0 {
                kv_row(
                    ui,
                    "Coords",
                    &format!("{:.2}, {:.2}", info.lat, info.lon),
                    Color32::from_rgb(180, 180, 180),
                );
            }
            let age = unix_now().saturating_sub(snapshot.local_ip_refresh_unix);
            ui.add_space(2.0);
            ui.label(
                RichText::new(format!("Updated {}", format_elapsed(age)))
                    .font(FontId::new(11.0, FontFamily::Proportional))
                    .color(Color32::from_rgb(110, 110, 110)),
            );
        } else {
            ui.add_space(6.0);
            ui.label(
                RichText::new("Looking up public IP…")
                    .font(FontId::new(13.0, FontFamily::Proportional))
                    .color(Color32::from_rgb(150, 150, 150)),
            );
        }
    });
}

fn render_hosting_section(
    ui: &mut egui::Ui,
    snapshot: &ClientSnapshot,
    command_tx: &Sender<ClientCommand>,
    open_server_settings: &mut bool,
    open: &mut bool,
    _panel_w: f32,
) {
    ui.horizontal(|ui| {
        let arrow = if *open { "▾" } else { "▸" };
        if ui
            .add(
                egui::Button::new(
                    RichText::new(format!("{arrow}  Hosting"))
                        .font(FontId::new(15.0, FontFamily::Proportional))
                        .strong()
                        .color(Color32::WHITE),
                )
                .fill(Color32::TRANSPARENT)
                .stroke(Stroke::NONE),
            )
            .clicked()
        {
            *open = !*open;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let (label, color) = if snapshot.local_server_active {
                ("ONLINE", Color32::from_rgb(0, 255, 127))
            } else {
                ("OFFLINE", Color32::from_rgb(120, 120, 120))
            };
            ui.label(
                RichText::new(label)
                    .font(FontId::new(12.0, FontFamily::Proportional))
                    .strong()
                    .color(color),
            );
        });
    });

    if !*open {
        return;
    }
    ui.add_space(6.0);

    pane_card(ui, Color32::from_rgb(40, 40, 40), |ui| {
        ui.horizontal(|ui| {
            if ui
                .add(
                    egui::Button::new(RichText::new("Settings").color(Color32::BLACK))
                        .fill(Color32::from_rgb(0, 255, 127))
                        .min_size(Vec2::new(80.0, 26.0)),
                )
                .clicked()
            {
                *open_server_settings = true;
            }
            ui.add_space(6.0);
            if snapshot.local_server_active {
                if ui
                    .add(
                        egui::Button::new(RichText::new("Stop").color(Color32::WHITE))
                            .fill(Color32::from_rgb(48, 48, 48))
                            .min_size(Vec2::new(64.0, 26.0)),
                    )
                    .clicked()
                {
                    let _ = command_tx.send(ClientCommand::StopLocalServer);
                }
            } else if ui
                .add(
                    egui::Button::new(RichText::new("Start Server").color(Color32::BLACK))
                        .fill(Color32::from_rgb(0, 255, 127))
                        .min_size(Vec2::new(100.0, 26.0)),
                )
                .clicked()
            {
                let _ = command_tx.send(ClientCommand::StartLocalServer);
            }
        });

        if snapshot.local_server_active {
            ui.add_space(8.0);
            kv_row(
                ui,
                "Port",
                &format!("UDP {}", snapshot.local_server_port),
                Color32::WHITE,
            );
            kv_row(
                ui,
                "Peers",
                &format!("{}", snapshot.local_server_peers),
                Color32::WHITE,
            );
            let endpoints = local_server_endpoints(snapshot);
            if let Some(ep) = endpoints.first() {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(ep)
                                .font(FontId::new(12.0, FontFamily::Monospace))
                                .color(Color32::from_rgb(0, 255, 127)),
                        )
                        .wrap(),
                    );
                    if ui.small_button("Copy").clicked() {
                        ui.output_mut(|o| o.copied_text = ep.clone());
                    }
                });
            }
            if !snapshot.local_server_public_key.is_empty() {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let key = &snapshot.local_server_public_key;
                    let short = if key.len() > 22 {
                        format!("{}…{}", &key[..10], &key[key.len().saturating_sub(8)..])
                    } else {
                        key.clone()
                    };
                    ui.label(
                        RichText::new(short)
                            .font(FontId::new(11.0, FontFamily::Monospace))
                            .color(Color32::from_rgb(180, 180, 180)),
                    );
                    if ui.small_button("Copy key").clicked() {
                        ui.output_mut(|o| o.copied_text = key.clone());
                    }
                });
            }
        }
    });
}

fn render_network_section(ui: &mut egui::Ui, open: &mut bool, _panel_w: f32) {
    ui.horizontal(|ui| {
        let arrow = if *open { "▾" } else { "▸" };
        if ui
            .add(
                egui::Button::new(
                    RichText::new(format!("{arrow}  Network"))
                        .font(FontId::new(15.0, FontFamily::Proportional))
                        .strong()
                        .color(Color32::WHITE),
                )
                .fill(Color32::TRANSPARENT)
                .stroke(Stroke::NONE),
            )
            .clicked()
        {
            *open = !*open;
        }
    });
    if !*open {
        return;
    }
    ui.add_space(6.0);

    let net_info = vpn_suite_core::net_info::HostNetInfo::query();
    pane_card(ui, Color32::from_rgb(40, 40, 40), |ui| {
        ui.label(
            RichText::new("Public")
                .font(FontId::new(12.0, FontFamily::Proportional))
                .color(Color32::from_rgb(140, 140, 140)),
        );
        if net_info.global_ipv4.is_empty() && net_info.global_ipv6.is_empty() {
            ui.label(
                RichText::new("none detected")
                    .font(FontId::new(12.0, FontFamily::Proportional))
                    .color(Color32::from_rgb(130, 130, 130)),
            );
        } else {
            for ip in &net_info.global_ipv4 {
                ui.add(
                    egui::Label::new(
                        RichText::new(ip)
                            .font(FontId::new(12.0, FontFamily::Monospace))
                            .color(Color32::WHITE),
                    )
                    .wrap(),
                );
            }
            for ip in &net_info.global_ipv6 {
                ui.add(
                    egui::Label::new(
                        RichText::new(ip)
                            .font(FontId::new(12.0, FontFamily::Monospace))
                            .color(Color32::from_rgb(200, 200, 200)),
                    )
                    .wrap(),
                );
            }
        }
        ui.add_space(6.0);
        ui.label(
            RichText::new("LAN")
                .font(FontId::new(12.0, FontFamily::Proportional))
                .color(Color32::from_rgb(140, 140, 140)),
        );
        if net_info.local_ipv4.is_empty() && net_info.local_ipv6.is_empty() {
            ui.label(
                RichText::new("none")
                    .font(FontId::new(12.0, FontFamily::Proportional))
                    .color(Color32::from_rgb(130, 130, 130)),
            );
        } else {
            for ip in &net_info.local_ipv4 {
                ui.add(
                    egui::Label::new(
                        RichText::new(ip)
                            .font(FontId::new(12.0, FontFamily::Monospace))
                            .color(Color32::WHITE),
                    )
                    .wrap(),
                );
            }
            for ip in &net_info.local_ipv6 {
                ui.add(
                    egui::Label::new(
                        RichText::new(ip)
                            .font(FontId::new(12.0, FontFamily::Monospace))
                            .color(Color32::from_rgb(200, 200, 200)),
                    )
                    .wrap(),
                );
            }
        }
    });
}

fn local_server_endpoints(snapshot: &ClientSnapshot) -> Vec<String> {
    let port = if snapshot.local_server_port == 0 {
        51820
    } else {
        snapshot.local_server_port
    };
    let mut endpoints = snapshot
        .local_server_selected_ipv4
        .iter()
        .map(|ip| format!("{}:{}", ip, port))
        .collect::<Vec<_>>();
    endpoints.extend(
        snapshot
            .local_server_selected_ipv6
            .iter()
            .map(|ip| format!("[{}]:{}", ip, port)),
    );
    endpoints
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

fn render_ip_selection_group(
    ui: &mut egui::Ui,
    title: &str,
    available: &[String],
    selected: &mut BTreeSet<String>,
    empty_message: &str,
) {
    egui::Frame::group(ui.style())
        .fill(Color32::from_rgb(13, 13, 13))
        .stroke(Stroke::new(1.0, Color32::from_rgb(31, 31, 31)))
        .inner_margin(Margin::same(12.0))
        .show(ui, |ui| {
            ui.label(
                RichText::new(title)
                    .font(FontId::new(16.0, FontFamily::Proportional))
                    .color(Color32::WHITE),
            );
            ui.add_space(8.0);

            if available.is_empty() {
                ui.label(
                    RichText::new(empty_message)
                        .font(FontId::new(12.0, FontFamily::Proportional))
                        .color(Color32::from_rgb(170, 170, 170)),
                );
                return;
            }

            for ip in available {
                let mut checked = selected.contains(ip);
                if ui.checkbox(&mut checked, ip).changed() {
                    if checked {
                        selected.insert(ip.clone());
                    } else {
                        selected.remove(ip);
                    }
                }
            }
        });
}

fn format_elapsed(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;
    format!("{hours:02}:{minutes:02}:{secs:02}")
}

fn format_remaining(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3600;
    let minutes = (seconds % 3600) / 60;

    if days > 0 {
        format!("{days}d {hours:02}h")
    } else if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else {
        format!("{minutes}m")
    }
}

fn start_backend(
    paths: AppPaths,
    config: ClientConfig,
    command_rx: Receiver<ClientCommand>,
    event_tx: Sender<ClientEvent>,
) {
    thread::spawn(move || {
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime should build");

        runtime.block_on(async move {
            let state = load_or_create_client_state(&paths).unwrap_or_default();
            let (async_tx, mut async_rx) = tokio_mpsc::unbounded_channel();
            let async_tx_clone = async_tx.clone();
            thread::spawn(move || {
                while let Ok(command) = command_rx.recv() {
                    if async_tx_clone.send(command).is_err() {
                        break;
                    }
                }
            });

            let mut backend = BackendState::new(paths.clone(), config, state, event_tx, async_tx.clone());
            // Stale-cleanup for previous SIGKILL / task-manager kill: the GUI
            // was hard-killed before it could run teardown_and_quit, so its
            // `tor` child and helper-owned TUN may still be up and will block
            // the next Tor bootstrap at 18%. Cleaning here is idempotent and
            // runs unprivileged (helper does the root part).
            #[cfg(target_os = "linux")]
            {
                let _ = crate::helper::send("tor_stop", serde_json::json!({}));
                // If helper not running, try direct cleanup best-effort in a
                // blocking pool so we don't stall the runtime startup.
                let _ = tokio::task::spawn_blocking(|| {
                    let _ = vpn_platform_linux::stop_tor_system_tunnel();
                    // Orphaned tor from SIGKILL has no pdeathsig on old builds.
                    let _ = vpn_platform_linux::kill_process_by_name("tor");
                })
                .await;
            }
            let geo_dir = backend.paths.base_dir.join("geoip");
            backend.geoip = crate::tor_geo::try_open_geoip_stack(&geo_dir);
            let geo_refresh_tx = async_tx;
            tokio::spawn(async move {
                if let Some(stack) = crate::tor_geo::open_geoip_stack(&geo_dir).await {
                    let _ = geo_refresh_tx.send(ClientCommand::GeoIpReady(stack));
                }
            });
            let _ = backend.publish_snapshot();

            // Kick the local IP + GeoIP lookups immediately so the right-pane
            // "Your IP Details" card has data to display the moment the GUI
            // makes its first paint. The VpnClientApp::new() also fires this,
            // but we don't depend on that — doing it from the backend too
            // means the very first refresh-publish after startup already
            // carries local_ip_info.
            let _ = backend.refresh_local_ip().await;

            let mut discovery_interval = time::interval(Duration::from_secs(30));
            let mut status_interval = time::interval(Duration::from_secs(5));
            let mut local_ip_interval = time::interval(Duration::from_secs(5 * 60));

            loop {
                tokio::select! {
                    _ = discovery_interval.tick() => {
                        if let Err(error) = backend.refresh().await {
                            backend.notice = Some(format!("Refresh failed: {error:#}"));
                            let _ = backend.publish_snapshot();
                        }
                    }
                    _ = status_interval.tick() => {
                        if let Err(error) = backend.poll_active_connection().await {
                            backend.notice = Some(format!("Status check failed: {error:#}"));
                            let _ = backend.publish_snapshot();
                        }
                    }
                    _ = local_ip_interval.tick() => {
                        // Refresh the user's real public IP info every
                        // 5 minutes so the right-pane card stays current
                        // even on long-lived idle sessions.
                        if let Err(error) = backend.refresh_local_ip().await {
                            tracing::warn!("local IP refresh failed: {error:#}");
                        }
                    }
                    Some(command) = async_rx.recv() => {
                        if let Err(error) = backend.handle_command(command).await {
                            backend.notice = Some(format!("Action failed: {error:#}"));
                            let _ = backend.publish_snapshot();
                        }
                    }
                }
            }
        });
    });
}

struct BackendState {
    paths: AppPaths,
    config: ClientConfig,
    client_state: ClientState,
    servers: BTreeMap<String, ServerSummary>,
    active_connection: Option<ActiveConnection>,
    notice: Option<String>,
    event_tx: Sender<ClientEvent>,
    local_server_active: bool,
    local_server_peers: u32,
    local_server_port: u16,
    local_server_public_key: String,
    command_tx: tokio_mpsc::UnboundedSender<ClientCommand>,
    geoip: Option<Arc<GeoIpStack>>,
    /// The user's real (non-Tor) public IP + GeoIP details. Populated by
    /// `refresh_local_ip()` and shipped to the UI via `ClientSnapshot`. The
    /// right-pane "Your IP Details" card renders this with no conditions.
    local_ip_info: Option<vpn_suite_core::model::TorExitInfo>,
    /// Unix timestamp of the most recent `refresh_local_ip()` success; UI
    /// shows it as "Updated 12s ago".
    local_ip_refresh_unix: u64,
    /// Set once `apply_tor_system_route()` has actually brought the Wintun
    /// tunnel up. Toggled back to false by `remove_tor_system_route()` or by
    /// `disconnect()`.
    tor_system_route_active: bool,
    /// SOCKS5 port the most recent `connect_to_tor()` allocated. The backend
    /// stashes this so a later `apply_tor_system_route()` can attach tun2proxy
    /// to the right listener without re-discovering it.
    tor_socks_port: Option<u16>,
    /// Auto-retry counter for enabling the Tor system route while Tor is
    /// still bootstrapping (helper reports no established guards yet).
    tor_route_attempts: u32,
    /// Real connect/disconnect progress (0..1) driven by completed stages.
    op_progress: f32,
    op_progress_label: Option<String>,
    /// `"connect"` | `"disconnect"` | None
    op_progress_kind: Option<String>,
    /// Incremented on each public-IP refresh so the globe re-pans every time.
    globe_pan_token: u64,
}

impl BackendState {
    fn new(
        paths: AppPaths,
        config: ClientConfig,
        client_state: ClientState,
        event_tx: Sender<ClientEvent>,
        command_tx: tokio_mpsc::UnboundedSender<ClientCommand>,
    ) -> Self {
        Self {
            paths,
            config,
            client_state,
            servers: BTreeMap::new(),
            active_connection: None,
            notice: Some(String::from(
                "Ready. Discovery runs every 30 seconds and connected sessions are checked every 5 seconds.",
            )),
            event_tx,
            local_server_active: false,
            local_server_peers: 0,
            local_server_port: 0,
            local_server_public_key: String::new(),
            command_tx,
            geoip: None,
            // `None` until the first `refresh_local_ip()` succeeds. The
            // right-pane IP Details card shows a pulsing "Looking up..."
            // placeholder in this state.
            local_ip_info: None,
            // 0 until the first refresh; UI hides the "Updated Xs ago"
            // label when this is 0 so we don't display "Updated 47 years ago".
            local_ip_refresh_unix: 0,
            // Set true by `apply_tor_system_route()` after the Wintun
            // adapter + 0.0.0.0/0 route are actually applied (or false
            // again after `remove_tor_system_route()` / disconnect). The
            // Tor card gates the "Disable" / "Enable" button on this.
            tor_system_route_active: false,
            // Set by `connect_to_tor()` once it's reserved a free
            // 127.0.0.1 port for the local SOCKS5 listener. The
            // `ApplyTorSystemRoute` handler reads it so it knows which
            // port to attach tun2proxy to.
            tor_socks_port: None,
            tor_route_attempts: 0,
            op_progress: 0.0,
            op_progress_label: None,
            op_progress_kind: None,
            globe_pan_token: 0,
        }
    }

    /// Publish a real progress stage (0..1) so the UI bar tracks backend work.
    fn set_op_progress(&mut self, kind: &str, pct: f32, label: &str) {
        self.op_progress = pct.clamp(0.0, 1.0);
        self.op_progress_label = Some(label.to_string());
        self.op_progress_kind = Some(kind.to_string());
        let _ = self.publish_snapshot();
    }

    fn clear_op_progress(&mut self) {
        self.op_progress = 0.0;
        self.op_progress_label = None;
        self.op_progress_kind = None;
    }

    fn publish_snapshot(&self) -> Result<()> {
        let mut servers = self.servers.values().cloned().collect::<Vec<_>>();
        servers.sort_by(|left, right| {
            right
                .online
                .cmp(&left.online)
                .then_with(|| left.name.cmp(&right.name))
        });
        let (local_server_selected_ipv4, local_server_selected_ipv6) =
            self.read_local_server_selection().unwrap_or_default();

        self.event_tx.send(ClientEvent::Snapshot(ClientSnapshot {
            servers,
            active_connection: self.active_connection.clone(),
            notice: self.notice.clone(),
            last_refresh_unix: unix_now(),
            local_server_active: self.local_server_active,
            local_server_peers: self.local_server_peers,
            local_server_port: self.local_server_port,
            local_server_public_key: self.local_server_public_key.clone(),
            local_server_selected_ipv4,
            local_server_selected_ipv6,
            // Surface the user's real public IP (NOT through Tor) so the
            // right-pane "Your IP Details" card can render the moment the
            // UI wakes up — and keep rendering through every refresh.
            local_ip_info: self.local_ip_info.clone(),
            local_ip_refresh_unix: self.local_ip_refresh_unix,
            // Whether `apply_tor_system_route()` has actually brought the
            // Wintun tunnel up. The Tor card shows "ACTIVE" / "OFF" based
            // on this and gates the Enable/Disable button.
            tor_system_route_active: self.tor_system_route_active,
            tor_socks_port: self.tor_socks_port,
            op_progress: self.op_progress,
            op_progress_label: self.op_progress_label.clone(),
            op_progress_kind: self.op_progress_kind.clone(),
            globe_pan_token: self.globe_pan_token,
        }))?;
        Ok(())
    }

    fn persist_client_state(&mut self) -> Result<()> {
        self.client_state.last_active_connection = self.active_connection.clone();
        save_client_state(&self.paths, &self.client_state)
    }

    fn read_local_server_selection(&self) -> Result<(Vec<String>, Vec<String>)> {
        let paths = server_paths()?;
        let config = load_or_create_server_config(&paths, &server_bootstrap_options())?;
        let net_info = HostNetInfo::query();
        Ok((
            net_info.effective_selected_global_ipv4(&config.selected_global_ipv4),
            net_info.effective_selected_global_ipv6(&config.selected_global_ipv6),
        ))
    }

    fn save_local_server_selection(
        &mut self,
        selected_ipv4: Vec<String>,
        selected_ipv6: Vec<String>,
    ) -> Result<()> {
        let paths = server_paths()?;
        let mut config = load_or_create_server_config(&paths, &server_bootstrap_options())?;
        let net_info = HostNetInfo::query();
        config.selected_global_ipv4 = selected_ipv4
            .into_iter()
            .filter(|ip| net_info.global_ipv4.iter().any(|candidate| candidate == ip))
            .collect();
        config.selected_global_ipv6 = selected_ipv6
            .into_iter()
            .filter(|ip| net_info.global_ipv6.iter().any(|candidate| candidate == ip))
            .collect();
        config.selected_global_ipv4.sort();
        config.selected_global_ipv4.dedup();
        config.selected_global_ipv6.sort();
        config.selected_global_ipv6.dedup();
        save_server_config(&paths, &config)?;

        self.notice = Some(format!(
            "Saved server address selection: {} IPv4 / {} IPv6 selected.",
            config.selected_global_ipv4.len(),
            config.selected_global_ipv6.len()
        ));
        Ok(())
    }

    fn prepare_local_server_config_for_start(&mut self) -> Result<bool> {
        let paths = server_paths()?;
        let mut config = load_or_create_server_config(&paths, &server_bootstrap_options())?;
        let net_info = HostNetInfo::query();
        if !net_info.has_global_connectivity() {
            self.notice = Some(String::from(
                "No global/public IPv4 or IPv6 address was detected on this machine. Local/LAN addresses are not allowed for server hosting.",
            ));
            return Ok(false);
        }

        let selected_ipv4 = net_info.effective_selected_global_ipv4(&config.selected_global_ipv4);
        let selected_ipv6 = net_info.effective_selected_global_ipv6(&config.selected_global_ipv6);
        if selected_ipv4.is_empty() && selected_ipv6.is_empty() {
            self.notice = Some(String::from(
                "No public IPv4 or IPv6 address is selected for hosting. Open Settings on the hosting card and select at least one global address.",
            ));
            return Ok(false);
        }

        config.selected_global_ipv4 = selected_ipv4;
        config.selected_global_ipv6 = selected_ipv6;
        self.local_server_port = config.listen_port;
        self.local_server_public_key = config.wireguard_keys.public_key.clone();
        save_server_config(&paths, &config)?;
        Ok(true)
    }

    async fn handle_command(&mut self, command: ClientCommand) -> Result<()> {
        match command {
            ClientCommand::RefreshNow => self.refresh().await,
            ClientCommand::AddHost(host) => {
                if looks_like_wireguard_public_key(&host) {
                    self.notice = Some(String::from(
                        "That looks like a WireGuard public key, not a host or IP address. Use one of the server endpoint addresses shown on the hosting panel instead.",
                    ));
                    return self.publish_snapshot();
                }
                if !self.config.known_hosts.iter().any(|known| known == &host) {
                    self.config.known_hosts.push(host.clone());
                    save_client_config(&self.paths, &self.config)?;
                    self.notice = Some(format!("Saved manual host {host}."));
                }
                self.refresh().await
            }
            ClientCommand::ApplyTunnel => self.apply_kernel_tunnel().await,
            ClientCommand::RemoveTunnel => self.remove_kernel_tunnel(),
            ClientCommand::Connect {
                server_id,
                endpoint,
                server_name,
                password,
                protocol,
            } => {
                self.connect_to_server(server_id, endpoint, server_name, password, protocol)
                    .await
            }
            ClientCommand::ConnectOvpnFile {
                id,
                username,
                password,
            } => self.connect_to_ovpn_file(id, username, password).await,
            ClientCommand::ConnectSelectedOvpn {
                username,
                password,
            } => {
                if let Some(id) = crate::db::get_selected_ovpn_id() {
                    self.connect_to_ovpn_file(id, username, password).await
                } else if let Some(first) = crate::db::get_ovpn_configs()
                    .ok()
                    .and_then(|c| c.into_iter().next())
                {
                    self.connect_to_ovpn_file(first.id, username, password).await
                } else {
                    self.notice = Some(String::from(
                        "No OpenVPN server selected. Import a .ovpn profile first.",
                    ));
                    self.publish_snapshot()
                }
            }
            ClientCommand::EnrichOvpnProfile(id) => self.enrich_ovpn_profile(id).await,
            ClientCommand::EnsureOpenVpnBinary => {
                match crate::ovpn::ensure_openvpn_exe().await {
                    Ok(path) => {
                        self.notice =
                            Some(format!("OpenVPN ready: {}", path.display()));
                    }
                    Err(error) => {
                        self.notice = Some(format!("OpenVPN setup: {error:#}"));
                    }
                }
                self.publish_snapshot()
            }
            ClientCommand::ConnectWireGuard { id } => self.connect_to_wireguard(id).await,
            ClientCommand::EnrichWgProfile(id) => self.enrich_wg_profile(id).await,
            ClientCommand::ConnectPptp { id } => self.connect_to_pptp(id).await,
            ClientCommand::ConnectOutline { id, system_wide } => {
                self.connect_to_outline(id, system_wide).await
            }
            ClientCommand::EnrichOutlineProfile(id) => self.enrich_outline_profile(id).await,
            ClientCommand::ConnectTor => self.connect_to_tor().await,
            ClientCommand::ApplyTorSystemRoute => self.apply_tor_system_route().await,
            ClientCommand::RemoveTorSystemRoute => self.remove_tor_system_route().await,
            ClientCommand::SetTorIsolationMode(mode) => {
                crate::db::set_tor_isolation_mode(&mode);
                self.notice = Some(if mode == "apps" {
                    String::from(
                        "Tor isolation: selected apps via SOCKS5 (system-wide route off).",
                    )
                } else {
                    String::from(
                        "Tor isolation: whole PC (system-wide Wintun route when enabled).",
                    )
                });
                let _ = self.event_tx.send(ClientEvent::ReloadTorApps);
                self.publish_snapshot()
            }
            ClientCommand::LaunchTorIsolatedApp(id) => {
                self.launch_tor_isolated_app(id).await
            }
            ClientCommand::ShowMainWindow => {
                if let Some(ctx) = APP_EGUI_CTX.get() {
                    // Undo both X11 hide and Wayland minimize. Visible(true)
                    // is a no-op on Wayland but harmless; Minimized(false)
                    // restores a minimized Wayland window.
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    ctx.request_repaint();
                }
                tracing::info!("ShowMainWindow: tray requested restore");
                Ok(())
            }
            ClientCommand::QuitApp | ClientCommand::SignalQuit => {
                self.teardown_and_quit().await
            }
            ClientCommand::RefreshLocalIp => {
                if let Err(error) = self.refresh_local_ip().await {
                    self.notice = Some(format!("Local IP refresh failed: {error:#}"));
                }
                self.publish_snapshot()
            }
            ClientCommand::GeoIpReady(stack) => {
                self.geoip = Some(stack);
                Ok(())
            }
            ClientCommand::TorConnected { ip, country, socks_port, exit_info } => {
                self.tor_socks_port = Some(socks_port);
                if let Some(active) = &mut self.active_connection {
                    if active.server_id == "tor_local" {
                        active.phase = ConnectionPhase::Connected;
                        // Prefer a real exit IP over the local SOCKS endpoint.
                        if !ip.is_empty() && ip != "Unknown IP" {
                            active.endpoint = ip.clone();
                        }
                        // Prefer ISO from full exit_info when present.
                        let cc_from_info = exit_info
                            .as_ref()
                            .map(|e| e.country_code.trim().to_uppercase())
                            .filter(|c| c.len() == 2 && c.chars().all(|ch| ch.is_ascii_alphabetic()));
                        let cc_upper = country.to_uppercase();
                        let is_real_iso = cc_upper.len() == 2
                            && cc_upper.chars().all(|c| c.is_ascii_alphabetic());
                        let final_cc = cc_from_info.or_else(|| is_real_iso.then_some(cc_upper));

                        if let Some(ref cc) = final_cc {
                            active.country_code = Some(cc.clone());
                            let friendly = exit_info
                                .as_ref()
                                .map(|e| e.country.as_str())
                                .filter(|n| !n.is_empty() && *n != "Resolving…")
                                .map(|n| n.to_string())
                                .or_else(|| crate::tor_geo::country_name(cc))
                                .unwrap_or_else(|| cc.clone());
                            active.server_name = format!("Tor Exit: {friendly}");
                        } else {
                            active.country_code = None;
                            active.server_name = if ip != "Unknown IP" && !ip.is_empty() {
                                format!("Tor Exit: {ip}")
                            } else {
                                String::from("Tor Exit: Unknown")
                            };
                        }
                        // Full GeoIP card + exact lat/lon for the globe beacon.
                        active.tor_exit_info = exit_info.clone();
                    }
                }

                // Immediately push exit geo into "Your IP" so the right pane +
                // globe track the public address the world sees through Tor.
                if let Some(ref info) = exit_info {
                    if !info.ip.is_empty() && info.ip != "Unknown IP" {
                        self.local_ip_info = Some(info.clone());
                        self.local_ip_refresh_unix = unix_now();
                    }
                }

                // Computer-wide tunnel via temporary Wintun + tun2proxy — not a
                // permanent OS proxy. Requires Administrator; SOCKS stays up either way.
                // In "apps" isolation mode we intentionally leave Tor as SOCKS5-only
                // so only selected apps (launched via Tor) use the proxy.
                // Skip re-start when a refined GeoIP update arrives and the tunnel
                // is already running (avoids route flapping).
                let isolation = crate::db::get_tor_isolation_mode();
                let apps_only = isolation == "apps";
                #[cfg(target_os = "windows")]
                {
                    if apps_only {
                        self.tor_system_route_active = false;
                        self.notice = Some(format!(
                            "Tor SOCKS5 up on 127.0.0.1:{socks_port} (app isolation mode). Launch selected apps through Tor — system-wide route is off."
                        ));
                    } else if self.tor_system_route_active
                        && platform::is_tor_tunnel_running()
                    {
                        let label = self
                            .active_connection
                            .as_ref()
                            .map(|a| a.server_name.clone())
                            .unwrap_or_else(|| String::from("Tor"));
                        self.notice = Some(format!(
                            "Tor exit updated: {label}. System-wide VPN still active on SOCKS5 127.0.0.1:{socks_port}."
                        ));
                    } else if platform::is_elevated() {
                        match platform::start_tor_system_tunnel(socks_port) {
                            Ok(()) => {
                                self.tor_system_route_active = true;
                                self.notice = Some(format!(
                                    "Tor connected. System-wide VPN ACTIVE (Wintun → SOCKS5 127.0.0.1:{socks_port}). All apps route through Tor; disconnect restores normal routing."
                                ));
                            }
                            Err(error) => {
                                self.tor_system_route_active = false;
                                tracing::error!("Tor system tunnel failed: {error:#}");
                                self.notice = Some(format!(
                                    "Tor SOCKS5 is up on 127.0.0.1:{socks_port}, but system-wide tunnel failed: {error:#}"
                                ));
                            }
                        }
                    } else {
                        self.tor_system_route_active = false;
                        self.notice = Some(format!(
                            "Tor SOCKS5 is up on 127.0.0.1:{socks_port} (browser proxy OK). System-wide VPN needs Administrator — click Connect again and accept UAC, or use “Enable System-Wide Routing”."
                        ));
                    }
                }
                #[cfg(target_os = "linux")]
                {
                    // Never flip the system route while Tor is still
                    // bootstrapping: its guard connections would get routed
                    // into the TUN→SOCKS loop (stuck at ~18%). Wait until a
                    // real exit IP is known, then enable.
                    let geo_ready = !ip.is_empty() && ip != "Unknown IP";
                    if apps_only {
                        self.tor_system_route_active = false;
                        self.notice = Some(format!(
                            "Tor SOCKS5 up on 127.0.0.1:{socks_port} (app isolation mode). Launch selected apps through Tor — system-wide route is off."
                        ));
                        return Ok(());
                    }
                    if !geo_ready {
                        if isolation == "system" && self.tor_route_attempts < 24 {
                            self.tor_route_attempts += 1;
                            let n = self.tor_route_attempts;
                            self.notice = Some(format!(
                                "Tor bootstrapping… enabling system-wide VPN automatically ({n}/24)"
                            ));
                            let _ = self.publish_snapshot();
                            let tx2 = self.command_tx.clone();
                            tokio::spawn(async move {
                                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                                let _ = tx2.send(ClientCommand::ApplyTorSystemRoute);
                            });
                        } else if self.tor_route_attempts >= 24 {
                            self.tor_system_route_active = false;
                            self.notice = Some(String::from(
                                "Tor SOCKS5 is up but bootstrap is stuck — check your connection, then click Connect again.",
                            ));
                        }
                        return Ok(());
                    }
                    self.tor_route_attempts = 0;
                    // ProtonVPN-style: ask the root helper daemon first — no
                    // polkit dialog, no second window.
                    let helper_reply = crate::helper::send(
                        "tor_start",
                        serde_json::json!({ "socks_port": socks_port }),
                    );
                    match helper_reply {
                        Some(reply) => {
                            if reply.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                                self.tor_system_route_active = true;
                                self.notice = Some(String::from(
                                    "Tor VPN connected — all apps now use Tor.",
                                ));
                            } else {
                                let err = reply
                                    .get("error")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("helper error")
                                    .to_string();
                                self.tor_system_route_active = false;
                                tracing::error!("helper tor_start failed: {err}");
                                self.notice = Some(format!(
                                    "Tor connected (browser proxy OK); system-wide failed: {err}"
                                ));
                            }
                        }
                        None => {
                            if apps_only {
                                self.tor_system_route_active = false;
                                self.notice = Some(format!(
                                    "Tor SOCKS5 up on 127.0.0.1:{socks_port} (app isolation mode). Launch selected apps through Tor — system-wide route is off."
                                ));
                            } else if self.tor_system_route_active
                                && platform::is_tor_tunnel_running()
                            {
                                let label = self
                                    .active_connection
                                    .as_ref()
                                    .map(|a| a.server_name.clone())
                                    .unwrap_or_else(|| String::from("Tor"));
                                self.notice = Some(format!(
                                    "Tor exit updated: {label}. System-wide VPN still active on SOCKS5 127.0.0.1:{socks_port}."
                                ));
                            } else if platform::is_elevated() {
                                match platform::start_tor_system_tunnel(socks_port) {
                                    Ok(()) => {
                                        self.tor_system_route_active = true;
                                        self.notice = Some(String::from(
                                            "Tor VPN connected — all apps now use Tor. Disconnect to restore.",
                                        ));
                                    }
                                    Err(error) => {
                                        self.tor_system_route_active = false;
                                        tracing::error!("Tor system tunnel failed: {error:#}");
                                        self.notice = Some(format!(
                                            "Tor connected, but system-wide failed (will use browser proxy): {error:#}"
                                        ));
                                    }
                                }
                            } else {
                                // One-click fallback (no helper): auto-prompt pkexec once.
                                self.tor_system_route_active = false;
                                self.notice = Some(String::from("Tor connected — enabling system-wide VPN…"));
                                let _ = self.publish_snapshot();
                                let tx2 = self.command_tx.clone();
                                tokio::spawn(async move {
                                    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                                    let _ = tx2.send(ClientCommand::ApplyTorSystemRoute);
                                });
                            }
                        }
                    }
                }
                #[cfg(not(any(target_os = "windows", target_os = "linux")))]
                {
                    let _ = apps_only;
                    self.notice = Some(format!(
                        "Tor SOCKS5 proxy listening on 127.0.0.1:{socks_port}."
                    ));
                }

                // Reverse-IP refresh of Your IP through the live Tor path so
                // globe + right pane stay in sync after 100% connected.
                let _ = self.refresh_public_ip_after_tunnel().await;
                let _ = self.publish_snapshot();
                Ok(())
            },
            ClientCommand::Disconnect => self.disconnect().await,
            ClientCommand::SaveLocalServerSelection { ipv4, ipv6 } => {
                self.save_local_server_selection(ipv4, ipv6)?;
                self.publish_snapshot()
            }
            ClientCommand::StartLocalServer => {
                if !self.prepare_local_server_config_for_start()? {
                    return self.publish_snapshot();
                }

                #[cfg(target_os = "windows")]
                {
                    // Force terminate any orphaned/hung vpn-server process to free the UDP socket port.
                    let _ = stop_windows_server_process();
                    std::thread::sleep(std::time::Duration::from_millis(300));

                    match ensure_windows_server_host_setup() {
                        Ok(setup_output) => match spawn_windows_server_process() {
                            Ok(child) => {
                                save_windows_server_pid(child.id())?;
                                self.notice = Some(format!(
                                    "Local server launch requested. Firewall prep: {} PID: {}",
                                    command_output_detail(&setup_output),
                                    child.id()
                                ));
                            }
                            Err(error) => {
                                self.notice = Some(format!(
                                    "Firewall prep succeeded, but starting vpn-server.exe failed: {error}. Setup: {}",
                                    command_output_detail(&setup_output)
                                ));
                            }
                        },
                        Err(error) => {
                            self.notice = Some(format!(
                                "Automatic Windows host setup failed before server launch: {error}"
                            ));
                        }
                    }
                }
                #[cfg(target_os = "linux")]
                {
                    let start = std::process::Command::new("systemctl")
                        .args(["start", "zeronode-vpn-server.service"])
                        .output();
                    self.notice = Some(match start {
                        Ok(output) if output.status.success() => String::from(
                            "Local systemd service (zeronode-vpn-server) start requested.",
                        ),
                        Ok(output) => format!(
                            "Could not start zeronode-vpn-server.service. {}",
                            command_output_detail(&output)
                        ),
                        Err(error) => {
                            format!("Could not start zeronode-vpn-server.service: {error}")
                        }
                    });
                }
                #[cfg(not(any(target_os = "windows", target_os = "linux")))]
                {
                    self.notice = Some(String::from(
                        "Service control is only supported on Windows and Linux.",
                    ));
                }
                self.publish_snapshot()
            }
            ClientCommand::StopLocalServer => {
                #[cfg(target_os = "windows")]
                {
                    self.notice = Some(match stop_windows_server_process() {
                        Ok(summary) => summary,
                        Err(error) => format!("Could not stop local vpn-server.exe: {error}"),
                    });
                }
                #[cfg(target_os = "linux")]
                {
                    let stop = std::process::Command::new("systemctl")
                        .args(["stop", "zeronode-vpn-server.service"])
                        .output();
                    self.notice = Some(match stop {
                        Ok(output) if output.status.success() => String::from(
                            "Local systemd service (zeronode-vpn-server) stop requested.",
                        ),
                        Ok(output) => format!(
                            "Could not stop zeronode-vpn-server.service. {}",
                            command_output_detail(&output)
                        ),
                        Err(error) => {
                            format!("Could not stop zeronode-vpn-server.service: {error}")
                        }
                    });
                }
                #[cfg(not(any(target_os = "windows", target_os = "linux")))]
                {
                    self.notice = Some(String::from(
                        "Service control is only supported on Windows and Linux.",
                    ));
                }
                self.publish_snapshot()
            }
        }
    }

    async fn connect_to_server(
        &mut self,
        server_id: String,
        endpoint: String,
        server_name: String,
        password: Option<String>,
        _protocol: vpn_suite_core::model::VpnProtocol,
    ) -> Result<()> {
        self.prune_cooldowns()?;
        let now = unix_now();
        if let Some(entry) = self.client_state.cooldowns.get(&server_id) {
            if entry.until_unix > now {
                self.active_connection = Some(ActiveConnection {
                    server_id: server_id.clone(),
                    server_name,
                    endpoint,
                    protocol: vpn_suite_core::model::VpnProtocol::WireGuard,
        phase: ConnectionPhase::Cooldown,
                    connected_at_unix: None,
                    attempt_count: 0,
                    session_id: None,
                    reserved_client_ip: None,
                    server_internal_ip: None,
                    tunnel_profile_path: None,
                    cooldown_until_unix: Some(entry.until_unix),
                    tor_exit_info: None,
                    country_code: None,
                    last_status_unix: None,
                });
                self.notice = Some(format!(
                    "Access Denied — Too many failed attempts. Try again in {}.",
                    format_remaining(entry.until_unix.saturating_sub(now))
                ));
                self.persist_client_state()?;
                return self.publish_snapshot();
            }
        }

        self.active_connection = Some(ActiveConnection {
            server_id: server_id.clone(),
            server_name: server_name.clone(),
            endpoint: endpoint.clone(),
            protocol: vpn_suite_core::model::VpnProtocol::WireGuard,
        phase: ConnectionPhase::Connecting,
            connected_at_unix: None,
            attempt_count: 1,
            session_id: None,
            reserved_client_ip: None,
            server_internal_ip: None,
            tunnel_profile_path: None,
            cooldown_until_unix: None,
            tor_exit_info: None,
            country_code: None,
            last_status_unix: None,
        });
        self.persist_client_state()?;
        self.publish_snapshot()?;

        match attempt_auth(&self.config, &endpoint, &server_id, password).await {
            Ok(result) => {
                if result.accepted {
                    self.clear_cooldown(&server_id)?;
                    self.client_state.last_connected_server_id = Some(server_id.clone());
                    let lease = result.lease.clone();
                    let mut notice = result.message.clone();
                    let discovered_server = self.servers.get(&server_id).cloned();
                    let tunnel_profile_path = if let Some(lease) = lease.as_ref() {
                        match self.write_tunnel_artifact(&server_id, lease, _protocol) {
                            Ok(path) => {
                                self.client_state.last_tunnel_profile_path = Some(path.clone());
                                Some(path)
                            }
                            Err(error) => {
                                notice = format!(
                                    "Control session established, but profile generation failed: {error:#}"
                                );
                                None
                            }
                        }
                    } else {
                        None
                    };

                    self.active_connection = Some(connection_from_lease(
                        &server_id,
                        &server_name,
                        &endpoint,
                        lease.as_ref(),
                        tunnel_profile_path.clone(),
                        discovered_server.as_ref().map(|s| s.country_code.as_str()),
                    ));
                    if let (Some(server), Some(lease)) =
                        (discovered_server.as_ref(), lease.as_ref())
                    {
                        let tunnel_notice =
                            self.apply_platform_tunnel_for_connection(server, lease, _protocol);
                        notice = format!("{notice} {tunnel_notice}");
                    }
                    self.persist_client_state()?;
                    self.notice = Some(notice);
                    self.update_server_message(
                        &server_id,
                        Some(match tunnel_profile_path {
                            Some(path) => format!("VPN tunnel active. Profile: {path}"),
                            None => String::from("VPN tunnel active."),
                        }),
                    );
                } else if let Some(until) = result.cooldown_until_unix {
                    self.set_cooldown(
                        server_id.clone(),
                        endpoint.clone(),
                        until,
                        result.message.clone(),
                    )?;
                    self.active_connection = Some(ActiveConnection {
                        server_id: server_id.clone(),
                        server_name,
                        endpoint,
                        protocol: vpn_suite_core::model::VpnProtocol::WireGuard,
        phase: ConnectionPhase::Cooldown,
                        connected_at_unix: None,
                        attempt_count: 0,
                        session_id: None,
                        reserved_client_ip: None,
                        server_internal_ip: None,
                        tunnel_profile_path: None,
                        cooldown_until_unix: Some(until),
                        tor_exit_info: None,
                        country_code: None,
                        last_status_unix: None,
                    });
                    self.persist_client_state()?;
                    self.notice = Some(result.message.clone());
                    self.update_server_message(&server_id, Some(result.message));
                } else {
                    self.active_connection = Some(ActiveConnection {
                        server_id: server_id.clone(),
                        server_name,
                        endpoint,
                        protocol: vpn_suite_core::model::VpnProtocol::WireGuard,
        phase: ConnectionPhase::Error,
                        connected_at_unix: None,
                        attempt_count: 1,
                        session_id: None,
                        reserved_client_ip: None,
                        server_internal_ip: None,
                        tunnel_profile_path: None,
                        cooldown_until_unix: None,
                        tor_exit_info: None,
                        country_code: None,
                        last_status_unix: None,
                    });
                    self.persist_client_state()?;
                    self.notice = Some(result.message.clone());
                    self.update_server_message(&server_id, Some(result.message));
                }
            }
            Err(error) => {
                self.active_connection = Some(ActiveConnection {
                    server_id: server_id.clone(),
                    server_name,
                    endpoint,
                    protocol: vpn_suite_core::model::VpnProtocol::WireGuard,
        phase: ConnectionPhase::Error,
                    connected_at_unix: None,
                    attempt_count: 1,
                    session_id: None,
                    reserved_client_ip: None,
                    server_internal_ip: None,
                    tunnel_profile_path: None,
                    cooldown_until_unix: None,
                    tor_exit_info: None,
                    country_code: None,
                    last_status_unix: None,
                });
                self.persist_client_state()?;
                self.notice = Some(format!("Could not reach the server: {error:#}"));
                self.update_server_message(
                    &server_id,
                    Some(String::from("Authentication attempt timed out or failed.")),
                );
            }
        }

        self.apply_cooldowns_to_servers();
        self.publish_snapshot()
    }

    async fn enrich_ovpn_profile(&mut self, id: i64) -> Result<()> {
        let Some(config) = crate::db::get_ovpn_config(id)? else {
            return Ok(());
        };
        let parsed = crate::ovpn::parse_ovpn(&config.content);
        let host = if !parsed.remote_host.is_empty() {
            parsed.remote_host.clone()
        } else {
            config.remote_host.clone().unwrap_or_default()
        };
        if host.is_empty() {
            self.notice = Some(format!(
                "OpenVPN profile '{}' has no remote host to geolocate.",
                config.name
            ));
            let _ = self.event_tx.send(ClientEvent::ReloadOvpnConfigs);
            return self.publish_snapshot();
        }
        let info = crate::ovpn::enrich_remote_location(&host).await;
        if let Some(info) = info {
            crate::db::update_ovpn_location(
                id,
                &info,
                Some(&host),
                Some(parsed.remote_port),
                Some(&parsed.proto),
                if parsed.cipher.is_empty() {
                    None
                } else {
                    Some(parsed.cipher.as_str())
                },
                if parsed.auth.is_empty() {
                    None
                } else {
                    Some(parsed.auth.as_str())
                },
            )?;
            self.notice = Some(format!(
                "OpenVPN server '{}': {} · {} {}",
                config.name,
                if info.ip.is_empty() {
                    host.as_str()
                } else {
                    info.ip.as_str()
                },
                info.country,
                info.city
            ));
        } else {
            // Still persist parsed remote even without GeoIP.
            let partial = vpn_suite_core::model::TorExitInfo {
                ip: crate::ovpn::resolve_remote_ip(&host).unwrap_or_default(),
                ..Default::default()
            };
            let _ = crate::db::update_ovpn_location(
                id,
                &partial,
                Some(&host),
                Some(parsed.remote_port),
                Some(&parsed.proto),
                None,
                None,
            );
            self.notice = Some(format!(
                "Imported '{}'; location lookup incomplete (offline?).",
                config.name
            ));
        }
        let _ = self.event_tx.send(ClientEvent::ReloadOvpnConfigs);
        self.publish_snapshot()
    }

    async fn connect_to_ovpn_file(
        &mut self,
        id: i64,
        username: Option<String>,
        password: Option<String>,
    ) -> Result<()> {
        let config = match crate::db::get_ovpn_config(id)? {
            Some(c) => c,
            None => {
                self.notice = Some(String::from("OpenVPN profile not found."));
                return self.publish_snapshot();
            }
        };

        crate::db::set_selected_ovpn_id(Some(id));
        std::fs::create_dir_all(&self.paths.profiles_dir)?;

        // Persist original profile and (optional) credentials before elevation
        // so the elevated process can reconnect without re-prompting.
        let source_path = self.paths.profiles_dir.join(format!("ovpn_{id}.ovpn"));
        std::fs::write(&source_path, &config.content)?;
        let auth_path = crate::ovpn::auth_file_path(&self.paths.profiles_dir, id);
        if let (Some(user), Some(pass)) = (username.as_ref(), password.as_ref()) {
            crate::ovpn::write_auth_file(&auth_path, user, pass)?;
        }

        let needs_auth = crate::ovpn::needs_auth_user_pass(&config.content);
        if needs_auth && !auth_path.is_file() {
            self.notice = Some(format!(
                "OpenVPN profile '{}' requires username/password (auth-user-pass). Enter credentials and Connect again.",
                config.name
            ));
            return self.publish_snapshot();
        }

        // System-wide OpenVPN needs Admin (TAP/Wintun + route add). Mirror Tor UAC.
        #[cfg(target_os = "windows")]
        {
            if !platform::is_elevated() {
                let id_arg = id.to_string();
                match platform::relaunch_elevated_with_args(&[
                    "--auto-connect-ovpn",
                    &id_arg,
                ]) {
                    Ok(()) => {
                        self.notice = Some(String::from(
                            "Accepted UAC — restarting elevated for system-wide OpenVPN…",
                        ));
                        let _ = self.publish_snapshot();
                        platform::exit_after_relaunch();
                    }
                    Err(error) => {
                        self.notice = Some(format!(
                            "System-wide OpenVPN needs Administrator ({error:#}). Accept UAC or Run as administrator."
                        ));
                        return self.publish_snapshot();
                    }
                }
            }
        }
        #[cfg(target_os = "linux")]
        {
            if !platform::is_elevated() {
                let id_arg = id.to_string();
                match platform::relaunch_elevated_with_args(&[
                    "--auto-connect-ovpn",
                    &id_arg,
                ]) {
                    Ok(()) => {
                        self.notice = Some(String::from(
                            "Accepted — restarting elevated for system-wide OpenVPN (pkexec)…",
                        ));
                        let _ = self.publish_snapshot();
                        platform::exit_after_relaunch();
                    }
                    Err(error) => {
                        self.notice = Some(format!(
                            "System-wide OpenVPN needs Administrator ({error:#}). Authenticate via pkexec or run with sudo."
                        ));
                        return self.publish_snapshot();
                    }
                }
            }
        }

        self.notice = Some(String::from(
            "Preparing OpenVPN binary and bringing up system tunnel…",
        ));
        let _ = self.publish_snapshot();
        let openvpn_exe = match crate::ovpn::ensure_openvpn_exe().await {
            Ok(p) => p,
            Err(error) => {
                self.notice = Some(format!("OpenVPN unavailable: {error:#}"));
                return self.publish_snapshot();
            }
        };

        let auth_opt = if auth_path.is_file() {
            Some(auth_path.as_path())
        } else {
            None
        };
        let runtime_profile = match crate::ovpn::prepare_runtime_profile_with_driver(
            &config.content,
            &self.paths.profiles_dir,
            id,
            auth_opt,
            Some(openvpn_exe.as_path()),
        ) {
            Ok(p) => p,
            Err(error) => {
                self.notice = Some(format!("Could not prepare OpenVPN profile: {error:#}"));
                return self.publish_snapshot();
            }
        };

        let log_path = crate::ovpn::log_file_path(&self.paths.profiles_dir, id);
        let endpoint = config.endpoint_label();
        let country_code = config.country_code.clone();
        let exit_info = {
            let info = config.location_info();
            if info.ip.is_empty() && info.country_code.is_empty() {
                None
            } else {
                Some(info)
            }
        };

        self.active_connection = Some(ActiveConnection {
            server_id: format!("ovpn_{id}"),
            server_name: config.name.clone(),
            endpoint: endpoint.clone(),
            protocol: vpn_suite_core::model::VpnProtocol::OpenVPN,
            phase: ConnectionPhase::Connecting,
            connected_at_unix: Some(unix_now()),
            attempt_count: config.fail_count as u32,
            session_id: None,
            reserved_client_ip: None,
            server_internal_ip: None,
            tunnel_profile_path: Some(runtime_profile.to_string_lossy().to_string()),
            cooldown_until_unix: None,
            tor_exit_info: exit_info,
            country_code,
            last_status_unix: Some(unix_now()),
        });
        let _ = self.publish_snapshot();

        crate::ovpn::kill_openvpn_processes();
        // Brief pause so TAP/Wintun releases cleanly after kill.
        tokio::time::sleep(Duration::from_millis(500)).await;

        match crate::ovpn::spawn_openvpn_with_log(
            &openvpn_exe,
            &runtime_profile,
            Some(&log_path),
        ) {
            Ok(child) => {
                std::mem::forget(child);
                self.notice = Some(format!(
                    "OpenVPN starting for '{}' ({endpoint})… waiting for tunnel up.",
                    config.name
                ));
                let _ = self.publish_snapshot();

                match crate::ovpn::wait_for_openvpn_up(
                    &log_path,
                    Duration::from_secs(60),
                )
                .await
                {
                    Ok(crate::ovpn::OvpnTunnelState::Up) => {
                        if let Some(active) = self.active_connection.as_mut() {
                            active.phase = ConnectionPhase::Connected;
                            active.connected_at_unix = Some(unix_now());
                        }
                        self.notice = Some(format!(
                            "OpenVPN system tunnel ACTIVE for '{}' ({endpoint}). Refreshing Your IP…",
                            config.name
                        ));
                        let _ = self.publish_snapshot();
                        // Let routes settle, then reverse-lookup public IP so
                        // Your IP + globe show the VPN exit.
                        let _ = self.refresh_public_ip_after_tunnel().await;
                    }
                    Ok(crate::ovpn::OvpnTunnelState::AuthFailed) => {
                        if let Some(active) = self.active_connection.as_mut() {
                            active.phase = ConnectionPhase::Error;
                        }
                        crate::ovpn::kill_openvpn_processes();
                        let _ = std::fs::remove_file(&auth_path);
                        self.notice = Some(format!(
                            "OpenVPN authentication failed for '{}'. Check username/password.",
                            config.name
                        ));
                    }
                    Ok(crate::ovpn::OvpnTunnelState::Fatal(detail)) => {
                        if let Some(active) = self.active_connection.as_mut() {
                            active.phase = ConnectionPhase::Error;
                        }
                        crate::ovpn::kill_openvpn_processes();
                        self.notice = Some(format!(
                            "OpenVPN failed for '{}': {detail}",
                            config.name
                        ));
                    }
                    Ok(crate::ovpn::OvpnTunnelState::Exited)
                    | Ok(crate::ovpn::OvpnTunnelState::Connecting) => {
                        if let Some(active) = self.active_connection.as_mut() {
                            active.phase = ConnectionPhase::Error;
                        }
                        crate::ovpn::kill_openvpn_processes();
                        self.notice = Some(format!(
                            "OpenVPN exited before the tunnel was ready for '{}'. See {}",
                            config.name,
                            log_path.display()
                        ));
                    }
                    Err(error) => {
                        if let Some(active) = self.active_connection.as_mut() {
                            active.phase = ConnectionPhase::Error;
                        }
                        crate::ovpn::kill_openvpn_processes();
                        self.notice = Some(format!(
                            "OpenVPN did not establish a system tunnel: {error:#}"
                        ));
                    }
                }
            }
            Err(error) => {
                if let Some(active) = self.active_connection.as_mut() {
                    active.phase = ConnectionPhase::Error;
                }
                self.notice = Some(format!(
                    "Failed to start OpenVPN ({}): {error:#}",
                    openvpn_exe.display()
                ));
            }
        }

        self.persist_client_state()?;
        self.publish_snapshot()
    }

    /// Reject connect if another non-Tor full tunnel is already up.
    fn reject_if_other_tunnel_busy(&mut self, want_prefix: &str) -> Option<Result<()>> {
        if let Some(active) = self.active_connection.as_ref() {
            let busy = matches!(
                active.phase,
                ConnectionPhase::Connecting | ConnectionPhase::Connected
            );
            if busy
                && active.server_id != "tor_local"
                && !active.server_id.starts_with(want_prefix)
            {
                let name = active.protocol.display_name();
                self.notice = Some(format!(
                    "{name} is already connected. Disconnect it before switching protocols."
                ));
                return Some(self.publish_snapshot());
            }
        }
        None
    }

    async fn enrich_wg_profile(&mut self, id: i64) -> Result<()> {
        let Some(config) = crate::db::get_wg_config(id)? else {
            return self.publish_snapshot();
        };
        let host = config
            .endpoint
            .as_ref()
            .map(|e| crate::protocols::host_from_endpoint(e))
            .unwrap_or_default();
        if host.is_empty() {
            let _ = self.event_tx.send(ClientEvent::ReloadProtocolProfiles);
            return self.publish_snapshot();
        }
        if let Some(info) = crate::ovpn::enrich_remote_location(&host).await {
            let resolved = if info.ip.is_empty() {
                None
            } else {
                Some(info.ip.as_str())
            };
            let _ = crate::db::update_wg_location(id, &info, resolved);
            self.notice = Some(format!(
                "WireGuard '{}': {} · {} {}",
                config.name,
                if info.ip.is_empty() {
                    host.as_str()
                } else {
                    info.ip.as_str()
                },
                info.country,
                info.city
            ));
        }
        let _ = self.event_tx.send(ClientEvent::ReloadProtocolProfiles);
        self.publish_snapshot()
    }

    async fn connect_to_wireguard(&mut self, id: i64) -> Result<()> {
        if let Some(r) = self.reject_if_other_tunnel_busy("wg_") {
            return r;
        }
        let config = match crate::db::get_wg_config(id)? {
            Some(c) => c,
            None => {
                self.notice = Some(String::from("WireGuard profile not found."));
                return self.publish_snapshot();
            }
        };
        crate::db::set_selected_wg_id(Some(id));
        std::fs::create_dir_all(&self.paths.profiles_dir)?;
        let profile_path = self
            .paths
            .profiles_dir
            .join(format!("wg_{id}.conf"));
        std::fs::write(&profile_path, &config.content)?;

        let endpoint = config
            .endpoint
            .clone()
            .unwrap_or_else(|| String::from("wireguard"));
        let country = config.country_code.clone();

        self.active_connection = Some(ActiveConnection {
            server_id: format!("wg_{id}"),
            server_name: config.name.clone(),
            endpoint: endpoint.clone(),
            protocol: VpnProtocol::WireGuard,
            phase: ConnectionPhase::Connecting,
            connected_at_unix: None,
            attempt_count: 1,
            session_id: None,
            reserved_client_ip: config.address.clone(),
            server_internal_ip: None,
            tunnel_profile_path: Some(profile_path.display().to_string()),
            country_code: country.clone(),
            tor_exit_info: None,
            cooldown_until_unix: None,
            last_status_unix: Some(unix_now()),
        });
        self.notice = Some(format!(
            "Starting WireGuard for '{}' ({endpoint})…",
            config.name
        ));
        self.set_op_progress("connect", 0.08, "Parsing WireGuard profile…");

        #[cfg(target_os = "windows")]
        {
            if !platform::is_elevated() {
                match platform::relaunch_elevated_with_args(&[
                    &format!("--auto-connect-wg={id}"),
                ]) {
                    Ok(()) => {
                        self.set_op_progress("connect", 0.15, "Requesting Administrator…");
                        self.notice = Some(String::from(
                            "Accepted UAC — restarting elevated for WireGuard…",
                        ));
                        let _ = self.publish_snapshot();
                        platform::exit_after_relaunch();
                    }
                    Err(error) => {
                        if let Some(active) = self.active_connection.as_mut() {
                            active.phase = ConnectionPhase::Error;
                        }
                        self.clear_op_progress();
                        self.notice = Some(format!(
                            "WireGuard needs Administrator ({error:#}). Accept UAC or Run as administrator."
                        ));
                        return self.publish_snapshot();
                    }
                }
            }

            // Prefer embedded boringtun + Wintun (no wireguard.exe).
            self.set_op_progress("connect", 0.22, "Preparing Wintun adapter…");
            let _ = platform::stop_wireguard_global();
            match platform::parse_wireguard_config(&config.content) {
                Ok(tc) => {
                    tracing::info!(
                        "WireGuard parsed: endpoint={} tunnel_ip={} allowed={:?}",
                        tc.server_endpoint,
                        tc.tunnel_ip,
                        tc.allowed_ips
                    );
                    self.set_op_progress("connect", 0.4, "Starting tunnel + routes…");
                    tokio::task::yield_now().await;
                    match platform::start_wireguard_global(tc) {
                        Ok(()) => {
                            self.set_op_progress("connect", 0.65, "Handshake in progress…");
                            // Brief settle then confirm the pump is still up.
                            tokio::time::sleep(Duration::from_millis(400)).await;
                            if !platform::is_wireguard_running() {
                                if let Some(active) = self.active_connection.as_mut() {
                                    active.phase = ConnectionPhase::Error;
                                }
                                self.clear_op_progress();
                                self.notice = Some(String::from(
                                    "WireGuard tunnel exited immediately. Run as Administrator and ensure wintun.dll is next to vpn-client.exe.",
                                ));
                            } else {
                                if let Some(active) = self.active_connection.as_mut() {
                                    active.phase = ConnectionPhase::Connected;
                                    active.connected_at_unix = Some(unix_now());
                                }
                                self.notice = Some(format!(
                                    "WireGuard ACTIVE for '{}' ({endpoint}). Refreshing Your IP…",
                                    config.name
                                ));
                                self.set_op_progress("connect", 0.8, "Refreshing Your IP…");
                                let _ = self.publish_snapshot();
                                let _ = self.refresh_public_ip_after_tunnel().await;
                                self.set_op_progress("connect", 1.0, "Connected");
                                tokio::time::sleep(Duration::from_millis(350)).await;
                                self.clear_op_progress();
                            }
                        }
                        Err(error) => {
                            if let Some(active) = self.active_connection.as_mut() {
                                active.phase = ConnectionPhase::Error;
                            }
                            self.clear_op_progress();
                            self.notice = Some(format!(
                                "WireGuard failed: {error:#}. Need Administrator + wintun.dll beside the app."
                            ));
                        }
                    }
                }
                Err(error) => {
                    if let Some(active) = self.active_connection.as_mut() {
                        active.phase = ConnectionPhase::Error;
                    }
                    self.clear_op_progress();
                    self.notice = Some(format!("Invalid WireGuard config: {error:#}"));
                }
            }
        }

        #[cfg(target_os = "linux")]
        {
            if !platform::is_elevated() {
                match platform::relaunch_elevated_with_args(&[
                    &format!("--auto-connect-wg={id}"),
                ]) {
                    Ok(()) => {
                        self.set_op_progress("connect", 0.15, "Requesting Administrator…");
                        self.notice = Some(String::from(
                            "Accepted — restarting elevated for WireGuard (pkexec)…",
                        ));
                        let _ = self.publish_snapshot();
                        platform::exit_after_relaunch();
                    }
                    Err(error) => {
                        if let Some(active) = self.active_connection.as_mut() {
                            active.phase = ConnectionPhase::Error;
                        }
                        self.clear_op_progress();
                        self.notice = Some(format!(
                            "WireGuard needs Administrator ({error:#}). Authenticate via pkexec or run with sudo."
                        ));
                        return self.publish_snapshot();
                    }
                }
            }

            self.set_op_progress("connect", 0.22, "Preparing TUN adapter…");
            let _ = platform::stop_wireguard_global();
            match platform::parse_wireguard_config(&config.content) {
                Ok(tc) => {
                    tracing::info!(
                        "WireGuard parsed: endpoint={} tunnel_ip={} allowed={:?}",
                        tc.server_endpoint,
                        tc.tunnel_ip,
                        tc.allowed_ips
                    );
                    self.set_op_progress("connect", 0.4, "Starting tunnel + routes…");
                    tokio::task::yield_now().await;
                    match platform::start_wireguard_global(tc) {
                        Ok(()) => {
                            self.set_op_progress("connect", 0.65, "Handshake in progress…");
                            tokio::time::sleep(Duration::from_millis(400)).await;
                            if !platform::is_wireguard_running() {
                                if let Some(active) = self.active_connection.as_mut() {
                                    active.phase = ConnectionPhase::Error;
                                }
                                self.clear_op_progress();
                                self.notice = Some(String::from(
                                    "WireGuard tunnel exited immediately. Run as Administrator (pkexec) and ensure TUN is available.",
                                ));
                            } else {
                                if let Some(active) = self.active_connection.as_mut() {
                                    active.phase = ConnectionPhase::Connected;
                                    active.connected_at_unix = Some(unix_now());
                                }
                                self.notice = Some(format!(
                                    "WireGuard ACTIVE for '{}' ({endpoint}). Refreshing Your IP…",
                                    config.name
                                ));
                                self.set_op_progress("connect", 0.8, "Refreshing Your IP…");
                                let _ = self.publish_snapshot();
                                let _ = self.refresh_public_ip_after_tunnel().await;
                                self.set_op_progress("connect", 1.0, "Connected");
                                tokio::time::sleep(Duration::from_millis(350)).await;
                                self.clear_op_progress();
                            }
                        }
                        Err(error) => {
                            if let Some(active) = self.active_connection.as_mut() {
                                active.phase = ConnectionPhase::Error;
                            }
                            self.clear_op_progress();
                            self.notice = Some(format!(
                                "WireGuard failed: {error:#}. Need Administrator + TUN (pkexec)."
                            ));
                        }
                    }
                }
                Err(error) => {
                    if let Some(active) = self.active_connection.as_mut() {
                        active.phase = ConnectionPhase::Error;
                    }
                    self.clear_op_progress();
                    self.notice = Some(format!("Invalid WireGuard config: {error:#}"));
                }
            }
        }
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            // Imported .conf full-tunnel apply for Linux will use userspace
            // boringtun or `wg-quick` in a follow-up; profile is already written.
            if let Some(active) = self.active_connection.as_mut() {
                active.phase = ConnectionPhase::Error;
            }
            self.notice = Some(format!(
                "WireGuard profile saved to {}. Full-tunnel apply for imported .conf is Windows-first; \
                 use ZeroNode control-plane WireGuard on Linux for now.",
                profile_path.display()
            ));
        }

        self.persist_client_state()?;
        self.publish_snapshot()
    }

    async fn connect_to_pptp(&mut self, id: i64) -> Result<()> {
        if let Some(r) = self.reject_if_other_tunnel_busy("pptp_") {
            return r;
        }
        let config = match crate::db::get_pptp_config(id)? {
            Some(c) => c,
            None => {
                self.notice = Some(String::from("PPTP profile not found."));
                return self.publish_snapshot();
            }
        };
        let ep = config.to_endpoint();
        if let Err(e) = ep.validate() {
            self.notice = Some(format!("PPTP profile invalid: {e:#}"));
            return self.publish_snapshot();
        }

        self.active_connection = Some(ActiveConnection {
            server_id: format!("pptp_{id}"),
            server_name: config.name.clone(),
            endpoint: ep.endpoint_label(),
            protocol: VpnProtocol::Pptp,
            phase: ConnectionPhase::Connecting,
            connected_at_unix: None,
            attempt_count: 1,
            session_id: None,
            reserved_client_ip: None,
            server_internal_ip: None,
            tunnel_profile_path: None,
            country_code: config.country_code.clone(),
            tor_exit_info: None,
            cooldown_until_unix: None,
            last_status_unix: Some(unix_now()),
        });
        self.notice = Some(format!("Dialing PPTP '{}'…", config.name));
        let _ = self.publish_snapshot();

        #[cfg(target_os = "windows")]
        {
            let user = ep.user_for_dial();
            match platform::start_pptp(&ep.dial_target(), &user, &ep.password) {
                Ok(()) => {
                    if let Some(active) = self.active_connection.as_mut() {
                        active.phase = ConnectionPhase::Connected;
                        active.connected_at_unix = Some(unix_now());
                    }
                    self.notice = Some(format!(
                        "PPTP connected to '{}' (legacy). Refreshing Your IP…",
                        config.name
                    ));
                    let _ = self.publish_snapshot();
                    let _ = self.refresh_public_ip_after_tunnel().await;
                }
                Err(error) => {
                    if let Some(active) = self.active_connection.as_mut() {
                        active.phase = ConnectionPhase::Error;
                    }
                    self.notice = Some(format!("PPTP dial failed: {error:#}"));
                }
            }
        }
        #[cfg(target_os = "linux")]
        {
            match platform::start_pptp(&ep.dial_target(), &ep.user_for_dial(), &ep.password) {
                Ok(()) => {
                    if let Some(active) = self.active_connection.as_mut() {
                        active.phase = ConnectionPhase::Connected;
                        active.connected_at_unix = Some(unix_now());
                    }
                    self.notice = Some(format!(
                        "PPTP connected to '{}' (legacy). Refreshing Your IP…",
                        config.name
                    ));
                    let _ = self.publish_snapshot();
                    let _ = self.refresh_public_ip_after_tunnel().await;
                }
                Err(error) => {
                    if let Some(active) = self.active_connection.as_mut() {
                        active.phase = ConnectionPhase::Error;
                    }
                    self.notice = Some(format!("PPTP dial failed: {error:#}"));
                }
            }
        }
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            if let Some(active) = self.active_connection.as_mut() {
                active.phase = ConnectionPhase::Error;
            }
            self.notice = Some(String::from(
                "PPTP is currently supported on Windows only (RAS).",
            ));
        }

        self.persist_client_state()?;
        self.publish_snapshot()
    }

    async fn enrich_outline_profile(&mut self, id: i64) -> Result<()> {
        let Some(config) = crate::db::get_outline_config(id)? else {
            return self.publish_snapshot();
        };
        let host = config.host.clone().unwrap_or_default();
        if host.is_empty() {
            let _ = self.event_tx.send(ClientEvent::ReloadProtocolProfiles);
            return self.publish_snapshot();
        }
        if let Some(info) = crate::ovpn::enrich_remote_location(&host).await {
            let _ = crate::db::update_outline_location(id, &info);
            self.notice = Some(format!(
                "Outline '{}': {} · {} {}",
                config.name,
                if info.ip.is_empty() {
                    host.as_str()
                } else {
                    info.ip.as_str()
                },
                info.country,
                info.city
            ));
        }
        let _ = self.event_tx.send(ClientEvent::ReloadProtocolProfiles);
        self.publish_snapshot()
    }

    async fn connect_to_outline(&mut self, id: i64, system_wide: bool) -> Result<()> {
        if let Some(r) = self.reject_if_other_tunnel_busy("outline_") {
            return r;
        }
        self.set_op_progress("connect", 0.05, "Parsing access key…");
        let config = match crate::db::get_outline_config(id)? {
            Some(c) => c,
            None => {
                self.clear_op_progress();
                self.notice = Some(String::from("Outline profile not found."));
                return self.publish_snapshot();
            }
        };
        let ep = match vpn_suite_core::outline::parse_outline_input(&config.access_key) {
            Ok(e) => e,
            Err(error) => {
                self.clear_op_progress();
                self.notice = Some(format!("Invalid Outline key: {error:#}"));
                return self.publish_snapshot();
            }
        };

        self.active_connection = Some(ActiveConnection {
            server_id: format!("outline_{id}"),
            server_name: config.name.clone(),
            endpoint: ep.endpoint_label(),
            protocol: VpnProtocol::Outline,
            phase: ConnectionPhase::Connecting,
            connected_at_unix: None,
            attempt_count: 1,
            session_id: None,
            reserved_client_ip: None,
            server_internal_ip: None,
            tunnel_profile_path: None,
            country_code: config.country_code.clone(),
            tor_exit_info: None,
            cooldown_until_unix: None,
            last_status_unix: Some(unix_now()),
        });
        self.notice = Some(format!(
            "Starting Outline '{}' ({})…",
            config.name,
            ep.endpoint_label()
        ));
        self.set_op_progress("connect", 0.12, "Preparing Shadowsocks…");

        #[cfg(target_os = "windows")]
        {
            if system_wide && !platform::is_elevated() {
                match platform::relaunch_elevated_with_args(&[
                    &format!("--auto-connect-outline={id}"),
                ]) {
                    Ok(()) => {
                        self.set_op_progress("connect", 0.2, "Requesting Administrator…");
                        self.notice = Some(String::from(
                            "Accepted UAC — restarting elevated for Outline…",
                        ));
                        let _ = self.publish_snapshot();
                        platform::exit_after_relaunch();
                    }
                    Err(error) => {
                        if let Some(active) = self.active_connection.as_mut() {
                            active.phase = ConnectionPhase::Error;
                        }
                        self.clear_op_progress();
                        self.notice = Some(format!(
                            "Outline system tunnel needs Administrator ({error:#})."
                        ));
                        return self.publish_snapshot();
                    }
                }
            }

            self.set_op_progress(
                "connect",
                0.28,
                "Starting embedded Shadowsocks (SOCKS)…",
            );
            // Yield so the UI paints the progress stage before the blocking start.
            tokio::task::yield_now().await;
            match platform::start_outline(
                &ep.method,
                &ep.password,
                &ep.host,
                ep.port,
                system_wide,
            ) {
                Ok(socks_port) => {
                    self.set_op_progress(
                        "connect",
                        if system_wide { 0.72 } else { 0.85 },
                        if system_wide {
                            "System TUN up — verifying routes…"
                        } else {
                            "SOCKS ready…"
                        },
                    );
                    if let Some(active) = self.active_connection.as_mut() {
                        active.phase = ConnectionPhase::Connected;
                        active.connected_at_unix = Some(unix_now());
                    }
                    self.notice = Some(format!(
                        "Outline ACTIVE for '{}' (SOCKS 127.0.0.1:{socks_port}{}). Refreshing Your IP…",
                        config.name,
                        if system_wide { " + system TUN" } else { "" }
                    ));
                    let _ = self.publish_snapshot();
                    if system_wide {
                        self.set_op_progress("connect", 0.85, "Refreshing public IP…");
                        let _ = self.refresh_public_ip_after_tunnel().await;
                    }
                    self.set_op_progress("connect", 1.0, "Connected");
                    // Brief hold at 100% then clear so the bar doesn't stick forever.
                    tokio::time::sleep(Duration::from_millis(400)).await;
                    self.clear_op_progress();
                }
                Err(error) => {
                    if let Some(active) = self.active_connection.as_mut() {
                        active.phase = ConnectionPhase::Error;
                    }
                    self.clear_op_progress();
                    self.notice = Some(format!("Outline failed: {error:#}"));
                }
            }
        }
        #[cfg(target_os = "linux")]
        {
            if system_wide && !platform::is_elevated() {
                match platform::relaunch_elevated_with_args(&[
                    &format!("--auto-connect-outline={id}"),
                ]) {
                    Ok(()) => {
                        self.set_op_progress("connect", 0.2, "Requesting Administrator…");
                        self.notice = Some(String::from(
                            "Accepted — restarting elevated for Outline (pkexec)…",
                        ));
                        let _ = self.publish_snapshot();
                        platform::exit_after_relaunch();
                    }
                    Err(error) => {
                        if let Some(active) = self.active_connection.as_mut() {
                            active.phase = ConnectionPhase::Error;
                        }
                        self.clear_op_progress();
                        self.notice = Some(format!(
                            "Outline system tunnel needs Administrator ({error:#})."
                        ));
                        return self.publish_snapshot();
                    }
                }
            }

            self.set_op_progress(
                "connect",
                0.28,
                "Starting embedded Shadowsocks (SOCKS)…",
            );
            tokio::task::yield_now().await;
            match platform::start_outline(
                &ep.method,
                &ep.password,
                &ep.host,
                ep.port,
                system_wide,
            ) {
                Ok(socks_port) => {
                    self.set_op_progress(
                        "connect",
                        if system_wide { 0.72 } else { 0.85 },
                        if system_wide {
                            "System TUN up — verifying routes…"
                        } else {
                            "SOCKS ready…"
                        },
                    );
                    if let Some(active) = self.active_connection.as_mut() {
                        active.phase = ConnectionPhase::Connected;
                        active.connected_at_unix = Some(unix_now());
                    }
                    self.notice = Some(format!(
                        "Outline ACTIVE for '{}' (SOCKS 127.0.0.1:{socks_port}{}). Refreshing Your IP…",
                        config.name,
                        if system_wide { " + system TUN" } else { "" }
                    ));
                    let _ = self.publish_snapshot();
                    if system_wide {
                        self.set_op_progress("connect", 0.85, "Refreshing public IP…");
                        let _ = self.refresh_public_ip_after_tunnel().await;
                    }
                    self.set_op_progress("connect", 1.0, "Connected");
                    tokio::time::sleep(Duration::from_millis(400)).await;
                    self.clear_op_progress();
                }
                Err(error) => {
                    if let Some(active) = self.active_connection.as_mut() {
                        active.phase = ConnectionPhase::Error;
                    }
                    self.clear_op_progress();
                    self.notice = Some(format!("Outline failed: {error:#}"));
                }
            }
        }
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            let _ = system_wide;
            if let Some(active) = self.active_connection.as_mut() {
                active.phase = ConnectionPhase::Error;
            }
            self.clear_op_progress();
            self.notice = Some(String::from(
                "Outline system tunnel is currently implemented on Windows (sslocal + TUN).",
            ));
        }

        self.persist_client_state()?;
        self.publish_snapshot()
    }

    async fn launch_tor_isolated_app(&mut self, id: i64) -> Result<()> {
        let apps = crate::db::list_tor_isolated_apps().unwrap_or_default();
        let Some(app) = apps.into_iter().find(|a| a.id == id) else {
            self.notice = Some(String::from("Isolated app not found."));
            return self.publish_snapshot();
        };
        let port = match self.tor_socks_port {
            Some(p) => p,
            None => {
                self.notice = Some(String::from(
                    "Tor SOCKS5 is not up. Connect Tor first, then launch the app.",
                ));
                return self.publish_snapshot();
            }
        };
        let proxy = format!("socks5://127.0.0.1:{port}");
        let socks_url = format!("socks5h://127.0.0.1:{port}");
        let path = std::path::PathBuf::from(&app.path);
        if !path.is_file() {
            self.notice = Some(format!("App path missing: {}", app.path));
            return self.publish_snapshot();
        }

        let mut cmd = std::process::Command::new(&path);
        cmd.env("ALL_PROXY", &socks_url);
        cmd.env("all_proxy", &socks_url);
        cmd.env("HTTP_PROXY", &proxy);
        cmd.env("HTTPS_PROXY", &proxy);
        cmd.env("http_proxy", &proxy);
        cmd.env("https_proxy", &proxy);
        // Chromium-family browsers honor --proxy-server.
        let lower = app.path.to_ascii_lowercase();
        if lower.contains("chrome")
            || lower.contains("msedge")
            || lower.contains("brave")
            || lower.contains("chromium")
            || lower.contains("opera")
            || lower.contains("vivaldi")
        {
            cmd.arg(format!("--proxy-server=socks5://127.0.0.1:{port}"));
        }
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x00000010); // CREATE_NEW_CONSOLE so GUI apps work
        }
        match cmd.spawn() {
            Ok(_) => {
                self.notice = Some(format!(
                    "Launched '{}' through Tor SOCKS5 127.0.0.1:{port}.",
                    app.name
                ));
            }
            Err(error) => {
                self.notice = Some(format!("Could not launch {}: {error}", app.name));
            }
        }
        self.publish_snapshot()
    }

    async fn connect_to_tor(&mut self) -> Result<()> {
        // System-wide Tor VPN needs Administrator (Wintun + route add).
        // App-isolation mode only needs the local SOCKS5 — skip UAC.
        // If we are not elevated, offer UAC once; on Accept we relaunch with
        // `--auto-connect-tor` and exit this process. On Cancel we still
        // bring up SOCKS5-only so GeoIP/animation work (no silent kill).
        let apps_only = crate::db::get_tor_isolation_mode() == "apps";
        #[cfg(target_os = "windows")]
        {
            if !apps_only && !platform::is_elevated() {
                match platform::relaunch_elevated_with_args(&["--auto-connect-tor"]) {
                    Ok(()) => {
                        self.notice = Some(String::from(
                            "Accepted UAC — restarting elevated for system-wide Tor VPN…",
                        ));
                        let _ = self.publish_snapshot();
                        platform::exit_after_relaunch();
                    }
                    Err(error) => {
                        tracing::warn!(
                            "UAC elevation declined/failed for Tor system VPN: {error:#}; continuing SOCKS5-only"
                        );
                        self.notice = Some(format!(
                            "System-wide Tor VPN needs Administrator ({error:#}). Starting SOCKS5-only — enable routing after running as admin."
                        ));
                        let _ = self.publish_snapshot();
                    }
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = apps_only;
        }

        // Linux: use distro-aware resolver (bundled expert bundle 15.0.17 with fallback to system tor).
        // Windows: legacy tor.exe bundle.
        #[cfg(target_os = "linux")]
        let tor_exe = match platform::resolve_tor_binary() {
            Some(p) => p,
            None => {
                self.notice = Some(String::from(
                    "Tor not found. Install tor (sudo apt install tor / sudo pacman -S tor / sudo dnf install tor) or run tools/fetch-tor-linux.sh and rebuild.",
                ));
                return self.publish_snapshot();
            }
        };
        #[cfg(not(target_os = "linux"))]
        let tor_exe = {
            let exe_path = std::env::current_exe().unwrap_or_default();
            let exe_dir = exe_path.parent().unwrap_or(std::path::Path::new("")).to_path_buf();
            let mut tor_exe = exe_dir.join("assets/tor/tor.exe");
            if !tor_exe.exists() {
                tor_exe = std::env::current_dir().unwrap_or_default().join("apps/client/assets/tor/tor.exe");
            }
            if !tor_exe.exists() {
                tor_exe = exe_dir.join("../../apps/client/assets/tor/tor.exe");
            }
            if !tor_exe.exists() {
                self.notice = Some(format!("Tor executable not found in bundle. Looked for {:?}", tor_exe));
                return self.publish_snapshot();
            }
            tor_exe
        };

        // Find a free port for Tor SOCKS5
        let socks_port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0");
            match listener {
                Ok(l) => {
                    let port = l.local_addr().map(|a| a.port()).unwrap_or(9050);
                    drop(l);
                    port
                }
                Err(_) => 9050,
            }
        };

        // Remember the port so a later `ApplyTorSystemRoute` command can
        // attach tun2proxy to the right SOCKS5 listener.
        self.tor_socks_port = Some(socks_port);

        self.active_connection = Some(ActiveConnection {
            server_id: "tor_local".into(),
            server_name: "Tor (Connecting...)".into(),
            endpoint: format!("127.0.0.1:{}", socks_port),
            protocol: vpn_suite_core::model::VpnProtocol::WireGuard, // Hack for UI
            phase: ConnectionPhase::Connecting,
            connected_at_unix: Some(unix_now()),
            attempt_count: 0,
            session_id: None,
            reserved_client_ip: None,
            server_internal_ip: None,
            tunnel_profile_path: None,
            cooldown_until_unix: None,
            tor_exit_info: None,
            country_code: None,
            last_status_unix: None,
        });

        let tor_dir = tor_exe.parent().unwrap_or(std::path::Path::new("")).to_path_buf();

        // Kill any existing tor processes first (fully silent — no console flash).
        #[cfg(target_os = "windows")]
        {
            silent_windows_kill_image("tor.exe");
        }
        #[cfg(target_os = "linux")]
        {
            let _ = platform::kill_process_by_name("tor");
        }
        // Small sleep to ensure port is released
        std::thread::sleep(std::time::Duration::from_millis(500));

        // Generate a writable DataDirectory and torrc to prevent tor from crashing.
        let tor_data_dir = std::env::temp_dir().join(format!("vpn_suite_tor_data_{}", unix_now()));
        let _ = std::fs::create_dir_all(&tor_data_dir);
        let torrc_path = tor_data_dir.join("torrc");
        // Linux: bundled geoip at tor_dir/geoip (/usr/share/vpn-client/tor-linux/geoip),
        // system tor geoip at /usr/share/tor/geoip, dev fallback.
        let geoip_path = {
            #[cfg(target_os = "linux")]
            {
                let candidates = [
                    tor_dir.join("geoip"),
                    std::path::PathBuf::from("/usr/share/tor/geoip"),
                    std::path::PathBuf::from("/usr/share/vpn-client/tor-linux/geoip"),
                    std::path::PathBuf::from("apps/client/assets/tor-linux/geoip"),
                ];
                candidates
                    .into_iter()
                    .find(|p| p.is_file())
                    .unwrap_or_else(|| tor_dir.join("geoip"))
                    .display()
                    .to_string()
                    .replace('\\', "/")
            }
            #[cfg(not(target_os = "linux"))]
            {
                tor_dir.join("geoip").display().to_string().replace('\\', "/")
            }
        };
        let geoip6_path = {
            #[cfg(target_os = "linux")]
            {
                let candidates = [
                    tor_dir.join("geoip6"),
                    std::path::PathBuf::from("/usr/share/tor/geoip6"),
                    std::path::PathBuf::from("/usr/share/vpn-client/tor-linux/geoip6"),
                    std::path::PathBuf::from("apps/client/assets/tor-linux/geoip6"),
                ];
                candidates
                    .into_iter()
                    .find(|p| p.is_file())
                    .unwrap_or_else(|| tor_dir.join("geoip6"))
                    .display()
                    .to_string()
                    .replace('\\', "/")
            }
            #[cfg(not(target_os = "linux"))]
            {
                tor_dir.join("geoip6").display().to_string().replace('\\', "/")
            }
        };
        // SocksPort: fixed auth is used by our GeoIP client so IsolateSOCKSAuth
        // reuses one circuit family. NoIsolateDest* reduces exit hopping between
        // different probe hosts (still real Tor privacy; just less confusing IPs).
        let torrc_content = format!(
            "DataDirectory {}\n\
             SocksPort 127.0.0.1:{} NoIsolateDestAddr NoIsolateDestPort\n\
             GeoIPFile {}\n\
             GeoIPv6File {}\n\
             AvoidDiskWrites 1\n\
             Log notice file {}\n",
            tor_data_dir.display().to_string().replace('\\', "/"),
            socks_port,
            geoip_path,
            geoip6_path,
            tor_data_dir
                .join("notice.log")
                .display()
                .to_string()
                .replace('\\', "/"),
        );
        let _ = std::fs::write(&torrc_path, torrc_content);

        // Spawn Tor and verify it started
        let tor_exe_clone = tor_exe.clone();
        let torrc_path_clone = torrc_path.clone();
        let tor_dir_clone = tor_dir.clone();
        let tx = self.command_tx.clone();
        let mut geoip = self.geoip.clone();
        // Always re-open mmdb from disk in the worker if the in-memory stack
        // was still None when Connect was clicked (download race).
        let geo_dir = self.paths.base_dir.join("geoip");

        std::thread::spawn(move || {
            let child = {
                let mut cmd = std::process::Command::new(&tor_exe_clone);
                cmd.args(["-f", torrc_path_clone.to_str().unwrap_or("")])
                    .current_dir(&tor_dir_clone)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null());
                #[cfg(target_os = "windows")]
                {
                    use std::os::windows::process::CommandExt;
                    // CREATE_NO_WINDOW — tor.exe is a console subsystem binary.
                    cmd.creation_flags(0x08000000);
                }
                #[cfg(target_os = "linux")]
                {
                    // If the GUI is hard-killed (SIGKILL / task manager End
                    // Task), the child tor would otherwise orphan and keep its
                    // circuits + SocksPort open, leaking the VPN and blocking
                    // the next bootstrap at 18%. PDEATHSIG makes the kernel
                    // deliver SIGTERM to tor automatically when this parent dies.
                    use std::os::unix::process::CommandExt;
                    unsafe {
                        cmd.pre_exec(|| {
                            // SAFETY: prctl is async-signal-safe; we are in the
                            // child after fork but before exec.
                            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                            Ok(())
                        });
                    }
                }
                cmd.spawn()
            };

            match child {
                Ok(mut process) => {
                    // Give tor a few seconds to bind SOCKS, then confirm it's alive.
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    match process.try_wait() {
                        Ok(Some(_status)) => {
                            let _ = tx.send(ClientCommand::Disconnect);
                            return;
                        }
                        Ok(None) => {}
                        Err(_) => {
                            let _ = tx.send(ClientCommand::Disconnect);
                            return;
                        }
                    }

                    let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
                        let _ = tx.send(ClientCommand::Disconnect);
                        return;
                    };
                    rt.block_on(async move {
                        // Late-bind local GeoIP DBs so enrichment works even if
                        // the download finished after the GUI started.
                        if geoip.is_none() {
                            geoip = crate::tor_geo::open_geoip_stack(&geo_dir).await;
                            if let Some(stack) = geoip.clone() {
                                let _ = tx.send(ClientCommand::GeoIpReady(stack));
                            }
                        }

                        let Some(client) = crate::tor_geo::tor_proxy_client(socks_port) else {
                            let _ = tx.send(ClientCommand::Disconnect);
                            return;
                        };

                        // Tor bootstrap can take 60–120s+. Poll up to ~6 minutes.
                        let mut geoip_resolved = false;
                        for attempt in 0..180 {
                            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                            if let Ok(Some(status)) = process.try_wait() {
                                tracing::warn!(
                                    "tor.exe exited prematurely (status={status}); aborting GeoIP poll"
                                );
                                let _ = tx.send(ClientCommand::Disconnect);
                                return;
                            }
                            // Retry opening mmdb periodically if still missing.
                            if geoip.is_none() && attempt % 10 == 0 {
                                geoip = crate::tor_geo::try_open_geoip_stack(&geo_dir);
                            }
                            match crate::tor_geo::resolve_tor_exit(&client, geoip.as_deref()).await {
                                Some(exit_info) => {
                                    let has_country = !exit_info.country_code.is_empty();
                                    tracing::info!(
                                        "Tor GeoIP resolved after {} attempts: {} ({})",
                                        attempt + 1,
                                        exit_info.ip,
                                        exit_info.country_code
                                    );
                                    let _ = tx.send(ClientCommand::TorConnected {
                                        ip: exit_info.ip.clone(),
                                        country: if has_country {
                                            exit_info.country_code.clone()
                                        } else {
                                            String::from("TOR")
                                        },
                                        socks_port,
                                        exit_info: Some(exit_info),
                                    });
                                    // If we only got a partial IP (no country yet),
                                    // keep polling a bit longer for full details.
                                    if has_country {
                                        geoip_resolved = true;
                                        break;
                                    }
                                    // Partial success: still mark resolved so we don't
                                    // fall back to Unknown, but allow a few more tries
                                    // for richer fields by continuing briefly.
                                    geoip_resolved = true;
                                    // Keep improving details for up to ~40s more.
                                    for refine in 0..20 {
                                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                                        if let Ok(Some(_)) = process.try_wait() {
                                            break;
                                        }
                                        if geoip.is_none() {
                                            geoip = crate::tor_geo::try_open_geoip_stack(&geo_dir);
                                        }
                                        if let Some(better) =
                                            crate::tor_geo::resolve_tor_exit(&client, geoip.as_deref())
                                                .await
                                        {
                                            if !better.country_code.is_empty() {
                                                tracing::info!(
                                                    "Tor GeoIP refined after partial (try {}): {} ({})",
                                                    refine + 1,
                                                    better.ip,
                                                    better.country_code
                                                );
                                                let _ = tx.send(ClientCommand::TorConnected {
                                                    ip: better.ip.clone(),
                                                    country: better.country_code.clone(),
                                                    socks_port,
                                                    exit_info: Some(better),
                                                });
                                                break;
                                            }
                                        }
                                    }
                                    break;
                                }
                                None => {
                                    if attempt == 0 || attempt % 15 == 0 {
                                        tracing::info!(
                                            "still waiting for Tor GeoIP (attempt {}/180)",
                                            attempt + 1
                                        );
                                    }
                                }
                            }
                        }
                        if !geoip_resolved {
                            tracing::warn!(
                                "Tor GeoIP never resolved after 180 attempts; UI will show 'Unknown'"
                            );
                            let _ = tx.send(ClientCommand::TorConnected {
                                ip: "Unknown IP".to_string(),
                                country: "TOR".to_string(),
                                socks_port,
                                exit_info: None,
                            });
                        }
                    });
                }
                Err(_e) => {
                    let _ = tx.send(ClientCommand::Disconnect);
                }
            }
        });

        self.persist_client_state()?;
        self.notice = Some(format!(
            "Starting Tor (SOCKS5 on 127.0.0.1:{socks_port})… bootstrapping circuit, then system-wide tunnel."
        ));
        self.publish_snapshot()
    }

    /// Full teardown used by tray Quit and SIGTERM: stop every tunnel via the
    /// root helper (falling back to direct calls), kill Tor, then exit.
    /// Covers: GUI Quit button, tray Quit, tray Disconnect&Quit, SIGTERM from
    /// task manager (`kill`/`pkill`), and the X close→Quit fallback.
    /// Every helper call is spawn_blocking so a hung Unix socket never stalls
    /// the async select loop — and every direct platform call is retried even
    /// if the helper already cleaned up (idempotent).
    async fn teardown_and_quit(&mut self) -> Result<()> {
        tracing::warn!("teardown_and_quit: stopping all tunnels (user Quit / SIGTERM / task manager)");
        #[cfg(target_os = "linux")]
        {
            // Helper owns TUN/routes (root). Ask it first — still try direct
            // cleanup afterwards so a helper that wasn't running is not fatal.
            let _ = tokio::task::spawn_blocking(|| {
                let _ = crate::helper::send("tor_stop", serde_json::json!({}));
                let _ = crate::helper::send("wg_stop", serde_json::json!({}));
                let _ = crate::helper::send("ovpn_stop", serde_json::json!({}));
                let _ = crate::helper::send("pptp_stop", serde_json::json!({}));
                let _ = crate::helper::send("ss_stop", serde_json::json!({}));
            })
            .await;
            let _ = tokio::task::spawn_blocking(|| {
                let _ = platform::stop_tor_system_tunnel();
                let _ = platform::stop_wireguard_global();
                let _ = platform::stop_openvpn();
                let _ = platform::stop_pptp();
                let _ = platform::stop_outline();
                let _ = platform::kill_process_by_name("tor");
            })
            .await;
            // Final hard kill of any remaining tor children (pdeathsig not set
            // on older Tor builds → orphans after SIGKILL of GUI).
            let _ = tokio::task::spawn_blocking(|| platform::kill_process_by_name("tor"))
                .await;
        }
        #[cfg(target_os = "windows")]
        {
            let _ = platform::stop_tor_system_tunnel();
            let _ = platform::stop_wireguard_global();
            let _ = platform::stop_pptp();
            let _ = platform::stop_outline();
            silent_windows_kill_image("tor.exe");
        }
        self.tor_system_route_active = false;
        self.tor_socks_port = None;
        self.tor_route_attempts = 0;
        let _ = self.persist_client_state();
        // Give the helper a beat to apply, then hard-exit the GUI.
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        std::process::exit(0);
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.set_op_progress("disconnect", 0.08, "Stopping tunnel…");
        let mut removal_notice = String::new();
        // Capture the active connection before we clear it so UI notices
        // "Disconnecting…" immediately. `publish_snapshot` is emitted inside
        // `set_op_progress`, so the button disables right away.
        let taken = self.active_connection.take();
        let _ = self.publish_snapshot();
        if let Some(active) = taken {
            if active.server_id == "tor_local" {
                self.set_op_progress("disconnect", 0.25, "Stopping Tor system route…");
                #[cfg(target_os = "windows")]
                {
                    if let Err(error) = platform::stop_tor_system_tunnel() {
                        removal_notice = format!("Tor tunnel stop warning: {error:#}. ");
                    }
                }
                #[cfg(target_os = "linux")]
                {
                    // Helper owns the TUN (root); GUI can only ask it to tear down.
                    // Run in blocking pool so we never stall the tokio select loop
                    // for 90s on a hung socket.
                    let _ = tokio::task::spawn_blocking(|| crate::helper::send("tor_stop", serde_json::json!({})))
                        .await
                        .unwrap_or(None);
                    let _ = tokio::task::spawn_blocking(|| platform::stop_tor_system_tunnel())
                        .await
                        .unwrap_or(Ok(()))
                        .map_err(|error| removal_notice = format!("Tor tunnel stop warning: {error:#}. "));
                }
                self.tor_system_route_active = false;
                self.tor_socks_port = None;
                self.tor_route_attempts = 0;
                self.set_op_progress("disconnect", 0.55, "Stopping Tor process…");
                #[cfg(target_os = "windows")]
                {
                    silent_windows_kill_image("tor.exe");
                    clear_stale_wininet_socks_hint();
                }
                #[cfg(target_os = "linux")]
                {
                    let _ = tokio::task::spawn_blocking(|| platform::kill_process_by_name("tor"))
                        .await
                        .unwrap_or(0);
                    // Belt-and-braces: ensure routes are gone even if the
                    // helper worker was detached (stop_tor_system_tunnel already
                    // runs tproxy_remove internally when detached, but we retry
                    // once more here for the GUI-owned slot).
                    let _ = tokio::task::spawn_blocking(|| {
                        let _ = platform::stop_tor_system_tunnel();
                    })
                    .await;
                }
                removal_notice.push_str(
                    "Tor stopped; system routes restored (no permanent OS proxy was installed).",
                );
            } else if active.server_id.starts_with("ovpn_") {
                self.set_op_progress("disconnect", 0.4, "Stopping OpenVPN…");
                #[cfg(target_os = "linux")]
                {
                    let _ = tokio::task::spawn_blocking(|| crate::helper::send("ovpn_stop", serde_json::json!({})))
                        .await
                        .unwrap_or(None);
                    let _ = tokio::task::spawn_blocking(|| platform::stop_openvpn())
                        .await
                        .unwrap_or(Ok(()));
                }
                #[cfg(not(target_os = "linux"))]
                crate::ovpn::kill_openvpn_processes();
                #[cfg(target_os = "linux")]
                crate::ovpn::kill_openvpn_processes();
                removal_notice.push_str("OpenVPN disconnected; profile stopped.");
            } else if active.server_id.starts_with("wg_") {
                self.set_op_progress("disconnect", 0.4, "Stopping WireGuard…");
                #[cfg(target_os = "linux")]
                {
                    let _ = tokio::task::spawn_blocking(|| crate::helper::send("wg_stop", serde_json::json!({})))
                        .await
                        .unwrap_or(None);
                    let _ = tokio::task::spawn_blocking(|| platform::stop_wireguard_global())
                        .await
                        .unwrap_or(Ok(()));
                }
                #[cfg(target_os = "windows")]
                {
                    let _ = platform::stop_wireguard_global();
                }
                #[cfg(not(any(target_os = "windows", target_os = "linux")))]
                {
                    let _ = platform::stop_wireguard_global();
                }
                removal_notice.push_str("WireGuard disconnected; tunnel stopped.");
            } else if active.server_id.starts_with("pptp_") {
                self.set_op_progress("disconnect", 0.4, "Stopping PPTP…");
                #[cfg(target_os = "linux")]
                {
                    let _ = tokio::task::spawn_blocking(|| crate::helper::send("pptp_stop", serde_json::json!({})))
                        .await
                        .unwrap_or(None);
                    let _ = tokio::task::spawn_blocking(|| platform::stop_pptp())
                        .await
                        .unwrap_or(Ok(()));
                }
                #[cfg(target_os = "windows")]
                {
                    let _ = platform::stop_pptp();
                }
                removal_notice.push_str("PPTP disconnected.");
            } else if active.server_id.starts_with("outline_") {
                self.set_op_progress("disconnect", 0.3, "Stopping Outline TUN…");
                #[cfg(target_os = "linux")]
                {
                    let _ = tokio::task::spawn_blocking(|| crate::helper::send("ss_stop", serde_json::json!({})))
                        .await
                        .unwrap_or(None);
                    let _ = tokio::task::spawn_blocking(|| platform::stop_outline())
                        .await
                        .unwrap_or(Ok(()));
                }
                #[cfg(target_os = "windows")]
                {
                    let _ = platform::stop_outline();
                }
                self.set_op_progress("disconnect", 0.65, "Outline stopped…");
                removal_notice.push_str("Outline disconnected; SOCKS/TUN stopped.");
            } else {
                self.set_op_progress("disconnect", 0.4, "Tearing down session…");
                if let Some(session_id) = active.session_id.clone() {
                    let _ = send_disconnect_notice(
                        &active.endpoint,
                        &active.server_id,
                        &self.config.client_id,
                        &session_id,
                    )
                    .await;
                }
                // Also clear any control-plane tunnel that might be up.
                #[cfg(target_os = "linux")]
                {
                    let _ = tokio::task::spawn_blocking(|| crate::helper::send("wg_stop", serde_json::json!({})))
                        .await
                        .unwrap_or(None);
                }
                self.remove_kernel_tunnel()?;
                removal_notice = remove_platform_tunnel();
            }
        } else {
            self.set_op_progress("disconnect", 0.35, "Cleaning residual tunnels…");
            removal_notice = remove_platform_tunnel();
            #[cfg(target_os = "windows")]
            {
                let _ = platform::stop_tor_system_tunnel();
                let _ = platform::stop_wireguard_global();
                let _ = platform::stop_pptp();
                let _ = platform::stop_outline();
                self.tor_system_route_active = false;
                self.tor_socks_port = None;
                self.tor_route_attempts = 0;
            }
            #[cfg(target_os = "linux")]
            {
                let _ = tokio::task::spawn_blocking(|| {
                    let _ = crate::helper::send("tor_stop", serde_json::json!({}));
                    let _ = crate::helper::send("wg_stop", serde_json::json!({}));
                    let _ = crate::helper::send("ovpn_stop", serde_json::json!({}));
                    let _ = crate::helper::send("pptp_stop", serde_json::json!({}));
                    let _ = crate::helper::send("ss_stop", serde_json::json!({}));
                })
                .await;
                let _ = tokio::task::spawn_blocking(|| {
                    let _ = platform::stop_tor_system_tunnel();
                    let _ = platform::stop_wireguard_global();
                    let _ = platform::stop_pptp();
                    let _ = platform::stop_outline();
                    let _ = platform::kill_process_by_name("tor");
                })
                .await;
                self.tor_system_route_active = false;
                self.tor_socks_port = None;
                self.tor_route_attempts = 0;
            }
        }

        self.active_connection = None;
        self.tor_system_route_active = false;
        self.notice = Some(format!("Disconnected. {removal_notice} Refreshing Your IP…"));
        self.set_op_progress("disconnect", 0.8, "Refreshing Your IP…");
        // Restore real public IP + globe home pin after tunnel teardown.
        let _ = self.refresh_local_ip().await;
        self.set_op_progress("disconnect", 1.0, "Disconnected");
        tokio::time::sleep(Duration::from_millis(250)).await;
        self.clear_op_progress();
        self.persist_client_state()?;
        self.publish_snapshot()
    }

    /// Resolve the user's *real* (non-Tor) public IP + GeoIP details and
    /// stash them on `self.local_ip_info` so the right-pane "Your IP
    /// Details" card has live data the moment the GUI wakes up.
    ///
    /// This is independent of any VPN/Tor session — it always hits
    /// ip-api.com directly and reflects the IP your ISP currently assigns
    /// to your router. The right pane uses it to show the user "this is
    /// who you look like right now" before any tunnel is up.
    async fn refresh_local_ip(&mut self) -> Result<()> {
        match crate::tor_geo::resolve_local_ip().await {
            Some(info) => {
                tracing::info!(
                    "Local IP refreshed: {} ({})",
                    info.ip,
                    info.country_code
                );
                self.local_ip_info = Some(info);
                self.local_ip_refresh_unix = unix_now();
                // Always bump so manual Refresh re-triggers globe pan/tilt
                // even when the public IP string did not change.
                self.globe_pan_token = self.globe_pan_token.wrapping_add(1);
            }
            None => {
                tracing::warn!(
                    "Local IP / GeoIP lookup failed (ip-api unreachable or rate-limited); \
                     right pane will keep showing the last good result or 'Looking up...'"
                );
                // Don't clobber a previously-good result on a transient
                // failure — leave the prior `local_ip_info` in place and
                // just don't bump the refresh timestamp.
            }
        }
        Ok(())
    }

    /// After Tor/OpenVPN hit Connected (100%), refresh **Your IP** via reverse
    /// GeoIP so the right pane + globe show the public address the world sees.
    ///
    /// - Tor: lookup through local SOCKS5 (exit IP)
    /// - OpenVPN / system routes: direct lookup (follows default route)
    /// - Idle: same as manual Refresh
    async fn refresh_public_ip_after_tunnel(&mut self) -> Result<()> {
        let server_id = self
            .active_connection
            .as_ref()
            .map(|a| a.server_id.clone())
            .unwrap_or_default();

        if server_id == "tor_local" {
            if let Some(port) = self.tor_socks_port {
                // Brief settle so the circuit / system tunnel is usable.
                tokio::time::sleep(Duration::from_secs(1)).await;
                if let Some(client) = crate::tor_geo::tor_proxy_client(port) {
                    if let Some(info) =
                        crate::tor_geo::resolve_tor_exit(&client, self.geoip.as_deref()).await
                    {
                        tracing::info!(
                            "Your IP refreshed via Tor SOCKS after connect: {} ({})",
                            info.ip,
                            info.country_code
                        );
                        // Keep active connection exit card in sync.
                        if let Some(active) = self.active_connection.as_mut() {
                            if active.server_id == "tor_local" {
                                if !info.country_code.is_empty() {
                                    active.country_code = Some(info.country_code.clone());
                                }
                                if !info.ip.is_empty() {
                                    active.endpoint = info.ip.clone();
                                }
                                active.tor_exit_info = Some(info.clone());
                            }
                        }
                        self.local_ip_info = Some(info);
                        self.local_ip_refresh_unix = unix_now();
                        self.globe_pan_token = self.globe_pan_token.wrapping_add(1);
                        return Ok(());
                    }
                }
            }
            // Fall through: keep any exit_info already written into local_ip_info.
            return Ok(());
        }

        // OpenVPN / WireGuard / PPTP / Outline: wait for routes, then reverse-IP.
        let is_profile_tunnel = server_id.starts_with("ovpn_")
            || server_id.starts_with("wg_")
            || server_id.starts_with("pptp_")
            || server_id.starts_with("outline_");
        if is_profile_tunnel {
            // WireGuard / Outline often need a few extra seconds for handshake
            // + route install before the public IP flips.
            let (settle, attempts, gap) = if server_id.starts_with("pptp_") {
                (Duration::from_secs(3), 6usize, Duration::from_millis(1500))
            } else if server_id.starts_with("wg_") {
                (Duration::from_secs(2), 8usize, Duration::from_millis(1200))
            } else if server_id.starts_with("outline_") {
                (Duration::from_secs(2), 8usize, Duration::from_millis(1200))
            } else {
                (Duration::from_secs(2), 5usize, Duration::from_secs(1))
            };
            tokio::time::sleep(settle).await;
            let before = self.local_ip_info.as_ref().map(|i| i.ip.clone());
            let mut changed = false;
            for attempt in 0..attempts {
                let stage = 0.85 + 0.12 * ((attempt as f32 + 1.0) / attempts as f32);
                self.set_op_progress(
                    "connect",
                    stage.min(0.97),
                    &format!("Refreshing Your IP… (try {}/{})", attempt + 1, attempts),
                );
                let _ = self.refresh_local_ip().await;
                let after = self.local_ip_info.as_ref().map(|i| i.ip.clone());
                if before != after && after.as_ref().map(|s| !s.is_empty()).unwrap_or(false) {
                    tracing::info!(
                        "Your IP changed after tunnel connect ({server_id}): {:?} → {:?}",
                        before,
                        after
                    );
                    changed = true;
                    break;
                }
                if attempt + 1 < attempts {
                    tokio::time::sleep(gap).await;
                }
            }
            if !changed {
                tracing::warn!(
                    "Your IP unchanged after tunnel connect ({server_id}, {:?}) — \
                     routes may still be settling; use Refresh",
                    self.local_ip_info.as_ref().map(|i| i.ip.clone())
                );
                // Still bump pan token so globe recenters on current geo.
                self.globe_pan_token = self.globe_pan_token.wrapping_add(1);
            }
            // Stash geo on active connection so cards + globe share the same exit.
            if let (Some(info), Some(active)) = (
                self.local_ip_info.clone(),
                self.active_connection.as_mut(),
            ) {
                if active.server_id == server_id {
                    if !info.country_code.is_empty() {
                        active.country_code = Some(info.country_code.clone());
                    }
                    active.tor_exit_info = Some(info);
                }
            }
            return Ok(());
        }

        self.refresh_local_ip().await
    }

    /// Apply (or remove) the system-wide routing for an active Tor SOCKS5
    /// session. On Windows this requires Administrator and uses
    /// `start_tor_system_tunnel()` / `stop_tor_system_tunnel()` from the
    /// platform crate; on other operating systems it's a no-op because the
    /// Tor SOCKS5 proxy alone is the only thing we expose.
    async fn apply_tor_system_route(&mut self) -> Result<()> {
        #[cfg(target_os = "windows")]
        {
            if !platform::is_elevated() {
                // Relaunch elevated and auto-reconnect Tor (fresh process has no
                // live SOCKS; ConnectTor after elevation rebuilds everything).
                match platform::relaunch_elevated_with_args(&["--auto-connect-tor"]) {
                    Ok(()) => {
                        self.notice = Some(String::from(
                            "Accepted UAC — restarting elevated for system-wide Tor VPN…",
                        ));
                        let _ = self.publish_snapshot();
                        platform::exit_after_relaunch();
                    }
                    Err(error) => {
                        self.notice = Some(format!(
                            "System-wide Tor VPN needs Administrator: {error:#}"
                        ));
                        return self.publish_snapshot();
                    }
                }
            }

            let port = match self.tor_socks_port {
                Some(port) => port,
                None => {
                    self.notice = Some(String::from(
                        "No active Tor SOCKS5 session — connecting Tor first…",
                    ));
                    let _ = self.publish_snapshot();
                    return self.connect_to_tor().await;
                }
            };
            match platform::start_tor_system_tunnel(port) {
                Ok(()) => {
                    self.tor_system_route_active = true;
                    self.notice = Some(format!(
                        "Tor system-wide VPN ACTIVE (Wintun → SOCKS5 127.0.0.1:{port}). All apps route through Tor."
                    ));
                }
                Err(error) => {
                    self.tor_system_route_active = false;
                    self.notice = Some(format!(
                        "Could not install Tor system-wide route: {error:#}"
                    ));
                }
            }
        }
        #[cfg(target_os = "linux")]
        {
            // Helper daemon path: root service does the work, GUI stays put —
            // no pkexec prompt, no second window.
            let port_opt = self.tor_socks_port;
            if port_opt.is_none() {
                self.notice = Some(String::from(
                    "No active Tor SOCKS5 session — connecting Tor first…",
                ));
                let _ = self.publish_snapshot();
                return self.connect_to_tor().await;
            }
            match crate::helper::send(
                "tor_start",
                serde_json::json!({ "socks_port": port_opt.unwrap() }),
            ) {
                Some(reply) if reply.get("ok").and_then(|v| v.as_bool()) == Some(true) => {
                    self.tor_system_route_active = true;
                    self.notice = Some(String::from(
                        "Tor VPN connected — all apps now use Tor.",
                    ));
                    return self.publish_snapshot();
                }
                Some(reply) => {
                    let err = reply
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("helper error")
                        .to_string();
                    self.tor_system_route_active = false;
                    // Bootstrap still in progress (no established guards yet):
                    // retry automatically instead of failing hard.
                    if err.contains("no established guard") && self.tor_route_attempts < 24 {
                        self.tor_route_attempts += 1;
                        let n = self.tor_route_attempts;
                        self.notice = Some(format!(
                            "Tor bootstrapping… enabling system-wide VPN automatically ({n}/24)"
                        ));
                        let _ = self.publish_snapshot();
                        let tx2 = self.command_tx.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                            let _ = tx2.send(ClientCommand::ApplyTorSystemRoute);
                        });
                        return self.publish_snapshot();
                    }
                    self.tor_route_attempts = 0;
                    self.notice = Some(format!("Could not install Tor system-wide route: {err}"));
                    return self.publish_snapshot();
                }
                None => { /* no helper — legacy flow below */ }
            }

            // Legacy fallback: connect Tor first if needed, then relaunch elevated.
            if !platform::is_elevated() {
                if port_opt.is_none() {
                    self.notice = Some(String::from(
                        "No active Tor SOCKS5 session — connecting Tor first…",
                    ));
                    let _ = self.publish_snapshot();
                    return self.connect_to_tor().await;
                }
                match platform::relaunch_elevated_with_args(&["--auto-connect-tor"]) {
                    Ok(()) => {
                        self.notice = Some(String::from(
                            "Accepted — restarting elevated for system-wide Tor VPN (pkexec)…",
                        ));
                        let _ = self.publish_snapshot();
                        platform::exit_after_relaunch();
                    }
                    Err(error) => {
                        self.notice = Some(format!(
                            "System-wide Tor VPN needs Administrator: {error:#}"
                        ));
                        return self.publish_snapshot();
                    }
                }
            }

            let port = match port_opt {
                Some(port) => port,
                None => {
                    self.notice = Some(String::from(
                        "No active Tor SOCKS5 session — connecting Tor first…",
                    ));
                    let _ = self.publish_snapshot();
                    return self.connect_to_tor().await;
                }
            };
            match platform::start_tor_system_tunnel(port) {
                Ok(()) => {
                    self.tor_system_route_active = true;
                    self.notice = Some(format!(
                        "Tor system-wide VPN ACTIVE (TUN → SOCKS5 127.0.0.1:{port}). All apps route through Tor."
                    ));
                }
                Err(error) => {
                    self.tor_system_route_active = false;
                    self.notice = Some(format!(
                        "Could not install Tor system-wide route: {error:#}"
                    ));
                }
            }
        }
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            self.notice = Some(String::from(
                "System-wide routing is only implemented on Windows and Linux. Use the Tor SOCKS5 proxy directly on other platforms.",
            ));
        }
        self.publish_snapshot()
    }

    async fn remove_tor_system_route(&mut self) -> Result<()> {
        #[cfg(target_os = "windows")]
        {
            match platform::stop_tor_system_tunnel() {
                Ok(()) => {
                    self.tor_system_route_active = false;
                    self.notice = Some(String::from(
                        "Tor system-wide route removed. SOCKS5 proxy is still running.",
                    ));
                }
                Err(error) => {
                    self.notice = Some(format!(
                        "Could not remove Tor system-wide route: {error:#}"
                    ));
                }
            }
        }
        #[cfg(target_os = "linux")]
        {
            // Helper first (root service tears down TUN/routes).
            let _ = crate::helper::send("tor_stop", serde_json::json!({}));
            match platform::stop_tor_system_tunnel() {
                Ok(()) => {
                    self.tor_system_route_active = false;
                    self.notice = Some(String::from(
                        "Tor system-wide route removed. SOCKS5 proxy is still running.",
                    ));
                }
                Err(error) => {
                    self.notice = Some(format!(
                        "Could not remove Tor system-wide route: {error:#}"
                    ));
                }
            }
        }
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            self.tor_system_route_active = false;
            self.notice = Some(String::from(
                "System-wide routing is only implemented on Windows and Linux.",
            ));
        }
        self.publish_snapshot()
    }

    #[allow(unused_variables)]
    fn apply_platform_tunnel_for_connection(
        &mut self,
        server: &ServerSummary,
        lease: &ControlSessionLease,
        protocol: vpn_suite_core::model::VpnProtocol,
    ) -> String {
        let profile_path = self.client_state.last_tunnel_profile_path.as_deref();

        if protocol == vpn_suite_core::model::VpnProtocol::OpenVPN {
            if let Some(path) = profile_path {
                crate::ovpn::kill_openvpn_processes();
                match crate::ovpn::find_openvpn_exe() {
                    Some(exe) => match crate::ovpn::spawn_openvpn(&exe, std::path::Path::new(path))
                    {
                        Ok(child) => {
                            std::mem::forget(child);
                            return format!(
                                "OpenVPN tunnel launched via {}.",
                                exe.display()
                            );
                        }
                        Err(error) => {
                            return format!("OpenVPN tunnel failed to spawn: {error:#}");
                        }
                    },
                    None => {
                        return String::from(
                            "OpenVPN not found. Import a .ovpn and use Connect to auto-provision, or install OpenVPN Community.",
                        );
                    }
                }
            }
            return String::from("OpenVPN tunnel failed: no profile path.");
        }

        #[cfg(target_os = "windows")]
        {
            let checks = platform::apply_client_tunnel_service(profile_path);
            if checks.is_empty() {
                return String::from("Tunnel: no action taken (no profile).");
            }
            let passed = checks
                .iter()
                .filter(|c| c.status == vpn_suite_core::setup::SetupStatus::Pass)
                .count();
            let failed = checks
                .iter()
                .filter(|c| c.status == vpn_suite_core::setup::SetupStatus::Fail)
                .count();
            let warned = checks
                .iter()
                .filter(|c| c.status == vpn_suite_core::setup::SetupStatus::Warn)
                .count();
            if failed > 0 {
                let detail = checks
                    .iter()
                    .filter(|c| {
                        matches!(
                            c.status,
                            vpn_suite_core::setup::SetupStatus::Fail
                                | vpn_suite_core::setup::SetupStatus::Warn
                        )
                    })
                    .map(|c| format!("{}: {}", c.name, c.detail))
                    .collect::<Vec<_>>()
                    .join("; ");
                return format!("Tunnel setup issues: {detail}");
            }
            return format!("VPN tunnel active ({passed} ok, {warned} warnings).");
        }

        #[cfg(target_os = "linux")]
        {
            let checks = vpn_platform_linux::apply_client_tunnel(&self.config, server, lease);
            let passed = checks
                .iter()
                .filter(|c| c.status == vpn_suite_core::setup::SetupStatus::Pass)
                .count();
            let failed = checks
                .iter()
                .filter(|c| c.status == vpn_suite_core::setup::SetupStatus::Fail)
                .count();
            if failed > 0 {
                return format!("Tunnel setup failed ({failed} errors).");
            }
            return format!("VPN tunnel active ({passed} steps passed).");
        }

        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            String::from("Tunnel: platform not supported.")
        }
    }

    async fn apply_kernel_tunnel(&mut self) -> Result<()> {
        let Some(active) = self.active_connection.clone() else {
            self.notice = Some(String::from(
                "Connect to a server before applying a tunnel.",
            ));
            return self.publish_snapshot();
        };
        let Some(session_id) = active.session_id.clone() else {
            self.notice = Some(String::from("No active session id is available."));
            return self.publish_snapshot();
        };

        let status = query_server_status(
            &active.endpoint,
            &active.server_id,
            Some(self.config.client_id.clone()),
            Some(session_id),
        )
        .await?;
        let Some(lease) = status.active_session else {
            self.notice = Some(String::from("Server did not return an active session."));
            return self.publish_snapshot();
        };

        let Some(server) = self.servers.get(&active.server_id).cloned() else {
            self.notice = Some(String::from(
                "Refresh discovery before applying the tunnel.",
            ));
            return self.publish_snapshot();
        };
        let tunnel_notice = self.apply_platform_tunnel_for_connection(&server, &lease, active.protocol);
        self.notice = Some(tunnel_notice);
        self.publish_snapshot()
    }

    fn remove_kernel_tunnel(&mut self) -> Result<()> {
        self.notice = Some(remove_platform_tunnel());
        self.publish_snapshot()
    }

    async fn poll_local_server_status(&mut self) {
        use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
        use std::time::Duration;
        use tokio::net::UdpSocket;
        use tokio::time::timeout;
        use vpn_suite_core::protocol::{
            decode_packet, encode_packet, Packet, StatusQuery, MAX_PACKET_SIZE, PROTOCOL_VERSION,
        };

        self.local_server_active = false;
        self.local_server_peers = 0;

        let Ok(paths) = server_paths() else {
            return;
        };
        let Ok(srv_cfg) = load_or_create_server_config(&paths, &server_bootstrap_options()) else {
            return;
        };

        self.local_server_port = srv_cfg.listen_port;
        self.local_server_public_key = srv_cfg.wireguard_keys.public_key.clone();

        let payload = match encode_packet(&Packet::StatusQuery(StatusQuery {
            protocol_version: PROTOCOL_VERSION,
            server_id: srv_cfg.server_id.clone(),
            client_id: None,
            session_id: None,
        })) {
            Ok(payload) => payload,
            Err(_) => return,
        };

        for target in [
            SocketAddr::new(Ipv6Addr::LOCALHOST.into(), srv_cfg.listen_port),
            SocketAddr::new(Ipv4Addr::LOCALHOST.into(), srv_cfg.listen_port),
        ] {
            let bind_addr = match target {
                SocketAddr::V4(_) => SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0),
                SocketAddr::V6(_) => SocketAddr::new(Ipv6Addr::LOCALHOST.into(), 0),
            };
            let Ok(socket) = UdpSocket::bind(bind_addr).await else {
                continue;
            };
            if socket.send_to(&payload, target).await.is_err() {
                continue;
            }

            let mut buffer = [0_u8; MAX_PACKET_SIZE];
            match timeout(Duration::from_millis(250), socket.recv_from(&mut buffer)).await {
                Ok(Ok((bytes_read, _))) => {
                    if let Ok(Packet::StatusResponse(status)) = decode_packet(&buffer[..bytes_read])
                    {
                        self.local_server_active = true;
                        self.local_server_peers = status.connected_peers;
                        return;
                    }
                }
                _ => continue,
            }
        }
    }

    async fn poll_active_connection(&mut self) -> Result<()> {
        self.prune_cooldowns()?;
        self.poll_local_server_status().await;

        let Some(active) = self.active_connection.clone() else {
            self.publish_snapshot()?;
            return Ok(());
        };

        if active.phase == ConnectionPhase::Cooldown {
            if let Some(until) = active.cooldown_until_unix {
                if until <= unix_now() {
                    self.active_connection = None;
                    self.notice = Some(String::from("Cooldown expired. You can try again."));
                    self.apply_cooldowns_to_servers();
                    self.persist_client_state()?;
                    self.publish_snapshot()?;
                }
            }
            return Ok(());
        }

        let Some(session_id) = active.session_id.clone() else {
            // For OpenVPN files, we don't have a session ID, but we should check if openvpn.exe is still running
            if active.server_id.starts_with("ovpn_") {
                #[cfg(target_os = "windows")]
                {
                    use std::os::windows::process::CommandExt;
                    let output = std::process::Command::new("tasklist")
                        .args(["/FI", "IMAGENAME eq openvpn.exe", "/NH"])
                        .creation_flags(0x08000000)
                        .output();
                    
                    if let Ok(output) = output {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        if !stdout.to_lowercase().contains("openvpn.exe") {
                            // Process died or failed to start
                            if let Some(id_str) = active.server_id.strip_prefix("ovpn_") {
                                if let Ok(id) = id_str.parse::<i64>() {
                                    let _ = crate::db::increment_ovpn_fail_count(id);
                                    let _ = self.event_tx.send(ClientEvent::ReloadOvpnConfigs);
                                }
                            }
                            self.active_connection = None;
                            self.notice = Some(String::from("OpenVPN connection failed or process died."));
                            self.persist_client_state()?;
                            self.publish_snapshot()?;
                        }
                    }
                }
            }
            return Ok(());
        };

        match query_server_status(
            &active.endpoint,
            &active.server_id,
            Some(self.config.client_id.clone()),
            Some(session_id),
        )
        .await
        {
            Ok(status) => {
                self.consume_status(active, status)?;
            }
            Err(error) => {
                if let Some(connection) = self.active_connection.as_mut() {
                    connection.phase = ConnectionPhase::Reconnecting;
                    connection.attempt_count = connection.attempt_count.saturating_add(1);
                    connection.last_status_unix = Some(unix_now());
                    if connection.attempt_count >= 4 {
                        connection.phase = ConnectionPhase::Error;
                    }
                }
                self.notice = Some(format!("Background status check failed: {error:#}"));
            }
        }

        self.publish_snapshot()
    }

    fn consume_status(&mut self, previous: ActiveConnection, status: StatusResponse) -> Result<()> {
        let now = unix_now();
        self.update_server_message(
            &status.server_id,
            status.banner_message.clone().or_else(|| {
                if status.locked_down {
                    Some(String::from("Server locked down."))
                } else {
                    None
                }
            }),
        );

        if status.locked_down {
            self.active_connection = Some(ActiveConnection {
                protocol: vpn_suite_core::model::VpnProtocol::WireGuard,
        phase: ConnectionPhase::Error,
                tor_exit_info: None,
                country_code: None,
                last_status_unix: Some(now),
                ..previous
            });
            self.persist_client_state()?;
            self.notice = Some(String::from(
                "SERVER LOCKED DOWN — Contact the server owner to restore access.",
            ));
            return Ok(());
        }

        if let Some(lease) = status.active_session {
            let server_name = status.server_name.clone();
            let connected_peers = status.connected_peers;
            self.active_connection = Some(ActiveConnection {
                server_id: lease.server_id.clone(),
                server_name: server_name.clone(),
                endpoint: previous.endpoint,
                protocol: vpn_suite_core::model::VpnProtocol::WireGuard,
        phase: ConnectionPhase::Connected,
                connected_at_unix: previous.connected_at_unix.or(Some(now)),
                attempt_count: 0,
                session_id: Some(lease.session_id.clone()),
                reserved_client_ip: Some(lease.reserved_client_ip.clone()),
                server_internal_ip: Some(lease.server_internal_ip.clone()),
                tunnel_profile_path: previous.tunnel_profile_path,
                cooldown_until_unix: None,
                tor_exit_info: None,
                country_code: None,
                last_status_unix: Some(now),
            });
            self.persist_client_state()?;
            self.notice = Some(format!(
                "Connected to {}. {} authenticated peer(s) currently active.",
                server_name, connected_peers
            ));
        } else if previous.attempt_count >= 3 {
            self.active_connection = Some(ActiveConnection {
                protocol: vpn_suite_core::model::VpnProtocol::WireGuard,
        phase: ConnectionPhase::Error,
                tor_exit_info: None,
                country_code: None,
                last_status_unix: Some(now),
                attempt_count: previous.attempt_count.saturating_add(1),
                ..previous
            });
            self.persist_client_state()?;
            self.notice = Some(String::from(
                "The control session expired or could not be resumed.",
            ));
        } else {
            self.active_connection = Some(ActiveConnection {
                protocol: vpn_suite_core::model::VpnProtocol::WireGuard,
        phase: ConnectionPhase::Reconnecting,
                tor_exit_info: None,
                country_code: None,
                last_status_unix: Some(now),
                attempt_count: previous.attempt_count.saturating_add(1),
                ..previous
            });
            self.persist_client_state()?;
            self.notice = Some(String::from(
                "Session is being checked and may be reconnecting.",
            ));
        }

        Ok(())
    }

    async fn refresh(&mut self) -> Result<()> {
        self.prune_cooldowns()?;

        for server in self.servers.values_mut() {
            server.online = false;
        }

        let discovered = discover_servers(&self.config).await?;
        for server in discovered {
            self.servers
                .entry(server.server_id.clone())
                .and_modify(|existing| {
                    existing.name = server.name.clone();
                    existing.country_code = server.country_code.clone();
                    existing.country_name = server.country_name.clone();
                    existing.endpoint = server.endpoint.clone();
                    existing.wireguard_endpoint = server.wireguard_endpoint.clone();
                    existing.masked_endpoint = server.masked_endpoint.clone();
                    existing.listen_port = server.listen_port;
                    existing.has_password = server.has_password;
                    existing.last_seen_unix = server.last_seen_unix;
                    existing.public_key = server.public_key.clone();
                    existing.online = true;
                })
                .or_insert(server);
        }

        self.apply_cooldowns_to_servers();

        let online = self.servers.values().filter(|server| server.online).count();
        self.notice = Some(format!(
            "Discovered {} online node(s); {} saved node(s) are currently offline.",
            online,
            self.servers.len().saturating_sub(online)
        ));
        self.publish_snapshot()
    }

    fn prune_cooldowns(&mut self) -> Result<()> {
        let now = unix_now();
        let before = self.client_state.cooldowns.len();
        self.client_state
            .cooldowns
            .retain(|_, entry| entry.until_unix > now);
        if self.client_state.cooldowns.len() != before {
            save_client_state(&self.paths, &self.client_state)?;
        }
        Ok(())
    }

    fn set_cooldown(
        &mut self,
        server_id: String,
        endpoint: String,
        until_unix: u64,
        reason: String,
    ) -> Result<()> {
        self.client_state.cooldowns.insert(
            server_id.clone(),
            CooldownEntry {
                server_id,
                endpoint,
                until_unix,
                reason,
            },
        );
        save_client_state(&self.paths, &self.client_state)?;
        Ok(())
    }

    fn clear_cooldown(&mut self, server_id: &str) -> Result<()> {
        if self.client_state.cooldowns.remove(server_id).is_some() {
            save_client_state(&self.paths, &self.client_state)?;
        }
        Ok(())
    }

    fn apply_cooldowns_to_servers(&mut self) {
        let now = unix_now();
        for server in self.servers.values_mut() {
            if let Some(entry) = self.client_state.cooldowns.get(&server.server_id) {
                if entry.until_unix > now {
                    server.cooldown_until_unix = Some(entry.until_unix);
                    server.last_message = Some(entry.reason.clone());
                } else {
                    server.cooldown_until_unix = None;
                }
            } else {
                server.cooldown_until_unix = None;
            }
        }
    }

    fn update_server_message(&mut self, server_id: &str, message: Option<String>) {
        if let Some(server) = self.servers.get_mut(server_id) {
            server.last_message = message;
        }
    }

    fn write_tunnel_artifact(
        &self,
        server_id: &str,
        lease: &ControlSessionLease,
        protocol: vpn_suite_core::model::VpnProtocol,
    ) -> Result<String> {
        let server = self.servers.get(server_id).with_context(|| {
            format!("server {server_id} is not available for profile rendering")
        })?;
        
        if protocol == vpn_suite_core::model::VpnProtocol::OpenVPN {
            let artifact = vpn_suite_core::openvpn::build_client_artifact(&self.config, server, lease)?;
            return vpn_suite_core::control_plane::write_client_tunnel_artifact(&self.paths.profiles_dir, server_id, &artifact.contents);
        }
        
        let artifact = vpn_suite_core::wireguard::build_client_artifact(&self.config, server, lease)?;
        vpn_suite_core::control_plane::write_client_tunnel_artifact(&self.paths.profiles_dir, server_id, &artifact.contents)
    }
}

fn server_bootstrap_options() -> ServerBootstrapOptions {
    ServerBootstrapOptions {
        name: None,
        country_code: None,
        country_name: None,
        listen_port: None,
    }
}

fn command_output_detail(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("Exit code {:?}.", output.status.code())
    }
}

#[cfg(target_os = "windows")]
fn ensure_windows_server_host_setup() -> Result<std::process::Output> {
    let server_exe = find_windows_server_exe()
        .context("could not locate vpn-server.exe beside the installed client")?;
    let output = windows_command_no_window(&server_exe)
        .args(["host-setup", "apply"])
        .output()
        .with_context(|| format!("failed to run {}", server_exe.display()))?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(anyhow!(
            "{} returned a non-zero exit status. {}",
            server_exe.display(),
            command_output_detail(&output)
        ))
    }
}

#[cfg(target_os = "windows")]
fn spawn_windows_server_process() -> Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;

    const CREATE_NO_WINDOW: u32 = 0x08000000;
    let server_exe = find_windows_server_exe()
        .context("could not locate vpn-server.exe beside the installed client")?;

    let log_file = if let Ok(paths) = server_paths() {
        std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(true)
            .open(paths.base_dir.join("vpn-server-process.log"))
            .ok()
    } else {
        None
    };

    let mut cmd = std::process::Command::new(&server_exe);
    cmd.arg("run")
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);

    if let Some(ref file) = log_file {
        if let Ok(clone) = file.try_clone() {
            cmd.stdout(Stdio::from(clone));
        } else {
            cmd.stdout(Stdio::null());
        }
        if let Ok(clone2) = file.try_clone() {
            cmd.stderr(Stdio::from(clone2));
        } else {
            cmd.stderr(Stdio::null());
        }
    } else {
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
    }

    cmd.spawn()
        .with_context(|| format!("failed to start {}", server_exe.display()))
}

#[cfg(target_os = "windows")]
fn stop_windows_server_process() -> Result<String> {
    let pid_path = windows_server_pid_path()?;
    let mut details = Vec::new();
    if let Ok(pid_text) = std::fs::read_to_string(&pid_path) {
        let pid = pid_text.trim();
        if !pid.is_empty() {
            let output = windows_command_no_window("taskkill")
                .args(["/PID", pid, "/T", "/F"])
                .output()
                .with_context(|| format!("failed to stop pid {pid}"))?;
            details.push(format!("taskkill: {}", command_output_detail(&output)));
        }
    } else {
        let output = windows_command_no_window("taskkill")
            .args(["/IM", "vpn-server.exe", "/F"])
            .output()
            .with_context(|| String::from("failed to stop vpn-server.exe by image name"))?;
        details.push(format!("taskkill: {}", command_output_detail(&output)));
    }

    let server_exe = find_windows_server_exe()
        .context("could not locate vpn-server.exe beside the installed client")?;
    let cleanup = windows_command_no_window(&server_exe)
        .args(["host-setup", "remove"])
        .output()
        .with_context(|| format!("failed to run {} host-setup remove", server_exe.display()))?;
    details.push(format!("cleanup: {}", command_output_detail(&cleanup)));
    let _ = std::fs::remove_file(pid_path);
    Ok(format!(
        "Local server stop requested. {}",
        details.join(" | ")
    ))
}

#[cfg(target_os = "windows")]
fn save_windows_server_pid(pid: u32) -> Result<()> {
    let pid_path = windows_server_pid_path()?;
    if let Some(parent) = pid_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(&pid_path, pid.to_string())
        .with_context(|| format!("failed to write {}", pid_path.display()))
}

#[cfg(target_os = "windows")]
fn windows_server_pid_path() -> Result<PathBuf> {
    Ok(server_paths()?.base_dir.join("server-process.pid"))
}

#[cfg(target_os = "windows")]
fn find_windows_server_exe() -> Option<PathBuf> {
    let current_exe = env::current_exe().ok()?;
    let exe_dir = current_exe.parent()?;
    let sibling = exe_dir.join("vpn-server.exe");
    if sibling.exists() {
        return Some(sibling);
    }

    let install_bin = PathBuf::from(r"C:\Program Files\ZeroNode VPN\bin\vpn-server.exe");
    if install_bin.exists() {
        return Some(install_bin);
    }

    None
}

#[cfg(target_os = "windows")]
fn windows_command_no_window(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x08000000;
    let mut command = std::process::Command::new(program);
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

fn connection_from_lease(
    server_id: &str,
    server_name: &str,
    endpoint: &str,
    lease: Option<&ControlSessionLease>,
    tunnel_profile_path: Option<String>,
    country_code: Option<&str>,
) -> ActiveConnection {
    ActiveConnection {
        server_id: server_id.to_owned(),
        server_name: server_name.to_owned(),
        endpoint: endpoint.to_owned(),
        protocol: vpn_suite_core::model::VpnProtocol::WireGuard,
        phase: ConnectionPhase::Connected,
        connected_at_unix: Some(unix_now()),
        attempt_count: 0,
        session_id: lease.map(|lease| lease.session_id.clone()),
        reserved_client_ip: lease.map(|lease| lease.reserved_client_ip.clone()),
        server_internal_ip: lease.map(|lease| lease.server_internal_ip.clone()),
        tunnel_profile_path,
        cooldown_until_unix: None,
        tor_exit_info: None,
        country_code: country_code
            .filter(|cc| !cc.is_empty())
            .map(|cc| cc.to_uppercase()),
        last_status_unix: Some(unix_now()),
    }
}

fn remove_platform_tunnel() -> String {
    #[cfg(target_os = "windows")]
    {
        crate::ovpn::kill_openvpn_processes();
        let _ = std::process::Command::new("taskkill")
            .args(&["/F", "/IM", "openvpn.exe"])
            .status();

        let checks = platform::remove_client_tunnel_service();
        let passed = checks
            .iter()
            .filter(|c| c.status == vpn_suite_core::setup::SetupStatus::Pass)
            .count();
        return format!("Tunnel removed ({passed} step(s) passed).");
    }
    #[cfg(target_os = "linux")]
    {
        let checks = vec![vpn_platform_linux::remove_client_tunnel()];
        let passed = checks
            .iter()
            .filter(|c| c.status == vpn_suite_core::setup::SetupStatus::Pass)
            .count();
        return format!("Tunnel removed ({passed} step(s) passed).");
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        String::from("Tunnel removal: platform not supported.")
    }
}

// ---------------------------------------------------------------------------
// System tray (Linux) + SIGTERM graceful teardown
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod tray {
    use std::sync::mpsc::Sender;
    use tray_icon::{menu, TrayIcon, TrayIconBuilder};
    use crate::app::ClientCommand;

    static TRAY_AVAILABLE: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    pub fn is_available() -> bool {
        TRAY_AVAILABLE.load(std::sync::atomic::Ordering::SeqCst)
    }

    const MENU_SHOW_ID: &str = "zn-show";
    const MENU_DISCONNECT_QUIT_ID: &str = "zn-dq";
    const MENU_QUIT_ID: &str = "zn-quit";

    pub fn create_tray(command_tx: Sender<ClientCommand>) -> Option<TrayIcon> {
        // `muda`/gtk Menu requires GTK initialized — otherwise panic at
        // `gtk-0.18/src/auto/menu.rs:29:9: GTK has not been initialized`.
        // Must be called on the main thread before any `Menu::new()`.
        if gtk::init().is_err() {
            tracing::warn!("tray: gtk::init failed — tray disabled (no display?)");
            return None;
        }
        let icon = load_tray_image()?;

        let menu = menu::Menu::new();
        let item_show = menu::MenuItem::with_id(MENU_SHOW_ID, "Show ZeroNode", true, None);
        let item_dq =
            menu::MenuItem::with_id(MENU_DISCONNECT_QUIT_ID, "Disconnect && Quit", true, None);
        let item_quit = menu::MenuItem::with_id(MENU_QUIT_ID, "Quit", true, None);
        let _ = menu.append_items(&[&item_show, &item_dq, &item_quit]);

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("ZeroNode VPN")
            .with_icon(icon)
            .build()
            .ok()?;
        TRAY_AVAILABLE.store(true, std::sync::atomic::Ordering::SeqCst);

        // tray-icon on Linux needs GTK main-loop iterations to emit events.
        std::thread::Builder::new()
            .name("zn-tray-gtk".into())
            .spawn(|| loop {
                gtk_tick();
                std::thread::sleep(std::time::Duration::from_millis(50));
            })
            .ok();

        // Poll the global crossbeam menu-event channel and forward into app
        // commands. `QuitApp`/`SignalQuit` do full teardown (helper + tor kill)
        // so Disconnect&&Quit becomes an ordered Disconnect → QuitApp with a
        // long enough gap for the blocking helper stop to finish. If the gap
        // elapses before Disconnect finishes, QuitApp's own teardown still
        // guarantees cleanup (it stops every tunnel via helper again).
        let menu_rx = tray_icon::menu::MenuEvent::receiver().clone();
        std::thread::Builder::new()
            .name("zn-tray-menu".into())
            .spawn(move || loop {
                match menu_rx.try_recv() {
                    Ok(event) => {
                        let id: &str = &event.id.0;
                        if id == MENU_SHOW_ID {
                            let _ = command_tx.send(ClientCommand::ShowMainWindow);
                        } else if id == MENU_DISCONNECT_QUIT_ID {
                            let _ = command_tx.send(ClientCommand::Disconnect);
                            // Give Disconnect's blocking helper stop (5s join +
                            // tproxy) time to finish before QuitApp's final
                            // hard-exit. QuitApp itself re-stops everything
                            // anyway, so this is just ordering, not required.
                            std::thread::sleep(std::time::Duration::from_millis(2800));
                            let _ = command_tx.send(ClientCommand::QuitApp);
                        } else if id == MENU_QUIT_ID {
                            let _ = command_tx.send(ClientCommand::QuitApp);
                        }
                    }
                    Err(err) if err.is_empty() => {
                        std::thread::sleep(std::time::Duration::from_millis(80));
                    }
                    Err(_) => break,
                }
            })
            .ok();

        Some(tray)
    }

    fn load_tray_image() -> Option<tray_icon::Icon> {
        let png = include_bytes!("../../../assets/icon.png");
        let img = image::load_from_memory(png).ok()?.to_rgba8();
        let (w, h) = img.dimensions();
        tray_icon::Icon::from_rgba(img.into_raw(), w, h).ok()
    }

    fn gtk_tick() {
        // glib main iteration without blocking; keeps tray events flowing
        // alongside the winit event loop.
        unsafe extern "C" {
            fn g_main_context_default() -> *mut std::ffi::c_void;
            fn g_main_context_iteration(context: *mut std::ffi::c_void, may_block: i32) -> i32;
        }
        unsafe {
            g_main_context_iteration(g_main_context_default(), 0);
        }
    }
}

#[cfg(target_os = "linux")]
static SIGNAL_QUIT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(target_os = "linux")]
static SIGNAL_QUIT_TAKEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(target_os = "linux")]
extern "C" fn on_signal(_sig: libc::c_int) {
    SIGNAL_QUIT.store(true, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(target_os = "linux")]
fn install_signal_handlers(tx: Sender<ClientCommand>) {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        unsafe {
            libc::signal(libc::SIGTERM, on_signal as usize);
            libc::signal(libc::SIGINT, on_signal as usize);
        }
        // Forwarder thread so we never do real work inside the handler.
        std::thread::Builder::new()
            .name("zn-signal".into())
            .spawn(move || loop {
                if SIGNAL_QUIT.load(std::sync::atomic::Ordering::SeqCst)
                    && !SIGNAL_QUIT_TAKEN.swap(true, std::sync::atomic::Ordering::SeqCst)
                {
                    let _ = tx.send(ClientCommand::SignalQuit);
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            })
            .ok();
    });
}
