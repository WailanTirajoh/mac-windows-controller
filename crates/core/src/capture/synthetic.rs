//! Capture tiruan: "layar" berisi dokumen sintetis yang terus di-scroll.
//!
//! Dipakai uji mandiri sesi, yang harus bisa berjalan tanpa izin Screen
//! Recording dan tanpa layar sungguhan.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;

use super::{CaptureBackend, CaptureConfig, DisplayInfo, FrameSink, ScreenCapturer};
use crate::video::fit_even;
use crate::video::testsrc::scrolling_document;

pub struct SyntheticCapture {
    displays: Vec<DisplayInfo>,
}

impl SyntheticCapture {
    /// Dua layar bersebelahan dengan ukuran berbeda, supaya pindah layar terlihat.
    pub fn two_displays() -> Self {
        let display = |id, x: f64, width: u32, height: u32| DisplayInfo {
            id,
            x,
            y: 0.0,
            width: f64::from(width),
            height: f64::from(height),
            pixel_width: width,
            pixel_height: height,
            is_primary: id == 1,
        };
        Self {
            displays: vec![display(1, 0.0, 1280, 720), display(2, 1280.0, 800, 600)],
        }
    }
}

struct SyntheticCapturer {
    frame_size: (u32, u32),
    stop: Arc<AtomicBool>,
}

impl ScreenCapturer for SyntheticCapturer {
    fn frame_size(&self) -> (u32, u32) {
        self.frame_size
    }
}

impl Drop for SyntheticCapturer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl CaptureBackend for SyntheticCapture {
    fn displays(&self) -> anyhow::Result<Vec<DisplayInfo>> {
        Ok(self.displays.clone())
    }

    fn start(&self, config: CaptureConfig, sink: FrameSink) -> anyhow::Result<Box<dyn ScreenCapturer>> {
        let display = self
            .displays
            .iter()
            .find(|display| display.id == config.display_id)
            .with_context(|| format!("layar {} tidak ditemukan", config.display_id))?;
        let frame_size = fit_even(display.pixel_width, display.pixel_height, config.max_long_edge);
        let interval = Duration::from_secs(1) / config.max_fps.max(1);
        let stop = Arc::new(AtomicBool::new(false));

        let stopped = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut index = 0;
            while !stopped.load(Ordering::Relaxed) {
                sink(scrolling_document(frame_size.0, frame_size.1, index));
                index += 1;
                std::thread::sleep(interval);
            }
        });

        Ok(Box::new(SyntheticCapturer { frame_size, stop }))
    }
}
