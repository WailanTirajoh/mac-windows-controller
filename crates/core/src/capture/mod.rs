//! Capture layar. Tiap OS punya implementasi sendiri di balik API yang sama.

use crate::video::I420Frame;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;

#[cfg(not(target_os = "macos"))]
mod unsupported;
#[cfg(not(target_os = "macos"))]
use unsupported as platform;

/// Satu monitor yang bisa di-capture.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayInfo {
    pub id: u32,
    /// Posisi dan ukuran dalam koordinat global OS (point di macOS, piksel
    /// fisik di Windows). Dipakai untuk memetakan input ke monitor ini.
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Ukuran dalam piksel fisik.
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub is_primary: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct CaptureConfig {
    pub display_id: u32,
    /// Batas sisi panjang frame hasil capture, dalam piksel.
    pub max_long_edge: u32,
    pub max_fps: u32,
    pub show_cursor: bool,
}

/// Dipanggil dari thread capture untuk tiap frame yang isinya berubah.
pub type FrameSink = Box<dyn Fn(I420Frame) + Send + Sync + 'static>;

/// Capture yang sedang berjalan. Berhenti saat di-drop.
pub trait ScreenCapturer: Send {
    /// Ukuran frame yang dihasilkan, dalam piksel.
    fn frame_size(&self) -> (u32, u32);
}

pub fn list_displays() -> anyhow::Result<Vec<DisplayInfo>> {
    platform::list_displays()
}

pub fn start_capture(config: CaptureConfig, sink: FrameSink) -> anyhow::Result<Box<dyn ScreenCapturer>> {
    platform::start_capture(config, sink)
}
