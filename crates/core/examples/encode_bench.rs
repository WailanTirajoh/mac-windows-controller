//! Mengukur kecepatan encoder tanpa capture (tidak butuh izin Screen Recording).
//!
//! Kontennya meniru dokumen yang di-scroll: pola kontras tinggi mirip teks
//! yang bergeser di 60% area layar, kasus berat untuk encoder layar.
//!
//! ```text
//! cargo run -p sambung-core --release --example encode_bench -- [lebar] [tinggi] [frame] [mbps]
//! ```

use std::time::Instant;

use sambung_core::video::openh264_enc::OpenH264Encoder;
use sambung_core::video::{EncoderSettings, I420Frame, VideoEncoder};

fn arg<T: std::str::FromStr>(index: usize, default: T) -> T {
    std::env::args()
        .nth(index)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

/// Nilai acak tetap untuk sel "huruf" di posisi tertentu.
fn glyph(cell_x: usize, cell_y: usize) -> u8 {
    let mut hash = (cell_x as u32).wrapping_mul(0x9E37_79B1) ^ (cell_y as u32).wrapping_mul(0x85EB_CA77);
    hash ^= hash >> 15;
    hash = hash.wrapping_mul(0x2C1B_3C6D);
    if (hash >> 13) & 3 == 0 {
        30
    } else {
        235
    }
}

fn scrolling_document(width: u32, height: u32, index: u32) -> I420Frame {
    let mut frame = I420Frame::new(width, height, u64::from(index) * 16_667);
    let w = width as usize;
    let scroll = index as usize * 8;
    let content = w * 6 / 10;
    for (row, line) in frame.y.chunks_exact_mut(w).enumerate() {
        for (col, pixel) in line.iter_mut().enumerate() {
            let y = if col < content { row + scroll } else { row };
            // Baris teks setinggi 12 px dengan jarak antarbaris 6 px.
            *pixel = if y % 18 < 12 { glyph(col / 3, y / 3) } else { 235 };
        }
    }
    frame
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
