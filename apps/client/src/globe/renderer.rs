use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use crate::globe::{CentroidTable, lat_lng_to_vec3, Vec3};
use vpn_suite_core::model::ServerSummary;

/// Tracks the connection-triggered animation state.
///
/// `country_code` and `server_id` are stored for diagnostic / debug
/// visibility and to keep the struct round-trippable, but the renderer
/// itself only animates from the rotation/timing fields. Mark them
/// `allow(dead_code)` so the storage doesn't generate a warning while we
/// still keep the fields available for future per-server visual tweaks.
#[allow(dead_code)]
struct ConnectionAnimation {
    /// Country code of the connected server (for centroid lookup).
    country_code: String,
    /// The server_id that triggered this animation.
    server_id: String,
    /// Monotonic time when the animation was triggered.
    start_time: f64,
    /// Target rotation_y to center the country.
    target_rot_y: f32,
    /// Target rotation_x to center the country.
    target_rot_x: f32,
    /// Starting rotation_y when animation began.
    start_rot_y: f32,
    /// Starting rotation_x when animation began.
    start_rot_x: f32,
    /// Starting zoom when animation began.
    start_zoom: f32,
    /// Target zoom that frames the country with neighbors still visible.
    target_zoom: f32,
    /// Whether the pan-to-country phase has finished.
    pan_done: bool,
    /// Whether the full connection has been confirmed (phase == Connected).
    connected: bool,
    /// Time when phase became Connected (for wave decay).
    connected_time: Option<f64>,
}

/// Snapshot of the active connection needed to render the globe beacon.
/// We accept this rather than the full `ActiveConnection` so the renderer
/// stays decoupled from the model crate's mutable fields.
#[derive(Clone, Debug, Default)]
pub struct ActiveBeacon {
    pub server_id: Option<String>,
    /// Exact lat/lon of the connection's exit node (for Tor, this is the
    /// tor_exit_info lat/lon; for a normal server, it falls back to the
    /// country's centroid).
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    /// ISO-3166 country code of the exit — used to fetch the centroid and
    /// the country flag SVG.
    pub country_code: Option<String>,
    /// For tooltip display only — we don't read the full struct here.
    pub display_name: Option<String>,
}

pub struct GlobeRenderer {
    borders: Vec<(Vec3, Vec3)>,
    pub centroids: CentroidTable,
    pub rotation_y: f32,
    pub rotation_x: f32,
    pub zoom: f32,
    pub is_dragging: bool,
    pub velocity_y: f32,
    pub velocity_x: f32,
    /// Active connection animation, if any.
    anim: Option<ConnectionAnimation>,
}

/// Vertical bias so the globe sits slightly below the geometric center.
/// Positive = down. Keep a modest offset so when zoomed, the bottom of the
/// globe clips first — the top never reaches the action buttons first.
/// Reduced from 0.10 after top-chrome clutter was removed.
const GLOBE_CENTER_Y_OFFSET_FRAC: f32 = 0.045;

/// Comfortable focus zoom: country fills the view while neighbors remain visible.
const COUNTRY_FOCUS_ZOOM: f32 = 2.55;

/// Compute globe `rotation_y` / `rotation_x` so the given lat/lng projects to
/// the center of the view (x_t=0, y_t=0, z_t>0) under our rotation matrix.
fn rotations_to_center(lat: f64, lng: f64) -> (f32, f32) {
    let p = lat_lng_to_vec3(lat, lng, 1.0);
    // Yaw: put the point in the YZ plane with positive Z (facing camera).
    // With x' = cos(ry)*px + sin(ry)*pz, set x'=0 ⇒ ry = atan2(-px, pz).
    let rot_y = (-p.x).atan2(p.z);

    let cy = rot_y.cos();
    let sy = rot_y.sin();
    // Point after yaw only (rot_x = 0 intermediate):
    let py2 = p.y;
    let pz2 = -sy * p.x + cy * p.z;
    // Pitch: zero y' with y' = cos(rx)*py2 - sin(rx)*pz2 ⇒ rx = atan2(py2, pz2).
    let rot_x = py2.atan2(pz2.max(1e-6));

    (rot_y, rot_x.clamp(-1.35, 1.35))
}

