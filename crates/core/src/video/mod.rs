//! Pipeline video: frame mentah, konversi warna, encoder, dan slot frame.

pub mod convert;
pub mod frame;
pub mod openh264_enc;
pub mod pacing;
pub mod testsrc;

pub use frame::{fit_even, EncodedFrame, I420Frame};

/// Pengaturan encoder yang berlaku untuk semua implementasi.
#[derive(Debug, Clone, Copy)]
pub struct EncoderSettings {
    pub bitrate_bps: u32,
    pub max_fps: u32,
}

/// Encoder video. Implementasi software (`openh264`) dan, nanti, hardware
/// (VideoToolbox / Media Foundation) berada di balik trait ini.
pub trait VideoEncoder: Send {
    /// Meng-encode satu frame. `Ok(None)` berarti encoder melewati frame ini
    /// karena rate control.
    fn encode(&mut self, frame: &I420Frame) -> anyhow::Result<Option<EncodedFrame>>;

    /// Frame berikutnya akan di-encode sebagai keyframe.
    fn request_keyframe(&mut self);
}
