use super::{CaptureConfig, DisplayInfo, FrameSink, ScreenCapturer};

pub fn list_displays() -> anyhow::Result<Vec<DisplayInfo>> {
    anyhow::bail!("capture layar belum didukung di OS ini")
}

pub fn start_capture(_config: CaptureConfig, _sink: FrameSink) -> anyhow::Result<Box<dyn ScreenCapturer>> {
    anyhow::bail!("capture layar belum didukung di OS ini")
}
