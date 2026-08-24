#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use anyhow::Result;
use eframe::{
    egui::{self, Color32, FontFamily, FontId, Margin, RichText, Stroke},
    App, NativeOptions,
};
use std::{
    fs,
    net::{Ipv4Addr, SocketAddr},
    time::{Duration, Instant},
};
use vpn_suite_core::{
    app_paths::server_paths,
    auth::RateLimitJournal,
    config::{
        load_or_create_journal, load_or_create_server_config, load_or_create_server_runtime,
        save_journal, ServerBootstrapOptions, ServerConfig,
    },
    model::ServerRuntimeSnapshot,
    protocol::{
        decode_packet, encode_packet, Packet, StatusQuery, StatusResponse, MAX_PACKET_SIZE,
        PROTOCOL_VERSION,
    },
    setup::{server_setup_report, SetupCheck, SetupStatus},
};

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .init();

    let paths = server_paths()?;
    let config = load_or_create_server_config(
        &paths,
        &ServerBootstrapOptions {
            name: None,
            country_code: None,
            country_name: None,
            listen_port: None,
        },
    )?;
    let journal = load_or_create_journal(&paths)?;
    let runtime = load_or_create_server_runtime(&paths)?;

    let options = NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1040.0, 700.0])
            .with_min_inner_size([920.0, 620.0])
            .with_title("ZeroNode Server Dashboard"),
        ..Default::default()
    };

    eframe::run_native(
        "ZeroNode Server Dashboard",
        options,
        Box::new(|_cc| {
            Ok(Box::new(ServerDashboard::new(
                paths, config, journal, runtime,
            )))
        }),
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))?;

    Ok(())
}

#[derive(Clone, Debug, Default)]
struct DaemonSnapshot {
    reachable: bool,
    connected_peers: u32,
    locked_down: bool,
    uptime_secs: u64,
    banner_message: Option<String>,
}

struct ServerDashboard {
    paths: vpn_suite_core::app_paths::AppPaths,
    config: ServerConfig,
    journal: RateLimitJournal,
    runtime: ServerRuntimeSnapshot,
    daemon: DaemonSnapshot,
    recent_events: Vec<String>,
    last_sync: Instant,
    notice: Option<String>,
}

impl ServerDashboard {
    fn new(
        paths: vpn_suite_core::app_paths::AppPaths,
        config: ServerConfig,
        journal: RateLimitJournal,
        runtime: ServerRuntimeSnapshot,
    ) -> Self {
        Self {
            paths,
            config,
            journal,
            runtime,
            daemon: DaemonSnapshot::default(),
            recent_events: Vec::new(),
            last_sync: Instant::now(),
            notice: None,
        }
    }

    fn refresh(&mut self) {
        if let Ok(config) = load_or_create_server_config(
            &self.paths,
            &ServerBootstrapOptions {
                name: None,
                country_code: None,
                country_name: None,
                listen_port: None,
            },
        ) {
            self.config = config;
        }
        if let Ok(journal) = load_or_create_journal(&self.paths) {
            self.journal = journal;
        }
        if let Ok(runtime) = load_or_create_server_runtime(&self.paths) {
            self.runtime = runtime;
        }
        self.daemon = query_local_status(&self.config).unwrap_or_default();
        self.recent_events = read_recent_events(&self.paths.log_file, 10);
        self.last_sync = Instant::now();
    }
}

