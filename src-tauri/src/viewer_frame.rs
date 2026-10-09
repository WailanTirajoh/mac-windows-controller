//! Format biner frame video yang dikirim ke webview viewer.
//!
//! ```text
//! byte 0      flag (bit 0 = keyframe)
//! byte 1..4   cadangan
//! byte 4..8   lebar, u32 little-endian
//! byte 8..12  tinggi, u32 little-endian
//! byte 12..20 waktu capture (mikrodetik), u64 little-endian
//! byte 20..28 waktu kirim, mikrodetik sejak epoch Unix, u64 little-endian
//! byte 28..   access unit H.264 format Annex B
//! ```
//!
//! Pembacanya ada di `ui/src/viewer/frame.ts`.

use std::time::{SystemTime, UNIX_EPOCH};

use sambung_core::video::EncodedFrame;

const HEADER_BYTES: usize = 28;
const FLAG_KEYFRAME: u8 = 1;

pub fn pack(frame: &EncodedFrame) -> Vec<u8> {
    let sent_epoch_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_micros() as u64);

    let mut message = Vec::with_capacity(HEADER_BYTES + frame.data.len());
    message.extend_from_slice(&[if frame.keyframe { FLAG_KEYFRAME } else { 0 }, 0, 0, 0]);
    message.extend_from_slice(&frame.width.to_le_bytes());
    message.extend_from_slice(&frame.height.to_le_bytes());
    message.extend_from_slice(&frame.timestamp_us.to_le_bytes());
    message.extend_from_slice(&sent_epoch_us.to_le_bytes());
    message.extend_from_slice(&frame.data);
    message
}
