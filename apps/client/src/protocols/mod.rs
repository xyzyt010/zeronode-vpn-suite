//! Multi-protocol VPN UI chrome (OpenVPN / WireGuard / PPTP / Outline).
//!
//! Panel bodies still live on `VpnClientApp` (they need app state + commands);
//! this module holds shared styling helpers and metadata parsers.

use eframe::egui::{self, Color32, FontFamily, FontId, RichText};
use vpn_suite_core::model::VpnUiProtocol;

pub const VPN_GREEN: Color32 = Color32::from_rgb(0, 255, 127);
pub const VPN_GREEN_DIM: Color32 = Color32::from_rgb(0, 180, 120);
pub const VPN_CARD_BG: Color32 = Color32::from_rgb(13, 13, 13);
pub const WARN_AMBER: Color32 = Color32::from_rgb(255, 180, 60);

pub fn protocol_combo(
    ui: &mut egui::Ui,
    selected: &mut VpnUiProtocol,
    width: f32,
    busy_other: Option<&str>,
) -> bool {
    let mut changed = false;
    let avail = ui.available_width();
    let stack = avail < 240.0;
    let combo_w = if stack {
        (avail - 4.0).max(80.0)
    } else {
        width.clamp(80.0, (avail - 72.0).max(80.0))
    };
    let combo = |ui: &mut egui::Ui, selected: &mut VpnUiProtocol, changed: &mut bool| {
        let label = selected.display_name();
        egui::ComboBox::from_id_salt("vpn_protocol_select")
            .selected_text(label)
            .width(combo_w)
            .show_ui(ui, |ui| {
                ui.set_min_width(combo_w.min(200.0));
                for p in VpnUiProtocol::ALL {
                    if ui
                        .selectable_label(*selected == p, p.display_name())
                        .clicked()
                    {
                        if *selected != p {
                            *selected = p;
                            *changed = true;
                        }
                    }
                }
            });
    };
    if stack {
        ui.label(
            RichText::new("Protocol")
                .color(Color32::from_rgb(170, 170, 170))
                .font(FontId::new(11.0, FontFamily::Proportional)),
        );
        combo(ui, selected, &mut changed);
    } else {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Protocol:")
                    .color(Color32::from_rgb(170, 170, 170))
                    .font(FontId::new(12.0, FontFamily::Proportional)),
            );
            combo(ui, selected, &mut changed);
        });
    }
    if let Some(msg) = busy_other {
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                RichText::new(msg)
                    .color(WARN_AMBER)
                    .font(FontId::new(11.0, FontFamily::Proportional)),
            )
            .wrap(),
        );
    }
    changed
}

/// Extract WireGuard display fields from a `.conf` without starting a tunnel.
pub fn parse_wg_summary(content: &str) -> WgSummary {
    let mut endpoint = None;
    let mut public_key = None;
    let mut address = None;
    let mut in_interface = false;
    let mut in_peer = false;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.eq_ignore_ascii_case("[Interface]") {
            in_interface = true;
            in_peer = false;
            continue;
        }
        if line.eq_ignore_ascii_case("[Peer]") {
            in_interface = false;
            in_peer = true;
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let k = k.trim();
            let v = v.trim();
            match (k, in_interface, in_peer) {
                ("Address", true, false) => {
                    address = v.split(',').next().map(|s| s.trim().to_string());
                }
                ("PublicKey", false, true) => public_key = Some(v.to_string()),
                ("Endpoint", false, true) => endpoint = Some(v.to_string()),
                _ => {}
            }
        }
    }
    WgSummary {
        endpoint,
        public_key,
        address,
    }
}

#[derive(Clone, Debug, Default)]
pub struct WgSummary {
    pub endpoint: Option<String>,
    pub public_key: Option<String>,
    pub address: Option<String>,
}

/// Host portion of `host:port` or `[ipv6]:port`.
pub fn host_from_endpoint(endpoint: &str) -> String {
    let e = endpoint.trim();
    if let Some(rest) = e.strip_prefix('[') {
        return rest
            .split_once(']')
            .map(|(h, _)| h.to_string())
            .unwrap_or_else(|| e.to_string());
    }
    e.rsplit_once(':')
        .map(|(h, _)| h.to_string())
        .unwrap_or_else(|| e.to_string())
}
