//! Shared connect/disconnect progress for the Android UI.

use crate::TunnelProgress;
use std::sync::{Mutex, OnceLock};

static PROGRESS: OnceLock<Mutex<TunnelProgress>> = OnceLock::new();

fn slot() -> &'static Mutex<TunnelProgress> {
    PROGRESS.get_or_init(|| Mutex::new(TunnelProgress::default()))
}

pub fn set_progress(stage: impl Into<String>, fraction: f32, detail: impl Into<String>) {
    if let Ok(mut g) = slot().lock() {
        g.stage = stage.into();
        g.fraction = fraction.clamp(0.0, 1.0);
        g.detail = detail.into();
    }
}

pub fn get_progress() -> TunnelProgress {
    slot()
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default()
}

pub fn clear_progress() {
    if let Ok(mut g) = slot().lock() {
        *g = TunnelProgress::default();
    }
}
