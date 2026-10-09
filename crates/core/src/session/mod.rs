//! Sesi remote desktop: sisi tujuan (`host`) dan sisi pengendali (`controller`).

pub mod controller;
pub mod host;

use std::sync::Arc;

use crate::capture::{CaptureBackend, SystemCapture};
use crate::clipboard::{ClipboardBackend, SystemClipboard};
use crate::input::{InputBackend, SystemInput};

/// Layanan OS yang dipakai selama sesi. Capture dan input hanya dipakai saat
/// perangkat ini dikendalikan; clipboard dipakai di kedua peran. Test
/// menggantinya dengan tiruan.
#[derive(Clone)]
pub struct SessionServices {
    pub capture: Arc<dyn CaptureBackend>,
    pub input: Arc<dyn InputBackend>,
    pub clipboard: Arc<dyn ClipboardBackend>,
}

impl Default for SessionServices {
    fn default() -> Self {
        Self {
            capture: Arc::new(SystemCapture),
            input: Arc::new(SystemInput),
            clipboard: Arc::new(SystemClipboard),
        }
    }
}
