//! Capture layar macOS lewat ScreenCaptureKit.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context};
use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_core_graphics::{
    kCGDisplayStreamYCbCrMatrix_ITU_R_709_2, CGDisplayCopyDisplayMode, CGDisplayMode, CGMainDisplayID,
};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::{
    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, CVPixelBuffer, CVPixelBufferGetBaseAddressOfPlane,
    CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferGetHeightOfPlane, CVPixelBufferGetPlaneCount,
    CVPixelBufferGetWidthOfPlane, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
    CVPixelBufferUnlockBaseAddress,
};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSNumber, NSString};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCFrameStatus, SCShareableContent, SCStream, SCStreamConfiguration,
    SCStreamFrameInfoStatus, SCStreamOutput, SCStreamOutputType,
};

use super::{CaptureConfig, DisplayInfo, FrameSink, ScreenCapturer};
use crate::video::convert::nv12_to_i420;
use crate::video::{fit_even, I420Frame};

/// Batas waktu menunggu balasan asinkron dari ScreenCaptureKit.
const SCK_TIMEOUT: Duration = Duration::from_secs(10);

/// Membawa objek Objective-C melewati channel dari thread completion handler.
struct SendRetained<T: ?Sized>(Retained<T>);

// SAFETY: objek hanya dipindahkan, tidak dipakai bersamaan dari dua thread;
// retain/release Objective-C aman dari thread mana pun.
unsafe impl<T: ?Sized> Send for SendRetained<T> {}

fn describe(error: *mut NSError) -> String {
    // SAFETY: pointer berasal dari completion handler; null berarti tanpa detail.
    match unsafe { error.as_ref() } {
        Some(error) => error.localizedDescription().to_string(),
        None => "tanpa detail".to_owned(),
    }
}

fn shareable_content() -> anyhow::Result<Retained<SCShareableContent>> {
    let (tx, rx) = mpsc::channel();
    let handler = RcBlock::new(move |content: *mut SCShareableContent, error: *mut NSError| {
        // SAFETY: `content` valid selama handler berjalan; di-retain agar hidup lebih lama.
        let result = match unsafe { Retained::retain(content) } {
            Some(content) => Ok(SendRetained(content)),
            None => Err(describe(error)),
        };
        let _ = tx.send(result);
    });
    // SAFETY: block hidup sampai pemanggilan selesai dan di-copy oleh framework.
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&handler) };

    match rx.recv_timeout(SCK_TIMEOUT) {
        Ok(Ok(content)) => Ok(content.0),
        Ok(Err(reason)) => bail!(
            "tidak bisa membaca daftar layar ({reason}). Pastikan izin Screen Recording sudah diberikan"
        ),
        Err(_) => bail!("ScreenCaptureKit tidak merespons saat membaca daftar layar"),
    }
}

/// Ukuran monitor dalam piksel fisik, atau `None` kalau mode tidak terbaca.
fn pixel_size(display_id: u32) -> Option<(u32, u32)> {
    let mode = CGDisplayCopyDisplayMode(display_id)?;
    Some((
        CGDisplayMode::pixel_width(Some(&mode)) as u32,
        CGDisplayMode::pixel_height(Some(&mode)) as u32,
    ))
}

fn display_info(display: &SCDisplay, main_id: u32) -> DisplayInfo {
    // SAFETY: getter properti tanpa prasyarat.
    let (id, frame, width, height) =
        unsafe { (display.displayID(), display.frame(), display.width(), display.height()) };
    let (pixel_width, pixel_height) = pixel_size(id).unwrap_or((width as u32, height as u32));
    DisplayInfo {
        id,
        x: frame.origin.x,
        y: frame.origin.y,
        width: frame.size.width,
        height: frame.size.height,
        pixel_width,
        pixel_height,
        is_primary: id == main_id,
    }
}

pub fn list_displays() -> anyhow::Result<Vec<DisplayInfo>> {
    let main_id = CGMainDisplayID();
    let content = shareable_content()?;
    // SAFETY: getter properti tanpa prasyarat.
    let displays = unsafe { content.displays() };
    Ok(displays.iter().map(|display| display_info(&display, main_id)).collect())
}

struct OutputIvars {
    sink: FrameSink,
    started: Instant,
}

