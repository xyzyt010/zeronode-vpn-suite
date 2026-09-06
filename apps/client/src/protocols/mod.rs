//! Multi-protocol VPN UI chrome (OpenVPN / WireGuard / PPTP / Outline).
//!
//! Panel bodies still live on `VpnClientApp` (they need app state + commands);
//! this module holds shared styling helpers and metadata parsers.

use eframe::egui::{self, Align2, Color32, FontFamily, FontId, RichText, Stroke, Vec2};
use vpn_suite_core::model::VpnUiProtocol;

pub const VPN_GREEN: Color32 = Color32::from_rgb(0, 255, 127);
pub const VPN_GREEN_DIM: Color32 = Color32::from_rgb(0, 180, 120);
pub const VPN_CARD_BG: Color32 = Color32::from_rgb(13, 13, 13);
pub const WARN_AMBER: Color32 = Color32::from_rgb(255, 180, 60);
/// Pure-black dropdown surface (button + open menu).
pub const COMBO_BLACK: Color32 = Color32::from_rgb(0, 0, 0);
/// Hairline outline that gives the closed dropdown its structure.
pub const COMBO_OUTLINE: Color32 = Color32::from_rgb(95, 95, 95);

/// Chevron glyph painter for `ComboBox::icon()`: a wide ∨ when closed that
/// flips to ∧ while the menu is open. Same stance as the section headers,
/// drawn in the button's foreground color so it stays white-on-black.
pub fn combo_chevron_icon(
    ui: &egui::Ui,
    rect: egui::Rect,
    visuals: &egui::style::WidgetVisuals,
    is_open: bool,
    _above_or_below: egui::AboveOrBelow,
) {
    let c = rect.center();
    // Wide stance proportional to the icon box (~55° half-angle).
    let dx = rect.width() * 0.17;
    let dy = rect.width() * 0.27;
    let (a, b, d) = if is_open {
        // ∧ open
        (
            egui::pos2(c.x - dy, c.y + dx),
            egui::pos2(c.x, c.y - dx),
            egui::pos2(c.x + dy, c.y + dx),
        )
    } else {
        // ∨ closed
        (
            egui::pos2(c.x - dy, c.y - dx),
            egui::pos2(c.x, c.y + dx),
            egui::pos2(c.x + dy, c.y - dx),
        )
    };
    let stroke = Stroke::new(2.2, visuals.fg_stroke.color);
    let p = ui.painter();
    p.line_segment([a, b], stroke);
    p.line_segment([b, d], stroke);
}

/// Black dropdown shell: pure-black button with a light outline that turns
/// green on hover/open, white text throughout, chevron arrow instead of the
/// default triangle, and a black popup with a green-tinted outline.
/// Menu ROWS must use [`menu_item`] (explicit green/black hover paint) —
/// scoped `hovered` visuals stay black-on-purpose so the button itself never
/// flashes green.
pub fn black_combo<R>(
    ui: &mut egui::Ui,
    id_salt: &str,
    selected_text: &str,
    width: f32,
    menu: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    ui.scope(|ui| {
        let v = ui.visuals_mut();
        for w in [
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.bg_fill = COMBO_BLACK;
            w.weak_bg_fill = COMBO_BLACK;
            w.fg_stroke = Stroke::new(1.0, Color32::WHITE);
            w.rounding = 6.0.into();
        }
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, COMBO_OUTLINE);
        v.widgets.hovered.bg_stroke = Stroke::new(1.5, VPN_GREEN);
        v.widgets.active.bg_stroke = Stroke::new(1.5, VPN_GREEN);
        v.widgets.open.bg_stroke = Stroke::new(1.5, VPN_GREEN);
        // Open menu body: black with a green-tinted structural outline.
        v.window_fill = COMBO_BLACK;
        v.window_stroke = Stroke::new(1.0, VPN_GREEN_DIM);
        v.menu_rounding = 6.0.into();
        // Selected-row base (mirrors menu_item's explicit paint).
        v.selection.bg_fill = VPN_GREEN;
        v.selection.stroke = Stroke::new(1.0, Color32::BLACK);
        egui::ComboBox::from_id_salt(id_salt)
            .selected_text(RichText::new(selected_text).color(Color32::WHITE))
            .width(width)
            .icon(combo_chevron_icon)
            .show_ui(ui, menu)
    })
    .inner
}