impl GlobeRenderer {
    pub fn new() -> Self {
        let borders = crate::globe::borders::build_border_lines(
            include_str!("../../assets/globe/countries_50m.geojson"),
            1.0
        );
        let centroids = CentroidTable::load();
        Self {
            borders,
            centroids,
            rotation_y: 0.0,
            rotation_x: 0.0,
            zoom: 1.0,
            is_dragging: false,
            velocity_y: 0.0,
            velocity_x: 0.0,
            anim: None,
        }
    }

    /// Called every frame when the user is not actively dragging.
    /// Applies exponential inertia so coasting feels smooth and stable
    /// across variable frame times (unlike linear `1 - k*dt` friction).
    pub fn update(&mut self, dt: f32) {
        let dt = dt.clamp(1.0 / 240.0, 1.0 / 20.0);
        if !self.is_dragging {
            self.rotation_y += self.velocity_y * dt;
            self.rotation_x += self.velocity_x * dt;

            // Exponential decay: ~half-life ≈ 0.18s at decay=4.
            let decay = (-4.2 * dt).exp();
            self.velocity_y *= decay;
            self.velocity_x *= decay;

            // Snap tiny residual velocity to zero to stop endless micro-repaints.
            if self.velocity_y.abs() < 0.02 {
                self.velocity_y = 0.0;
            }
            if self.velocity_x.abs() < 0.02 {
                self.velocity_x = 0.0;
            }
        }
        // Keep pitch in a comfortable range so the poles don't flip.
        self.rotation_x = self.rotation_x.clamp(-1.35, 1.35);
    }

    /// Apply a pointer/scroll delta with smoothed velocity for professional feel.
    /// `dx`/`dy` are rotation radians to apply this frame; `dt` is frame time.
    pub fn apply_orbit_delta(&mut self, dx: f32, dy: f32, dt: f32) {
        // User interaction cancels an in-progress camera tween so controls
        // never fight the connection pan animation.
        if let Some(anim) = &mut self.anim {
            if !anim.pan_done {
                anim.pan_done = true;
            }
        }

        self.rotation_y += dx;
        self.rotation_x = (self.rotation_x + dy).clamp(-1.35, 1.35);

        let dt = dt.max(1.0 / 240.0);
        let inst_vx = dx / dt;
        let inst_vy = dy / dt;
        // Blend instantaneous velocity for natural inertia on release.
        const BLEND: f32 = 0.35;
        self.velocity_y = self.velocity_y * (1.0 - BLEND) + inst_vx * BLEND;
        self.velocity_x = self.velocity_x * (1.0 - BLEND) + inst_vy * BLEND;
        // Cap so a fling never spins wildly.
        const MAX_VEL: f32 = 8.0;
        self.velocity_y = self.velocity_y.clamp(-MAX_VEL, MAX_VEL);
        self.velocity_x = self.velocity_x.clamp(-MAX_VEL, MAX_VEL);
    }

    /// Trigger a connection animation: smooth pan to the target country
    /// and begin wave/beacon effects.
    pub fn trigger_connection_anim(&mut self, server_id: &str, country_code: &str, time: f64) {
        if country_code.is_empty() {
            self.anim = None;
            return;
        }
        if let Some(c) = self.centroids.get(country_code) {
            self.start_pan_anim(server_id, country_code, c.lat, c.lng, time);
        }
    }

    /// Pan the globe to exact coordinates (Tor exit lat/lon).
    pub fn trigger_connection_anim_coords(&mut self, server_id: &str, lat: f64, lng: f64, time: f64) {
        self.start_pan_anim(server_id, "", lat, lng, time);
    }

    fn start_pan_anim(&mut self, server_id: &str, country_code: &str, lat: f64, lng: f64, time: f64) {
        // Precise camera so (lat,lng) lands at screen center of the globe.
        let (mut target_rot_y, target_rot_x) = rotations_to_center(lat, lng);

        // Take the shortest yaw path so the globe doesn't spin the long way.
        let start_rot_y = self.rotation_y;
        let mut dy = target_rot_y - start_rot_y;
        while dy > std::f32::consts::PI {
            target_rot_y -= std::f32::consts::TAU;
            dy = target_rot_y - start_rot_y;
        }
        while dy < -std::f32::consts::PI {
            target_rot_y += std::f32::consts::TAU;
            dy = target_rot_y - start_rot_y;
        }

        // Kill drag momentum so the pan isn't fighting inertia.
        self.velocity_x = 0.0;
        self.velocity_y = 0.0;

        // Slightly stronger zoom for a clear country focus while neighbors remain visible.
        let target_zoom = COUNTRY_FOCUS_ZOOM;

        self.anim = Some(ConnectionAnimation {
            country_code: country_code.to_owned(),
            server_id: server_id.to_owned(),
            start_time: time,
            target_rot_y,
            target_rot_x,
            start_rot_y,
            start_rot_x: self.rotation_x,
            start_zoom: self.zoom,
            target_zoom,
            pan_done: false,
            connected: false,
            connected_time: None,
        });
    }

