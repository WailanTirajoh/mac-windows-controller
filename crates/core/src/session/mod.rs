//! Sesi remote desktop: sisi tujuan (`host`) dan sisi pengendali (`controller`).

pub mod controller;
pub mod host;

use std::sync::Arc;

use crate::capture::{CaptureBackend, SystemCapture};
use crate::input::{InputBackend, SystemInput};

/// Layanan OS yang dipakai perangkat saat dikendalikan. Test menggantinya
/// dengan tiruan.
#[derive(Clone)]
pub struct SessionServices {
    pub capture: Arc<dyn CaptureBackend>,
    pub input: Arc<dyn InputBackend>,
}

impl Default for SessionServices {
    fn default() -> Self {
        Self {
            capture: Arc::new(SystemCapture),
            input: Arc::new(SystemInput),
        }
    }
}
