//! Encoder H.264 software memakai OpenH264, mode screen content.

use anyhow::Context;
use openh264::encoder::{
    BitRate, Encoder, EncoderConfig, FrameRate, FrameType, RateControlMode, UsageType, VuiConfig,
};
use openh264::formats::YUVSlices;
use openh264::{OpenH264API, Timestamp};

use super::{EncodedFrame, EncoderSettings, I420Frame, VideoEncoder};

pub struct OpenH264Encoder {
    inner: Encoder,
}

impl OpenH264Encoder {
    pub fn new(settings: EncoderSettings) -> anyhow::Result<Self> {
        let config = EncoderConfig::new()
            .usage_type(UsageType::ScreenContentRealTime)
            .rate_control_mode(RateControlMode::Bitrate)
            .bitrate(BitRate::from_bps(settings.bitrate_bps))
            .max_frame_rate(FrameRate::from_hz(settings.max_fps as f32))
            // Tanpa frame skip, OpenH264 tidak bisa menjaga target bitrate.
            .skip_frames(true)
            // Dua fitur ini tidak didukung OpenH264 untuk screen content.
            .adaptive_quantization(false)
            .background_detection(false)
            .vui(VuiConfig::bt709());
        let inner = Encoder::with_api_config(OpenH264API::from_source(), config)
            .context("gagal membuat encoder OpenH264")?;
        Ok(Self { inner })
    }
}

impl VideoEncoder for OpenH264Encoder {
    fn encode(&mut self, frame: &I420Frame) -> anyhow::Result<Option<EncodedFrame>> {
        let width = frame.width as usize;
        let height = frame.height as usize;
        let source = YUVSlices::new(
            (&frame.y, &frame.u, &frame.v),
            (width, height),
            (width, width / 2, width / 2),
        );
        let bitstream = self
            .inner
            .encode_at(&source, Timestamp::from_millis(frame.timestamp_us / 1000))
            .context("encode frame gagal")?;

        let keyframe = match bitstream.frame_type() {
            FrameType::IDR | FrameType::I => true,
            FrameType::P | FrameType::IPMixed => false,
            FrameType::Skip | FrameType::Invalid => return Ok(None),
        };
        Ok(Some(EncodedFrame {
            data: bitstream.to_vec(),
            keyframe,
            width: frame.width,
            height: frame.height,
            timestamp_us: frame.timestamp_us,
        }))
    }

    fn request_keyframe(&mut self) {
        self.inner.force_intra_frame();
    }
}