/// One dropdown menu row: black with white text at rest; hovering (or being
/// the current selection) floods the row green and flips the text to black —
/// overpainted in the same frame, so there is zero hover lag.
pub fn menu_item(ui: &mut egui::Ui, selected: bool, label: &str) -> egui::Response {
    let h = 26.0;
    let resp = ui.add(
        egui::Button::new(
            RichText::new(label)
                .font(FontId::new(13.0, FontFamily::Proportional))
                .color(Color32::WHITE),
        )
        .fill(COMBO_BLACK)
        .stroke(Stroke::NONE)
        .rounding(4.0)
        .min_size(Vec2::new(ui.available_width(), h)),
    );
    if selected || resp.hovered() {
        let p = ui.painter();
        p.rect_filled(resp.rect, 4.0, VPN_GREEN);
        p.text(
            resp.rect.center(),
            Align2::CENTER_CENTER,
            label,
            FontId::new(13.0, FontFamily::Proportional),
            Color32::BLACK,
        );
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// Animated on/off pill toggle: the knob slides and the track cross-fades
/// grey → green via `animate_bool` (smooth color + motion, auto-repainted).
/// Returns the response; it is marked `changed()` exactly when clicked.
pub fn animated_toggle(ui: &mut egui::Ui, id_salt: &str, on: &mut bool) -> egui::Response {
    let w = 44.0;
    let h = 24.0;
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(w, h), egui::Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let t = ui.ctx().animate_bool(ui.make_persistent_id(id_salt), *on);
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        // Track: dark grey → VPN green.
        let bg = Color32::from_rgb(
            (70.0 * (1.0 - t)) as u8,
            (70.0 * (1.0 - t) + 255.0 * t) as u8,
            (70.0 * (1.0 - t) + 127.0 * t) as u8,
        );
        p.rect_filled(rect, h / 2.0, bg);
        // Knob: white circle sliding pad → pad.
        let pad = 3.0;
        let kr = (h - pad * 2.0) / 2.0;
        let kx = rect.left() + pad + kr + t * (w - 2.0 * (pad + kr));
        p.circle_filled(egui::pos2(kx, rect.center().y), kr, Color32::WHITE);
    }
    resp
}

pub fn protocol_combo(
    ui: &mut egui::Ui,
    selected: &mut VpnUiProtocol,
    width: f32,
    busy_other: Option<&str>,
) -> bool {
    let mut changed = false;
    // Narrow side panels (<240px): stack label above the combo so the black
    // dropdown never squeezes to an unreadable width.
    let avail = ui.available_width();
    let stack = avail < 240.0;
    let combo_w = if stack {
        (avail - 4.0).max(80.0)
    } else {
        width.clamp(80.0, (avail - 72.0).max(80.0)).clamp(80.0, 320.0)
    };
    let mut combo = |ui: &mut egui::Ui| {
        let label = selected.display_name();
        black_combo(ui, "vpn_protocol_select", label, combo_w, |ui| {
            ui.set_min_width(combo_w.min(200.0));
            for p in VpnUiProtocol::ALL {
                if menu_item(ui, *selected == p, p.display_name()).clicked() {
                    if *selected != p {
                        *selected = p;
                        changed = true;
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
        combo(ui);
    } else {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Protocol:")
                    .color(Color32::from_rgb(170, 170, 170))
                    .font(FontId::new(12.0, FontFamily::Proportional)),
            );
            combo(ui);
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