    /// Mark the animation as "fully connected" so waves start decaying.
    pub fn mark_connected(&mut self, time: f64) {
        if let Some(anim) = &mut self.anim {
            if !anim.connected {
                anim.connected = true;
                anim.connected_time = Some(time);
            }
        }
    }

    /// Clear animation (on disconnect).
    pub fn clear_anim(&mut self) {
        self.anim = None;
    }

    /// Returns true if an animation is currently playing (pan or waves).
    pub fn is_animating(&self) -> bool {
        if self.velocity_y.abs() > 0.01 || self.velocity_x.abs() > 0.01 {
            return true;
        }
        if self.anim.is_some() {
            // Either still panning, ramping waves up, or running the beacon
            // pulse — keep redrawing until we explicitly disconnect.
            return true;
        }
        false
    }

    /// True when a connection-triggered pan/beacon anim is active.
    pub fn has_active_anim(&self) -> bool {
        self.anim.is_some()
    }

    /// Extra downward offset applied to the globe center inside `rect`.
    pub fn globe_center(rect: Rect) -> Pos2 {
        rect.center() + Vec2::new(0.0, rect.height() * GLOBE_CENTER_Y_OFFSET_FRAC)
    }

    pub fn paint(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        servers: &[ServerSummary],
        active: &ActiveBeacon,
    ) -> Option<String> {
        let mut clicked_server = None;
        let center = Self::globe_center(rect);
        let radius = rect.width().min(rect.height()) * 0.42 * self.zoom;
        let painter = ui.painter_at(rect);
        let time = ui.input(|i| i.time);

        // --- Smooth pan + zoom animation ---
        if let Some(anim) = &mut self.anim {
            if !anim.pan_done {
                let elapsed = (time - anim.start_time) as f32;
                // Slightly longer settle so zoom + tilt read clearly.
                let duration = 1.75_f32;
                let t = (elapsed / duration).clamp(0.0, 1.0);
                // Smooth ease-in-out cubic for a cinematic settle.
                let ease = if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                };
                // Zoom lags slightly behind pan so the country "lands" then fills.
                let zoom_t = ((t - 0.08) / 0.92).clamp(0.0, 1.0);
                let zoom_ease = if zoom_t < 0.5 {
                    4.0 * zoom_t * zoom_t * zoom_t
                } else {
                    1.0 - (-2.0 * zoom_t + 2.0).powi(3) / 2.0
                };

                self.rotation_y = anim.start_rot_y + (anim.target_rot_y - anim.start_rot_y) * ease;
                self.rotation_x = anim.start_rot_x + (anim.target_rot_x - anim.start_rot_x) * ease;
                self.zoom = anim.start_zoom + (anim.target_zoom - anim.start_zoom) * zoom_ease;
                self.rotation_x = self.rotation_x.clamp(-1.35, 1.35);

                if t >= 1.0 {
                    anim.pan_done = true;
                    self.rotation_y = anim.target_rot_y;
                    self.rotation_x = anim.target_rot_x.clamp(-1.35, 1.35);
                    self.zoom = anim.target_zoom;
                }
            }
        }

        // Draw the globe body
        painter.circle_filled(center, radius, Color32::from_rgb(18, 20, 22));
        painter.circle_stroke(center, radius, Stroke::new(1.2, Color32::from_rgb(0, 255, 127).linear_multiply(0.25)));
        // Soft outer halo
        painter.circle_stroke(
            center,
            radius + 3.0,
            Stroke::new(6.0, Color32::from_rgba_unmultiplied(0, 255, 127, 18)),
        );