impl App for ServerDashboard {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_secs(1));
        if self.last_sync.elapsed() > Duration::from_secs(1) {
            self.refresh();
        }

        let mut visuals = egui::Visuals::dark();
        visuals.override_text_color = Some(Color32::WHITE);
        visuals.panel_fill = Color32::BLACK;
        visuals.window_fill = Color32::from_rgb(13, 13, 13);
        visuals.widgets.inactive.bg_fill = Color32::from_rgb(13, 13, 13);
        visuals.widgets.hovered.bg_fill = Color32::from_rgb(26, 26, 26);
        visuals.widgets.active.bg_fill = Color32::from_rgb(0, 204, 102);
        ctx.set_visuals(visuals);

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading(
                RichText::new("ZeroNode Server Dashboard")
                    .font(FontId::new(28.0, FontFamily::Proportional))
                    .color(Color32::WHITE),
            );
            ui.label(
                RichText::new("Local-first visibility for the control plane daemon, denylist state, and recent event stream.")
                    .color(Color32::from_rgb(170, 170, 170)),
            );
            ui.add_space(14.0);

            if let Some(notice) = &self.notice {
                ui.label(RichText::new(notice).color(Color32::from_rgb(232, 237, 242)));
                ui.add_space(8.0);
            }

            ui.horizontal(|ui| {
                if ui
                    .button(RichText::new("Refresh").color(Color32::WHITE))
                    .clicked()
                {
                    self.refresh();
                }

                if ui
                    .button(RichText::new("Unlock Server").color(Color32::BLACK))
                    .clicked()
                {
                    self.journal.unlock();
                    if save_journal(&self.paths, &self.journal).is_ok() {
                        self.notice = Some(String::from("Lockdown state cleared."));
                    }
                    self.refresh();
                }
            });

            ui.add_space(12.0);
            dashboard_card(ui, "Server Name", &self.config.name);
            dashboard_card(
                ui,
                "Country",
                &format!(
                    "{} ({})",
                    self.config.country_name, self.config.country_code
                ),
            );
            dashboard_card(ui, "Control Port", &self.config.listen_port.to_string());
            dashboard_card(ui, "VPN Subnet", &self.config.vpn_subnet);
            dashboard_card(
                ui,
                "Password Protection",
                if self.config.password_hash.is_some() {
                    "Enabled"
                } else {
                    "Disabled"
                },
            );
            dashboard_card(
                ui,
                "Daemon Reachable",
                if self.daemon.reachable { "Yes" } else { "No" },
            );
            dashboard_card(
                ui,
                "Connected Peers",
                &self.runtime.connected_peers.max(self.daemon.connected_peers).to_string(),
            );
            dashboard_card(ui, "Daemon Uptime", &format_elapsed(self.daemon.uptime_secs));
            dashboard_card(
                ui,
                "Locked Down",
                if self.journal.is_locked_down()
                    || self.daemon.locked_down
                    || self.runtime.locked_down
                {
                    "Yes"
                } else {
                    "No"
                },
            );
            dashboard_card(
                ui,
                "Denied IP Count",
                &self.journal.banned_ips.len().to_string(),
            );
            dashboard_card(
                ui,
                "WireGuard Public Key",
                &self.config.wireguard_keys.public_key,
            );

            if let Some(message) = &self.daemon.banner_message {
                dashboard_card(ui, "Last Banner", message);
            }
            dashboard_card(ui, "Runtime File", &self.paths.runtime_file.display().to_string());

            ui.add_space(12.0);
            render_setup_panel(ui, &self.paths, &self.config, &mut self.notice);

            ui.add_space(12.0);
            egui::Frame::group(ui.style())
                .fill(Color32::from_rgb(13, 13, 13))
                .stroke(Stroke::new(1.0, Color32::from_rgb(31, 31, 31)))
                .inner_margin(Margin::same(14.0))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new("Recent Events")
                            .font(FontId::new(18.0, FontFamily::Proportional))
                            .color(Color32::WHITE),
                    );
                    ui.add_space(6.0);

                    if self.recent_events.is_empty() {
                        ui.label(
                            RichText::new("No event log entries yet.")
                                .color(Color32::from_rgb(170, 170, 170)),
                        );
                    } else {
                        for line in &self.recent_events {
                            ui.label(
                                RichText::new(line)
                                    .font(FontId::new(12.0, FontFamily::Monospace))
                                    .color(Color32::from_rgb(232, 237, 242)),
                            );
                        }
                    }
                });

            ui.add_space(12.0);
            egui::Frame::group(ui.style())
                .fill(Color32::from_rgb(13, 13, 13))
                .stroke(Stroke::new(1.0, Color32::from_rgb(31, 31, 31)))
                .inner_margin(Margin::same(14.0))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new("Active Sessions")
                            .font(FontId::new(18.0, FontFamily::Proportional))
                            .color(Color32::WHITE),
                    );
                    ui.add_space(6.0);

                    if self.runtime.sessions.is_empty() {
                        ui.label(
                            RichText::new("No active sessions recorded in runtime snapshot.")
                                .color(Color32::from_rgb(170, 170, 170)),
                        );
                    } else {
                        for session in &self.runtime.sessions {
                            ui.label(
                                RichText::new(format!(
                                    "{}  {}  {}  {}",
                                    session.client_name,
                                    session.remote_endpoint,
                                    session.reserved_client_ip,
                                    session
                                        .server_peer_config_path
                                        .as_deref()
                                        .unwrap_or("artifact pending")
                                ))
                                .font(FontId::new(12.0, FontFamily::Monospace))
                                .color(Color32::from_rgb(232, 237, 242)),
                            );
                        }
                    }
                });

            ui.add_space(12.0);
            egui::Frame::group(ui.style())
                .fill(Color32::from_rgb(13, 13, 13))
                .stroke(Stroke::new(1.0, Color32::from_rgb(31, 31, 31)))
                .inner_margin(Margin::same(14.0))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new("Denied IPs")
                            .font(FontId::new(18.0, FontFamily::Proportional))
                            .color(Color32::WHITE),
                    );
                    ui.add_space(6.0);

                    if self.journal.banned_ips.is_empty() {
                        ui.label(
                            RichText::new("No banned IPs recorded.")
                                .color(Color32::from_rgb(170, 170, 170)),
                        );
                    } else {
                        for ip in self.journal.banned_ips.iter() {
                            ui.label(
                                RichText::new(ip).font(FontId::new(13.0, FontFamily::Monospace)),
                            );
                        }
                    }
                });
        });
    }
}

