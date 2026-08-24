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
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Protocol:")
                .color(Color32::from_rgb(170, 170, 170))
                .font(FontId::new(12.0, FontFamily::Proportional)),
        );
        let label = selected.display_name();
        egui::ComboBox::from_id_salt("vpn_protocol_select")
            .selected_text(label)
            .width(width.clamp(120.0, 320.0))
            .show_ui(ui, |ui| {
                for p in VpnUiProtocol::ALL {
                    if ui
                        .selectable_label(*selected == p, p.display_name())
                        .clicked()
                    {
                        if *selected != p {
                            *selected = p;
                            changed = true;
                        }
                    }
                }
            });
    });
    if let Some(msg) = busy_other {
        ui.add_space(4.0);
        ui.label(
            RichText::new(msg)
                .color(WARN_AMBER)
                .font(FontId::new(11.0, FontFamily::Proportional)),
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