define_class!(
    // SAFETY: NSObject tidak punya syarat subclassing, dan `StreamOutput`
    // tidak mengimplementasikan `Drop`.
    #[unsafe(super = NSObject)]
    #[name = "SambungStreamOutput"]
    #[ivars = OutputIvars]
    struct StreamOutput;

    // SAFETY: `NSObjectProtocol` tidak punya syarat keamanan.
    unsafe impl NSObjectProtocol for StreamOutput {}

    // SAFETY: tanda tangan method sama dengan deklarasi protokol.
    unsafe impl SCStreamOutput for StreamOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn did_output(&self, _stream: &SCStream, sample: &CMSampleBuffer, output_type: SCStreamOutputType) {
            if output_type != SCStreamOutputType::Screen {
                return;
            }
            // Frame `Idle` berarti layar tidak berubah, jadi tidak ada yang dikirim.
            if !matches!(frame_status(sample), Some(SCFrameStatus::Complete | SCFrameStatus::Started)) {
                return;
            }
            let ivars = self.ivars();
            let timestamp_us = ivars.started.elapsed().as_micros() as u64;
            if let Some(frame) = copy_frame(sample, timestamp_us) {
                (ivars.sink)(frame);
            }
        }
    }
);

impl StreamOutput {
    fn new(sink: FrameSink) -> Retained<Self> {
        let this = Self::alloc().set_ivars(OutputIvars {
            sink,
            started: Instant::now(),
        });
        // SAFETY: `init` milik NSObject dipanggil pada objek yang baru dialokasikan.
        unsafe { msg_send![super(this), init] }
    }
}

fn frame_status(sample: &CMSampleBuffer) -> Option<SCFrameStatus> {
    // SAFETY: sample valid selama callback; array attachment milik sample.
    let attachments = unsafe { sample.sample_attachments_array(false) }?;
    // SAFETY: CFArray toll-free bridged ke NSArray, dan ScreenCaptureKit
    // mengisinya dengan satu dictionary berkunci string per sample.
    let attachments: &NSArray<NSDictionary<NSString, AnyObject>> =
        unsafe { &*std::ptr::from_ref(&*attachments).cast() };
    let info = attachments.firstObject()?;
    // SAFETY: konstanta kunci disediakan framework.
    let status = info.objectForKey(unsafe { SCStreamFrameInfoStatus })?;
    let status = status.downcast_ref::<NSNumber>()?;
    Some(SCFrameStatus(status.integerValue()))
}

/// Menyalin pixel buffer NV12 milik sample ke frame I420 baru.
fn copy_frame(sample: &CMSampleBuffer, timestamp_us: u64) -> Option<I420Frame> {
    // SAFETY: sample valid selama callback.
    let buffer = unsafe { sample.image_buffer() }?;
    let buffer: &CVPixelBuffer = &buffer;

    // SAFETY: buffer valid; setiap lock dipasangkan dengan unlock di bawah.
    if unsafe { CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly) } != 0 {
        return None;
    }
    let frame = copy_locked(buffer, timestamp_us);
    // SAFETY: pasangan dari lock di atas, dengan flag yang sama.
    unsafe { CVPixelBufferUnlockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly) };
    frame
}

/// Membaca plane dari pixel buffer yang sudah terkunci.
fn copy_locked(buffer: &CVPixelBuffer, timestamp_us: u64) -> Option<I420Frame> {
    if CVPixelBufferGetPlaneCount(buffer) != 2 {
        return None;
    }

    // Ukuran buffer bisa berbeda dari yang diminta, jadi ukuran frame diambil dari buffer.
    let width = (CVPixelBufferGetWidthOfPlane(buffer, 0) & !1) as u32;
    let height = (CVPixelBufferGetHeightOfPlane(buffer, 0) & !1) as u32;
    if width == 0 || height == 0 {
        return None;
    }

    let y_stride = CVPixelBufferGetBytesPerRowOfPlane(buffer, 0);
    let uv_stride = CVPixelBufferGetBytesPerRowOfPlane(buffer, 1);
    let y_ptr = CVPixelBufferGetBaseAddressOfPlane(buffer, 0).cast::<u8>();
    let uv_ptr = CVPixelBufferGetBaseAddressOfPlane(buffer, 1).cast::<u8>();
    if y_ptr.is_null() || uv_ptr.is_null() {
        return None;
    }

    // SAFETY: buffer terkunci oleh pemanggil, dan tiap plane berisi
    // `tinggi_plane * bytes_per_row` byte.
    let (y, uv) = unsafe {
        (
            std::slice::from_raw_parts(y_ptr, y_stride * CVPixelBufferGetHeightOfPlane(buffer, 0)),
            std::slice::from_raw_parts(uv_ptr, uv_stride * CVPixelBufferGetHeightOfPlane(buffer, 1)),
        )
    };

    let mut frame = I420Frame::new(width, height, timestamp_us);
    nv12_to_i420(y, y_stride, uv, uv_stride, &mut frame);
    Some(frame)
}