fn query_local_status(config: &ServerConfig) -> Result<DaemonSnapshot> {
    let socket = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
    socket.set_read_timeout(Some(Duration::from_millis(250)))?;

    let payload = encode_packet(&Packet::StatusQuery(StatusQuery {
        protocol_version: PROTOCOL_VERSION,
        server_id: config.server_id.clone(),
        client_id: None,
        session_id: None,
    }))?;

    socket.send_to(
        &payload,
        SocketAddr::new(Ipv4Addr::LOCALHOST.into(), config.listen_port),
    )?;

    let mut buffer = [0_u8; MAX_PACKET_SIZE];
    let (bytes_read, _) = socket.recv_from(&mut buffer)?;

    match decode_packet(&buffer[..bytes_read])? {
        Packet::StatusResponse(StatusResponse {
            connected_peers,
            locked_down,
            uptime_secs,
            banner_message,
            ..
        }) => Ok(DaemonSnapshot {
            reachable: true,
            connected_peers,
            locked_down,
            uptime_secs,
            banner_message,
        }),
        _ => Ok(DaemonSnapshot::default()),
    }
}

fn read_recent_events(path: &std::path::Path, count: usize) -> Vec<String> {
    let Ok(contents) = fs::read_to_string(path) else {
        return Vec::new();
    };

    let mut lines = contents
        .lines()
        .rev()
        .take(count)
        .map(|line| line.to_owned())
        .collect::<Vec<_>>();
    lines.reverse();
    lines
}

fn format_elapsed(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;
    format!("{hours:02}:{minutes:02}:{secs:02}")
}

fn dashboard_card(ui: &mut egui::Ui, label: &str, value: &str) {
    egui::Frame::group(ui.style())
        .fill(Color32::from_rgb(13, 13, 13))
        .stroke(Stroke::new(1.0, Color32::from_rgb(31, 31, 31)))
        .inner_margin(Margin::same(12.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(label)
                        .font(FontId::new(15.0, FontFamily::Proportional))
                        .color(Color32::from_rgb(170, 170, 170)),
                );
                ui.separator();
                ui.label(
                    RichText::new(value)
                        .font(FontId::new(15.0, FontFamily::Monospace))
                        .color(Color32::WHITE),
                );
            });
        });
    ui.add_space(8.0);
}

