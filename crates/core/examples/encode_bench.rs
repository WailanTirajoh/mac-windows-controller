//! Mengukur kecepatan encoder tanpa capture (tidak butuh izin Screen Recording).
//!
//! Kontennya dokumen sintetis yang di-scroll, lihat `video::testsrc`.
//!
//! ```text
//! cargo run -p sambung-core --release --example encode_bench -- [lebar] [tinggi] [frame] [mbps]
//! ```

use std::time::Instant;

use sambung_core::video::openh264_enc::OpenH264Encoder;
use sambung_core::video::testsrc::scrolling_document;
use sambung_core::video::{EncoderSettings, VideoEncoder};

fn arg<T: std::str::FromStr>(index: usize, default: T) -> T {
    std::env::args()
        .nth(index)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn main() -> anyhow::Result<()> {
    let width: u32 = arg(1, 1920);
    let height: u32 = arg(2, 1248);
    let frames: u32 = arg(3, 120);
    let mbps: f64 = arg(4, 10.0);

    let mut encoder = OpenH264Encoder::new(EncoderSettings {
        bitrate_bps: (mbps * 1_000_000.0) as u32,
        max_fps: 60,
    })?;

    let mut times = Vec::with_capacity(frames as usize);
    let mut bytes = 0usize;
    let mut skipped = 0u32;
    for index in 0..frames {
        let frame = scrolling_document(width, height, index);
        let before = Instant::now();
        let encoded = encoder.encode(&frame)?;
        times.push(before.elapsed().as_secs_f64() * 1000.0);
        match encoded {
            Some(encoded) => bytes += encoded.data.len(),
            None => skipped += 1,
        }
    }

    // Frame pertama (keyframe + inisialisasi) tidak mewakili kondisi berjalan.
    let mut steady = times[1..].to_vec();
    steady.sort_by(|a, b| a.total_cmp(b));
    let mean = steady.iter().sum::<f64>() / steady.len() as f64;
    let p95 = steady[((steady.len() - 1) as f64 * 0.95).round() as usize];

    println!(
        "{width}x{height}: keyframe {:.1} ms, delta rata-rata {mean:.2} ms, p95 {p95:.2} ms (~{:.0} fps), \
         {:.2} Mbps pada 60 fps, {skipped}/{frames} frame dilewati",
        times[0],
        1000.0 / mean,
        bytes as f64 * 8.0 / (f64::from(frames) / 60.0) / 1e6,
    );
    Ok(())
}