struct MacCapturer {
    stream: Retained<SCStream>,
    // Dipegang agar tetap hidup selama stream berjalan.
    _output: Retained<StreamOutput>,
    _queue: DispatchRetained<DispatchQueue>,
    frame_size: (u32, u32),
}

// SAFETY: setelah dibuat, objek hanya disentuh lewat `Drop`, dan SCStream
// boleh dihentikan dari thread mana pun.
unsafe impl Send for MacCapturer {}

impl ScreenCapturer for MacCapturer {
    fn frame_size(&self) -> (u32, u32) {
        self.frame_size
    }
}

impl Drop for MacCapturer {
    fn drop(&mut self) {
        // Menunggu sampai stream benar-benar berhenti supaya tidak ada callback
        // yang masih berjalan saat output dilepas.
        let (tx, rx) = mpsc::channel();
        let handler = RcBlock::new(move |_error: *mut NSError| {
            let _ = tx.send(());
        });
        // SAFETY: block di-copy oleh framework.
        unsafe { self.stream.stopCaptureWithCompletionHandler(Some(&handler)) };
        let _ = rx.recv_timeout(SCK_TIMEOUT);
    }
}

pub fn start_capture(config: CaptureConfig, sink: FrameSink) -> anyhow::Result<Box<dyn ScreenCapturer>> {
    let content = shareable_content()?;
    // SAFETY: getter properti tanpa prasyarat.
    let displays = unsafe { content.displays() };
    let display = displays
        .iter()
        .find(|display| unsafe { display.displayID() } == config.display_id)
        .with_context(|| format!("layar {} tidak ditemukan", config.display_id))?;

    // SAFETY: getter properti tanpa prasyarat.
    let (point_width, point_height) = unsafe { (display.width() as u32, display.height() as u32) };
    let (pixel_width, pixel_height) = pixel_size(config.display_id).unwrap_or((point_width, point_height));
    let (width, height) = fit_even(pixel_width, pixel_height, config.max_long_edge);

    // SAFETY: semua argumen adalah objek valid yang hidup selama pemanggilan.
    let stream = unsafe {
        let filter = SCContentFilter::initWithDisplay_excludingWindows(
            SCContentFilter::alloc(),
            &display,
            &NSArray::new(),
        );

        let stream_config = SCStreamConfiguration::new();
        stream_config.setWidth(width as usize);
        stream_config.setHeight(height as usize);
        stream_config.setPixelFormat(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange);
        stream_config.setColorMatrix(kCGDisplayStreamYCbCrMatrix_ITU_R_709_2);
        stream_config.setMinimumFrameInterval(CMTime::new(1, config.max_fps as i32));
        stream_config.setShowsCursor(config.show_cursor);

        SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter, &stream_config, None)
    };

    let output = StreamOutput::new(sink);
    let queue = DispatchQueue::new("sambung.capture", DispatchQueueAttr::SERIAL);
    // SAFETY: output dan queue dipegang oleh `MacCapturer` selama stream hidup.
    unsafe {
        stream.addStreamOutput_type_sampleHandlerQueue_error(
            ProtocolObject::from_ref(&*output),
            SCStreamOutputType::Screen,
            Some(&queue),
        )
    }
    .map_err(|err| anyhow!("gagal memasang handler frame: {}", err.localizedDescription()))?;

    let (tx, rx) = mpsc::channel();
    let handler = RcBlock::new(move |error: *mut NSError| {
        let _ = tx.send(if error.is_null() { Ok(()) } else { Err(describe(error)) });
    });
    // SAFETY: block di-copy oleh framework.
    unsafe { stream.startCaptureWithCompletionHandler(Some(&handler)) };
    match rx.recv_timeout(SCK_TIMEOUT) {
        Ok(Ok(())) => {}
        Ok(Err(reason)) => bail!("gagal memulai capture: {reason}"),
        Err(_) => bail!("ScreenCaptureKit tidak merespons saat memulai capture"),
    }

    Ok(Box::new(MacCapturer {
        stream,
        _output: output,
        _queue: queue,
        frame_size: (width, height),
    }))
}
