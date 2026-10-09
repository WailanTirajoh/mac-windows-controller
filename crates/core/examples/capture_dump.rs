//! Uji pipeline video: capture layar utama → encode H.264 → tulis `out.h264`.
//!
//! ```text
//! cargo run -p sambung-core --release --example capture_dump -- [detik] [sisi_panjang] [mbps] [fps]
//! ffplay out.h264
//! ```
//!
//! Selama berjalan, gerakkan jendela atau putar video supaya layar berubah;
//! layar yang diam tidak menghasilkan frame.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use sambung_core::capture::{self, CaptureConfig};
use sambung_core::video::openh264_enc::OpenH264Encoder;
use sambung_core::video::pacing::LatestSlot;
use sambung_core::video::{EncoderSettings, I420Frame, VideoEncoder};

fn arg<T: std::str::FromStr>(index: usize, default: T) -> T {
    std::env::args()
        .nth(index)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn main() -> anyhow::Result<()> {
    let seconds: u64 = arg(1, 5);
    let max_long_edge: u32 = arg(2, 1920);
    let mbps: f64 = arg(3, 10.0);
    let max_fps: u32 = arg(4, 60);

    let displays = capture::list_displays()?;
    for display in &displays {
        println!(
            "layar {}: {}x{} point di ({}, {}), {}x{} piksel{}",
            display.id,
            display.width,
            display.height,
            display.x,
            display.y,
            display.pixel_width,
            display.pixel_height,
            if display.is_primary { " [utama]" } else { "" }
        );
    }
    let display = displays
        .iter()
        .find(|display| display.is_primary)
        .or(displays.first())
        .context("tidak ada layar")?;

    let slot = Arc::new(LatestSlot::<I420Frame>::new());
    let captured = Arc::new(AtomicU64::new(0));
    let dropped = Arc::new(AtomicU64::new(0));

    let capturer = {
        let slot = Arc::clone(&slot);
        let captured = Arc::clone(&captured);
        let dropped = Arc::clone(&dropped);
        capture::start_capture(
            CaptureConfig {
                display_id: display.id,
                max_long_edge,
                max_fps,
                show_cursor: true,
            },
            Box::new(move |frame| {
                captured.fetch_add(1, Ordering::Relaxed);
                if slot.put(frame) {
                    dropped.fetch_add(1, Ordering::Relaxed);
                }
            }),
        )?
    };
    let (width, height) = capturer.frame_size();
    println!("capture {width}x{height} @ maks {max_fps} fps, target {mbps} Mbps, {seconds} detik");

    let mut encoder = OpenH264Encoder::new(EncoderSettings {
        bitrate_bps: (mbps * 1_000_000.0) as u32,
        max_fps,
    })?;
    let mut out = BufWriter::new(File::create("out.h264").context("gagal membuat out.h264")?);

    let mut encode_ms = Vec::new();
    let mut bytes = 0usize;
    let mut keyframes = 0usize;
    let started = Instant::now();
    let deadline = started + Duration::from_secs(seconds);

    while Instant::now() < deadline {
        let Some(frame) = slot.take(Duration::from_millis(100)) else {
            continue;
        };
        let before = Instant::now();
        let encoded = encoder.encode(&frame)?;
        encode_ms.push(before.elapsed().as_secs_f64() * 1000.0);

        if let Some(encoded) = encoded {
            bytes += encoded.data.len();
            keyframes += usize::from(encoded.keyframe);
            out.write_all(&encoded.data)?;
        }
    }
    drop(capturer);
    out.flush()?;

    let elapsed = started.elapsed().as_secs_f64();
    let encoded_frames = encode_ms.len();
    encode_ms.sort_by(|a, b| a.total_cmp(b));
    let mean = encode_ms.iter().sum::<f64>() / encoded_frames.max(1) as f64;

    println!("frame dari capture : {}", captured.load(Ordering::Relaxed));
    println!("dibuang (encoder tertinggal): {}", dropped.load(Ordering::Relaxed));
    println!(
        "di-encode          : {encoded_frames} ({:.1} fps), {keyframes} keyframe",
        encoded_frames as f64 / elapsed
    );
    println!(
        "waktu encode (ms)  : rata-rata {mean:.2}, p50 {:.2}, p95 {:.2}, maks {:.2}",
        percentile(&encode_ms, 0.50),
        percentile(&encode_ms, 0.95),
        percentile(&encode_ms, 1.0),
    );
    println!(
        "ukuran             : {:.2} MB, rata-rata {:.2} Mbps",
        bytes as f64 / 1e6,
        bytes as f64 * 8.0 / elapsed / 1e6
    );
    println!("hasil              : out.h264");
    Ok(())
}