        // Precompute rotation matrix
        let sy = self.rotation_y.sin();
        let cy = self.rotation_y.cos();
        let sx = self.rotation_x.sin();
        let cx = self.rotation_x.cos();

        let stroke = Stroke::new(1.0, Color32::from_rgb(0, 255, 127).linear_multiply(0.55));

        let mut lines = Vec::with_capacity(self.borders.len());

        let m00 = cy;
        let m01 = 0.0_f32;
        let m02 = sy;

        let m10 = sy * sx;
        let m11 = cx;
        let m12 = -cy * sx;

        let m20 = -sy * cx;
        let m21 = sx;
        let m22 = cy * cx;

        for (p1, p2) in &self.borders {
            let y1_t = p1.x * m10 + p1.y * m11 + p1.z * m12;
            let z1_t = p1.x * m20 + p1.y * m21 + p1.z * m22;

            let y2_t = p2.x * m10 + p2.y * m11 + p2.z * m12;
            let z2_t = p2.x * m20 + p2.y * m21 + p2.z * m22;

            if z1_t < 0.0 && z2_t < 0.0 { continue; }

            let x1_t = p1.x * m00 + p1.y * m01 + p1.z * m02;
            let x2_t = p2.x * m00 + p2.y * m01 + p2.z * m02;

            let screen1 = center + Vec2::new(x1_t, -y1_t) * radius;
            let screen2 = center + Vec2::new(x2_t, -y2_t) * radius;

            let max_z = z1_t.max(z2_t);
            if max_z > -0.1 {
                lines.push(egui::Shape::line_segment([screen1, screen2], stroke));
            }
        }

        // --- Draw server nodes ---
        let pointer_pos = ui.input(|i| i.pointer.hover_pos());
        let pointer_clicked = ui.input(|i| i.pointer.any_click());

        // Determine whether the active beacon is for a server that's in the
        // servers list. If it's NOT in there (i.e. Tor / ovpn / any future
        // non-server transport) we render it as a freestanding beacon.
        let active_server_id_str = active.server_id.as_deref();
        let active_in_servers = active_server_id_str
            .map(|id| servers.iter().any(|s| s.server_id == id))
            .unwrap_or(false);

