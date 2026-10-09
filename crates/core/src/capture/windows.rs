//! Capture layar Windows lewat Windows.Graphics.Capture.

use std::ffi::c_void;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context as _};
use windows_capture::capture::{CaptureControl, Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::{GraphicsCaptureApi, InternalCaptureControl};
use windows_capture::monitor::Monitor;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings, MinimumUpdateIntervalSettings,
    SecondaryWindowSettings, Settings,
};
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITORINFO};

use super::{CaptureConfig, DisplayInfo, FrameSink, ScreenCapturer};
use crate::video::convert::BgraConverter;
use crate::video::fit_even;

type HandlerError = Box<dyn std::error::Error + Send + Sync>;

/// Posisi dan ukuran monitor di desktop virtual, dalam piksel fisik.
fn bounds(monitor: *mut c_void) -> Option<(i32, i32, i32, i32)> {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        // SAFETY: MONITORINFO hanya berisi bilangan, jadi nol adalah nilai yang valid.
        ..unsafe { std::mem::zeroed() }
    };
    // SAFETY: `info` valid dan `cbSize`-nya sudah diisi; handle yang tidak valid membuat fungsi gagal.
    let found = unsafe { GetMonitorInfoW(monitor, &mut info) } != 0;
    let area = info.rcMonitor;
    found.then_some((area.left, area.top, area.right - area.left, area.bottom - area.top))
}

fn display_info(index: usize, monitor: &Monitor, primary: Option<*mut c_void>) -> anyhow::Result<DisplayInfo> {
    let handle = monitor.as_raw_hmonitor();
    let (x, y, width, height) = bounds(handle).context("ukuran monitor tidak terbaca")?;
    Ok(DisplayInfo {
        // ID adalah urutan monitor, mulai dari 1.
        id: index as u32 + 1,
        x: f64::from(x),
        y: f64::from(y),
        width: f64::from(width),
        height: f64::from(height),
        pixel_width: width as u32,
        pixel_height: height as u32,
        is_primary: primary == Some(handle),
    })
}

fn monitors() -> anyhow::Result<Vec<Monitor>> {
    Monitor::enumerate().map_err(|error| anyhow!("daftar monitor tidak terbaca: {error}"))
}

pub fn list_displays() -> anyhow::Result<Vec<DisplayInfo>> {
    let primary = Monitor::primary().ok().map(|monitor| monitor.as_raw_hmonitor());
    monitors()?
        .iter()
        .enumerate()
        .map(|(index, monitor)| display_info(index, monitor, primary))
        .collect()
}

/// Yang dibutuhkan handler capture, diteruskan lewat pengaturan capture.
struct HandlerSetup {
    sink: FrameSink,
    max_long_edge: u32,
}

struct Handler {
    sink: FrameSink,
    max_long_edge: u32,
    started: Instant,
    converter: BgraConverter,
}

impl GraphicsCaptureApiHandler for Handler {
    type Flags = HandlerSetup;
    type Error = HandlerError;

    fn new(context: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self {
            sink: context.flags.sink,
            max_long_edge: context.flags.max_long_edge,
            started: Instant::now(),
            converter: BgraConverter::default(),
        })
    }

    fn on_frame_arrived(&mut self, frame: &mut Frame, _control: InternalCaptureControl) -> Result<(), Self::Error> {
        let timestamp_us = self.started.elapsed().as_micros() as u64;
        // Ukuran dibaca per frame: resolusi monitor bisa berubah di tengah sesi.
        let size = (frame.width(), frame.height());
        let target = fit_even(size.0, size.1, self.max_long_edge);

        let mut buffer = frame.buffer()?;
        let stride = buffer.row_pitch() as usize;
        let converted = self
            .converter
            .convert(buffer.as_raw_buffer(), stride, size, target, timestamp_us)?;
        (self.sink)(converted);
        Ok(())
    }
}

struct WindowsCapturer {
    control: Option<CaptureControl<Handler, HandlerError>>,
    frame_size: (u32, u32),
}

impl ScreenCapturer for WindowsCapturer {
    fn frame_size(&self) -> (u32, u32) {
        self.frame_size
    }
}

impl Drop for WindowsCapturer {
    fn drop(&mut self) {
        if let Some(control) = self.control.take() {
            let _ = control.stop();
        }
    }
}

pub fn start_capture(config: CaptureConfig, sink: FrameSink) -> anyhow::Result<Box<dyn ScreenCapturer>> {
    let monitor = monitors()?
        .into_iter()
        .nth((config.display_id as usize).wrapping_sub(1))
        .with_context(|| format!("layar {} tidak ditemukan", config.display_id))?;
    let (_, _, width, height) = bounds(monitor.as_raw_hmonitor()).context("ukuran monitor tidak terbaca")?;
    let frame_size = fit_even(width as u32, height as u32, config.max_long_edge);

    // Pengaturan di luar bawaan hanya dipakai kalau versi Windows-nya mendukung;
    // kalau tidak, capture menolak dimulai.
    let supported = |check: Result<bool, _>| check.unwrap_or(false);
    let cursor = match (config.show_cursor, supported(GraphicsCaptureApi::is_cursor_settings_supported())) {
        (false, true) => CursorCaptureSettings::WithoutCursor,
        _ => CursorCaptureSettings::Default,
    };
    // Tanpa ini Windows menggambar bingkai kuning di sekeliling layar yang di-capture.
    let border = if supported(GraphicsCaptureApi::is_border_settings_supported()) {
        DrawBorderSettings::WithoutBorder
    } else {
        DrawBorderSettings::Default
    };
    // Membatasi laju frame di sisi OS. Frame tidak dibuang sendiri di sini:
    // yang terbuang bisa saja perubahan terakhir sebelum layar diam.
    let interval = if supported(GraphicsCaptureApi::is_minimum_update_interval_supported()) {
        MinimumUpdateIntervalSettings::Custom(Duration::from_secs(1) / config.max_fps.max(1))
    } else {
        MinimumUpdateIntervalSettings::Default
    };

    let settings = Settings::new(
        monitor,
        cursor,
        border,
        SecondaryWindowSettings::Default,
        interval,
        DirtyRegionSettings::Default,
        ColorFormat::Bgra8,
        HandlerSetup {
            sink,
            max_long_edge: config.max_long_edge,
        },
    );
    let control = Handler::start_free_threaded(settings).map_err(|error| anyhow!("gagal memulai capture: {error}"))?;

    Ok(Box::new(WindowsCapturer {
        control: Some(control),
        frame_size,
    }))
}