fn render_setup_panel(
    ui: &mut egui::Ui,
    paths: &vpn_suite_core::app_paths::AppPaths,
    config: &ServerConfig,
    _notice: &mut Option<String>,
) {
    egui::Frame::group(ui.style())
        .fill(Color32::from_rgb(13, 13, 13))
        .stroke(Stroke::new(1.0, Color32::from_rgb(31, 31, 31)))
        .inner_margin(Margin::same(14.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Setup Health")
                        .font(FontId::new(18.0, FontFamily::Proportional))
                        .color(Color32::WHITE),
                );
                #[cfg(target_os = "linux")]
                {
                    if ui
                        .button(RichText::new("Apply Host Setup").color(Color32::BLACK))
                        .clicked()
                    {
                        let checks = vpn_platform_linux::apply_server_host_setup(config);
                        *_notice = Some(summarize_checks("Host setup apply", &checks));
                    }
                    if ui
                        .button(RichText::new("Remove Host Setup").color(Color32::WHITE))
                        .clicked()
                    {
                        let checks = vpn_platform_linux::remove_server_host_setup(config);
                        *_notice = Some(summarize_checks("Host setup remove", &checks));
                    }
                }
                #[cfg(target_os = "windows")]
                {
                    if ui
                        .button(RichText::new("Apply Host Setup").color(Color32::BLACK))
                        .clicked()
                    {
                        let checks = vpn_platform_windows::apply_server_host_setup(config);
                        *_notice = Some(summarize_checks("Host setup apply", &checks));
                    }
                    if ui
                        .button(RichText::new("Remove Host Setup").color(Color32::WHITE))
                        .clicked()
                    {
                        let checks = vpn_platform_windows::remove_server_host_setup(config);
                        *_notice = Some(summarize_checks("Host setup remove", &checks));
                    }
                }
            });
            ui.add_space(6.0);

            let report = server_setup_report(paths, config);
            for check in report.checks {
                setup_line(ui, &check);
            }

            #[cfg(target_os = "linux")]
            {
                for check in vpn_platform_linux::server_platform_checks(config) {
                    setup_line(ui, &check);
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new("Host setup CLI: sudo vpn-server host-setup apply")
                        .font(FontId::new(12.0, FontFamily::Monospace))
                        .color(Color32::from_rgb(170, 170, 170)),
                );
            }

            #[cfg(target_os = "windows")]
            {
                for check in vpn_platform_windows::wireguard_asset_checks() {
                    setup_line(ui, &check);
                }
                setup_line(ui, &vpn_platform_windows::server_tunnel_status());
                ui.add_space(6.0);
                ui.label(
                    RichText::new("Host setup CLI: vpn-server.exe host-setup apply")
                        .font(FontId::new(12.0, FontFamily::Monospace))
                        .color(Color32::from_rgb(170, 170, 170)),
                );
            }

            #[cfg(not(any(target_os = "linux", target_os = "windows")))]
            {
                ui.label(
                    RichText::new("Host setup automation is not available on this platform.")
                        .font(FontId::new(12.0, FontFamily::Monospace))
                        .color(Color32::from_rgb(170, 170, 170)),
                );
            }
        });
}

fn setup_line(ui: &mut egui::Ui, check: &SetupCheck) {
    let (label, color) = match check.status {
        SetupStatus::Pass => ("PASS", Color32::from_rgb(0, 255, 127)),
        SetupStatus::Warn => ("WARN", Color32::from_rgb(255, 184, 0)),
        SetupStatus::Fail => ("FAIL", Color32::from_rgb(255, 59, 59)),
    };
    ui.label(
        RichText::new(format!("[{label}] {} - {}", check.name, check.detail))
            .font(FontId::new(12.0, FontFamily::Monospace))
            .color(color),
    );
}

fn summarize_checks(label: &str, checks: &[SetupCheck]) -> String {
    let failed = checks
        .iter()
        .filter(|check| check.status == SetupStatus::Fail)
        .count();
    let warned = checks
        .iter()
        .filter(|check| check.status == SetupStatus::Warn)
        .count();
    if failed > 0 {
        format!("{label}: {failed} failed step(s), {warned} warning(s).")
    } else if warned > 0 {
        format!("{label}: completed with {warned} warning(s).")
    } else {
        format!("{label}: all steps passed.")
    }
}