        for server in servers {
            if let Some(c) = self.centroids.get(&server.country_code) {
                let pos = lat_lng_to_vec3(c.lat, c.lng, 1.01);

                let y_t = pos.x * m10 + pos.y * m11 + pos.z * m12;
                let z_t = pos.x * m20 + pos.y * m21 + pos.z * m22;

                if z_t >= 0.0 {
                    let x_t = pos.x * m00 + pos.y * m01 + pos.z * m02;
                    let screen = center + Vec2::new(x_t, -y_t) * radius;

                    let is_active = active_server_id_str == Some(server.server_id.as_str());

                    if is_active {
                        self.paint_beacon(&painter, screen, time);
                    } else {
                        // Normal server dots — neon green / soft grey
                        let (size, color) = if server.has_password {
                            (4.0, Color32::from_rgb(200, 200, 200))
                        } else {
                            (3.0, Color32::from_rgb(0, 255, 127))
                        };
                        painter.circle_filled(screen, size * 2.5, color.linear_multiply(0.3));
                        painter.circle_filled(screen, size, color);
                    }

                    // Manual hover/click detection (avoids ui.interact which causes egui hit_test panic)
                    let hover_radius = if is_active { 28.0 } else { 12.0 };
                    let is_hovered = pointer_pos
                        .map(|pp| (pp - screen).length() <= hover_radius)
                        .unwrap_or(false);

                    if is_hovered {
                        let tooltip_id = egui::Id::new("server_tooltip").with(&server.server_id);
                        egui::show_tooltip_at_pointer(ui.ctx(), egui::LayerId::new(egui::Order::Tooltip, tooltip_id), tooltip_id, |ui| {
                            ui.horizontal(|ui| {
                                crate::app::show_flag(ui, &server.country_code, Vec2::new(36.0, 24.0));
                                ui.vertical(|ui| {
                                    ui.label(egui::RichText::new(&server.name).strong().color(Color32::WHITE));
                                    ui.label(
                                        egui::RichText::new(format!("Country: {}", server.country_name))
                                            .color(Color32::from_rgb(200, 200, 200)),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!("Location: {:.4}, {:.4}", c.lat, c.lng))
                                            .color(Color32::from_rgb(160, 160, 160)),
                                    );
                                    if is_active {
                                        ui.label(
                                            egui::RichText::new("CONNECTED")
                                                .color(Color32::from_rgb(0, 255, 127))
                                                .strong(),
                                        );
                                    } else if server.has_password {
                                        ui.label(
                                            egui::RichText::new("Requires Password")
                                                .color(Color32::from_rgb(200, 200, 200)),
                                        );
                                    }
                                });
                            });
                        });

                        if pointer_clicked {
                            clicked_server = Some(server.server_id.clone());
                        }
                    }
                }
            }
        }

        // --- Freestanding beacon for active connections that are NOT in
        // the servers list (Tor transport, imported OpenVPN configs, etc).
        if !active_in_servers {
            if let Some(beacon) = compute_freestanding_beacon(active, &self.centroids) {
                let pos = lat_lng_to_vec3(beacon.lat, beacon.lon, 1.01);
                let y_t = pos.x * m10 + pos.y * m11 + pos.z * m12;
                let z_t = pos.x * m20 + pos.y * m21 + pos.z * m22;

                if z_t >= 0.0 {
                    let x_t = pos.x * m00 + pos.y * m01 + pos.z * m02;
                    let screen = center + Vec2::new(x_t, -y_t) * radius;

                    self.paint_beacon(&painter, screen, time);

                    // Tooltip: country flag + display name + coordinates
                    let hover_radius = 28.0_f32;
                    let is_hovered = pointer_pos
                        .map(|pp| (pp - screen).length() <= hover_radius)
                        .unwrap_or(false);
                    if is_hovered {
                        let tooltip_id = egui::Id::new("tor_beacon_tooltip");
                        egui::show_tooltip_at_pointer(ui.ctx(), egui::LayerId::new(egui::Order::Tooltip, tooltip_id), tooltip_id, |ui| {
                            ui.horizontal(|ui| {
                                if let Some(cc) = beacon.country_code.as_deref() {
                                    crate::app::show_flag(ui, cc, Vec2::new(36.0, 24.0));
                                }
                                ui.vertical(|ui| {
                                    ui.label(
                                        egui::RichText::new(beacon.display_name.as_deref().unwrap_or("Tor Exit"))
                                            .strong()
                                            .color(Color32::WHITE),
                                    );
                                    if let Some(cc) = beacon.country_code.as_deref() {
                                        ui.label(
                                            egui::RichText::new(format!("Country: {cc}"))
                                                .color(Color32::from_rgb(200, 200, 200)),
                                        );
                                    }
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "Location: {:.4}, {:.4}",
                                            beacon.lat, beacon.lon
                                        ))
                                        .color(Color32::from_rgb(160, 160, 160)),
                                    );
                                    let tag = if active.server_id.as_deref() == Some("local_ip") {
                                        ("YOUR PUBLIC IP", Color32::from_rgb(0, 255, 127))
                                    } else if active.server_id.as_deref() == Some("tor_local") {
                                        ("CONNECTED VIA TOR", Color32::from_rgb(168, 85, 247))
                                    } else {
                                        ("CONNECTED", Color32::from_rgb(0, 255, 127))
                                    };
                                    ui.label(
                                        egui::RichText::new(tag.0)
                                            .color(tag.1)
                                            .strong(),
                                    );
                                });
                            });
                        });
                    }
                }
            }
        }

        // --- Location status label when we have an active connection but no pin yet ---
        let freestanding_missing = !active_in_servers
            && active_server_id_str.is_some()
            && active.country_code.is_none()
            && active.lat.is_none();
        if freestanding_missing {
            let label_pos = Pos2::new(center.x, center.y + radius + 18.0);
            painter.text(
                label_pos,
                egui::Align2::CENTER_TOP,
                "Detecting exit location…",
                egui::FontId::new(14.0, egui::FontFamily::Proportional),
                Color32::from_rgb(180, 180, 180),
            );
        } else if let Some(active_id) = active_server_id_str {
            if let Some(server) = servers.iter().find(|s| s.server_id == active_id) {
                if server.country_code.is_empty() || self.centroids.get(&server.country_code).is_none() {
                    let label_pos = Pos2::new(center.x, center.y + radius + 18.0);
                    painter.text(
                        label_pos,
                        egui::Align2::CENTER_TOP,
                        "No country detected",
                        egui::FontId::new(14.0, egui::FontFamily::Proportional),
                        Color32::from_rgb(180, 180, 180),
                    );
                }
            }
        }

        painter.extend(lines);
        clicked_server
    }

    /// Calm green beacon: solid neon-green core + soft border-only ripples.
    /// No filled glow discs or red hues — only stroke rings that fade out.
    fn paint_beacon(&self, painter: &egui::Painter, screen: Pos2, time: f64) {
        let anim_elapsed = if let Some(anim) = &self.anim {
            (time - anim.start_time) as f32
        } else {
            time as f32
        };

        // Soft intensity ramp in the first half-second of a connection pan.
        let intensity = if self.anim.is_some() {
            (anim_elapsed / 0.5).clamp(0.45, 1.0)
        } else {
            1.0
        };

        // --- Small, calm border-only ripples ---
        let wave_count = 2;
        let wave_period = 2.4_f32; // slow
        let max_wave_radius = 26.0;
        let core_r = 4.5;

        for i in 0..wave_count {
            let phase = ((anim_elapsed / wave_period) + (i as f32) * 0.5) % 1.0;
            let wave_radius = core_r + phase * (max_wave_radius - core_r);
            // Fade as the ring expands; keep alpha modest.
            let wave_alpha = (1.0 - phase) * intensity * 0.55;
            if wave_alpha < 0.04 {
                continue;
            }
            let alpha = (wave_alpha * 255.0) as u8;
            // Border-only stroke — pure neon green, no fill.
            painter.circle_stroke(
                screen,
                wave_radius,
                Stroke::new(
                    1.35,
                    Color32::from_rgba_unmultiplied(0, 255, 127, alpha),
                ),
            );
        }

        // --- Compact green core (no large colored haze) ---
        let pulse = 0.5 + 0.5 * (anim_elapsed * 1.6).sin();
        let beacon_size = 4.2 + pulse * 0.55;

        // Very subtle outer halo (tiny, low alpha — not a red/green bloom)
        painter.circle_filled(
            screen,
            beacon_size * 1.85,
            Color32::from_rgba_unmultiplied(0, 255, 127, ((0.10 + pulse * 0.06) * 255.0) as u8),
        );
        // Solid neon-green dot
        painter.circle_filled(screen, beacon_size, Color32::from_rgb(0, 255, 127));
        // Tiny bright center for crispness
        painter.circle_filled(
            screen,
            beacon_size * 0.38,
            Color32::from_rgb(220, 255, 235),
        );
    }
}

