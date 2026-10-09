//! Frame sintetis → encoder → decoder: memastikan bitstream yang dihasilkan
//! valid dan gambarnya masih mirip dengan sumbernya.

use openh264::decoder::Decoder;
use openh264::formats::YUVSource;
use sambung_core::video::openh264_enc::OpenH264Encoder;
use sambung_core::video::{EncoderSettings, I420Frame, VideoEncoder};

const WIDTH: u32 = 640;
const HEIGHT: u32 = 360;

/// Gambar bergaris yang bergeser tiap frame, mirip teks yang di-scroll.
fn synthetic_frame(index: u32) -> I420Frame {
    let mut frame = I420Frame::new(WIDTH, HEIGHT, u64::from(index) * 16_667);
    for (row, line) in frame.y.chunks_exact_mut(WIDTH as usize).enumerate() {
        for (col, pixel) in line.iter_mut().enumerate() {
            let band = (row as u32 + index * 4) / 16 % 2;
            let stripe = (col as u32 / 8) % 2;
            *pixel = if band == stripe { 200 } else { 40 };
        }
    }
    frame
}

/// PSNR plane Y dalam dB; makin besar makin mirip.
fn luma_psnr(source: &I420Frame, decoded: &impl YUVSource) -> f64 {
    let (stride, _, _) = decoded.strides();
    let width = source.width as usize;
    let squared_error: f64 = source
        .y
        .chunks_exact(width)
        .zip(decoded.y().chunks(stride))
        .flat_map(|(a, b)| a.iter().zip(&b[..width]))
        .map(|(a, b)| (f64::from(*a) - f64::from(*b)).powi(2))
        .sum();
    let mse = squared_error / source.y.len() as f64;
    10.0 * (255.0_f64.powi(2) / mse.max(1e-9)).log10()
}

fn encoder() -> OpenH264Encoder {
    OpenH264Encoder::new(EncoderSettings {
        bitrate_bps: 4_000_000,
        max_fps: 60,
    })
    .unwrap()
}

#[test]
fn encoded_stream_decodes_back_to_a_similar_picture() {
    let mut encoder = encoder();
    let mut decoder = Decoder::new().unwrap();
    let mut decoded_frames = 0;

    for index in 0..30 {
        let source = synthetic_frame(index);
        // Rate control boleh melewati frame; yang dilewati tidak punya bitstream.
        let Some(encoded) = encoder.encode(&source).unwrap() else {
            continue;
        };
        assert_eq!(encoded.keyframe, index == 0, "hanya frame pertama yang keyframe");
        assert_eq!((encoded.width, encoded.height), (WIDTH, HEIGHT));

        let decoded = decoder.decode(&encoded.data).unwrap().expect("tiap frame harus terdekode");
        assert_eq!(decoded.dimensions(), (WIDTH as usize, HEIGHT as usize));

        let psnr = luma_psnr(&source, &decoded);
        assert!(psnr > 30.0, "frame {index}: PSNR {psnr:.1} dB terlalu rendah");
        decoded_frames += 1;
    }

    assert!(decoded_frames >= 25, "terlalu banyak frame dilewati: hanya {decoded_frames} dari 30");
}

#[test]
fn requested_keyframe_is_emitted_on_the_next_frame() {
    let mut encoder = encoder();
    for index in 0..5 {
        encoder.encode(&synthetic_frame(index)).unwrap();
    }

    encoder.request_keyframe();
    let encoded = encoder.encode(&synthetic_frame(5)).unwrap().unwrap();

    assert!(encoded.keyframe);
}

#[test]
fn stream_starts_with_annex_b_start_code() {
    let encoded = encoder().encode(&synthetic_frame(0)).unwrap().unwrap();
    assert_eq!(&encoded.data[..4], &[0, 0, 0, 1]);
}
