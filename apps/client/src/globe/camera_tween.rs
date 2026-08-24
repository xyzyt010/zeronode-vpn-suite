use crate::globe::Vec3;

pub struct CameraTween {
    start: Vec3,
    end: Vec3,
    elapsed: f32,
    duration: f32,
    active: bool,
}

impl CameraTween {
    pub fn start(current: Vec3, target: Vec3, duration_secs: f32) -> Self {
        Self { start: current, end: target, elapsed: 0.0, duration: duration_secs, active: true }
    }

    pub fn tick(&mut self, dt: f32) -> Option<Vec3> {
        if !self.active { return None; }
        self.elapsed += dt;
        let t = (self.elapsed / self.duration).clamp(0.0, 1.0);
        let eased = ease_in_out_cubic(t);
        let pos = self.start + (self.end - self.start) * eased;
        if t >= 1.0 { self.active = false; }
        Some(pos)
    }
    
    pub fn is_active(&self) -> bool {
        self.active
    }
}

fn ease_in_out_cubic(t: f32) -> f32 {
    if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 }
}