/// Resolves where the freestanding beacon (used for Tor and other
/// transports that aren't in the servers list) should be drawn. We prefer
/// the exact lat/lon of the exit node when GeoIP provided it; otherwise we
/// fall back to the centroid of the country. Returns `None` when neither is
/// available — in that case the globe just shows a "Detecting exit
/// location…" hint instead of a beacon.
fn compute_freestanding_beacon(active: &ActiveBeacon, centroids: &CentroidTable) -> Option<FreestandingBeacon> {
    if let (Some(lat), Some(lon)) = (active.lat, active.lon) {
        // GeoIP gave us exact coordinates — use those for the most precise
        // pin. The country code may be missing for a brief moment, so we
        // only attach a flag when we actually have one.
        if lat.abs() <= 90.0 && lon.abs() <= 180.0 {
            return Some(FreestandingBeacon {
                lat,
                lon,
                country_code: active.country_code.clone(),
                display_name: active.display_name.clone().or_else(|| {
                    active
                        .country_code
                        .as_deref()
                        .and_then(|cc| centroids.get(cc).map(|c| format!("Tor Exit: {}", c.name)))
                }),
            });
        }
    }

    if let Some(cc) = active.country_code.as_deref() {
        if let Some(centroid) = centroids.get(cc) {
            return Some(FreestandingBeacon {
                lat: centroid.lat,
                lon: centroid.lng,
                country_code: Some(cc.to_owned()),
                display_name: active
                    .display_name
                    .clone()
                    .or_else(|| Some(format!("Tor Exit: {}", centroid.name))),
            });
        }
    }
    None
}

#[derive(Clone, Debug)]
struct FreestandingBeacon {
    lat: f64,
    lon: f64,
    country_code: Option<String>,
    display_name: Option<String>,
}
