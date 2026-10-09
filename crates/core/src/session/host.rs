//! Sisi tujuan: layar di-capture, di-encode, dan dikirim; input dari
//! pengendali disuntikkan ke OS.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use tokio::sync::{mpsc, watch, Semaphore};

use super::SessionServices;
use crate::capture::{CaptureBackend, CaptureConfig, DisplayInfo, ScreenCapturer};
use crate::clipboard::ClipboardSync;
use crate::input::InputInjector;
use crate::protocol::{DisplayDesc, InputEvent, StreamKind, VideoHeader, VideoParams};
use crate::video::openh264_enc::OpenH264Encoder;
use crate::video::pacing::LatestSlot;
use crate::video::{EncodedFrame, EncoderSettings, I420Frame, VideoEncoder};

/// Jumlah frame yang boleh sedang dalam perjalanan. Kalau jaringan melambat,
/// encoder menunggu, dan frame baru menimpa yang lama sebelum di-encode,
/// sehingga latensi tidak menumpuk.
const MAX_FRAMES_IN_FLIGHT: usize = 2;

/// Seberapa sering thread encoder memeriksa tanda berhenti saat layar diam.
const IDLE_POLL: Duration = Duration::from_millis(100);

pub struct HostSession {
    capture: Arc<dyn CaptureBackend>,
    video: VideoParams,
    stop: Arc<AtomicBool>,
    keyframe: Arc<AtomicBool>,
    slot: Arc<LatestSlot<I420Frame>>,
    /// Monitor yang sedang dikirim; dipakai thread input untuk memetakan posisi.
    display: Arc<Mutex<DisplayInfo>>,
    capturer: Option<Box<dyn ScreenCapturer>>,
    input: Option<std_mpsc::Sender<InputEvent>>,
    clipboard: ClipboardSync,
    /// Saat field ini di-drop, task pengirim berhenti, lalu thread encoder
    /// ikut berhenti karena kanal frame-nya tertutup.
    _alive: watch::Sender<()>,
}

fn describe(display: &DisplayInfo) -> DisplayDesc {
    DisplayDesc {
        id: display.id,
        width: display.pixel_width,
        height: display.pixel_height,
        is_primary: display.is_primary,
    }
}

fn pick_display(displays: &[DisplayInfo], requested: Option<u32>) -> anyhow::Result<DisplayInfo> {
    requested
        .and_then(|id| displays.iter().find(|display| display.id == id))
        .or_else(|| displays.iter().find(|display| display.is_primary))
        .or(displays.first())
        .cloned()
        .context("tidak ada layar yang bisa ditampilkan")
}

impl HostSession {
    /// Memulai capture dan pengiriman video lewat `connection`. Teks yang
    /// disalin di perangkat ini selama sesi dikirim ke `copied`. Harus
    /// dipanggil dari dalam runtime tokio.
    pub fn start(
        connection: quinn::Connection,
        services: &SessionServices,
        display_id: Option<u32>,
        video: VideoParams,
        copied: mpsc::UnboundedSender<String>,
    ) -> anyhow::Result<(Self, Vec<DisplayDesc>)> {
        let displays = services.capture.displays()?;
        let display = pick_display(&displays, display_id)?;
        let encoder = OpenH264Encoder::new(EncoderSettings {
            bitrate_bps: video.bitrate_bps,
            max_fps: video.max_fps,
        })?;

        let (alive, ended) = watch::channel(());
        let clipboard = ClipboardSync::start(Arc::clone(&services.clipboard), move |text| {
            let _ = copied.send(text);
        });
        let mut session = Self {
            capture: Arc::clone(&services.capture),
            video,
            stop: Arc::new(AtomicBool::new(false)),
            keyframe: Arc::new(AtomicBool::new(false)),
            slot: Arc::new(LatestSlot::new()),
            display: Arc::new(Mutex::new(display.clone())),
            capturer: None,
            input: None,
            clipboard,
            _alive: alive,
        };
        session.capturer = Some(session.start_capture(&display)?);

        let (encoded, frames) = mpsc::channel(1);
        spawn_encoder(
            encoder,
            Arc::clone(&session.slot),
            Arc::clone(&session.stop),
            Arc::clone(&session.keyframe),
            encoded,
        );
        tokio::spawn(send_frames(connection, frames, Arc::clone(&session.stop), ended));

        // Tanpa injector (mis. izin belum ada) sesi tetap jalan, hanya untuk melihat.
        match services.input.open() {
            Ok(injector) => session.input = Some(spawn_injector(injector, Arc::clone(&session.display))),
            Err(error) => tracing::warn!("sesi berjalan tanpa kontrol input: {error:#}"),
        }

        Ok((session, displays.iter().map(describe).collect()))
    }

    fn start_capture(&self, display: &DisplayInfo) -> anyhow::Result<Box<dyn ScreenCapturer>> {
        let slot = Arc::clone(&self.slot);
        self.capture.start(
            CaptureConfig {
                display_id: display.id,
                max_long_edge: self.video.max_long_edge,
                max_fps: self.video.max_fps,
                show_cursor: true,
            },
            Box::new(move |frame| {
                slot.put(frame);
            }),
        )
    }

    /// Monitor yang sedang dikirim.
    pub fn display_id(&self) -> u32 {
        self.display.lock().unwrap().id
    }

    pub fn input_enabled(&self) -> bool {
        self.input.is_some()
    }

    /// Pindah ke monitor lain. Kalau gagal, monitor lama tetap dikirim.
    pub fn select_display(&mut self, display_id: u32) -> anyhow::Result<()> {
        let display = self
            .capture
            .displays()?
            .into_iter()
            .find(|display| display.id == display_id)
            .with_context(|| format!("layar {display_id} tidak ditemukan"))?;

        // Capture lama dihentikan dulu; sebagian OS menolak dua capture sekaligus.
        let previous = self.display.lock().unwrap().clone();
        self.capturer = None;
        match self.start_capture(&display) {
            Ok(capturer) => {
                self.capturer = Some(capturer);
                *self.display.lock().unwrap() = display;
                self.request_keyframe();
                Ok(())
            }
            Err(error) => {
                self.capturer = self.start_capture(&previous).ok();
                self.request_keyframe();
                Err(error)
            }
        }
    }

    pub fn request_keyframe(&self) {
        self.keyframe.store(true, Ordering::Relaxed);
    }

    /// Menerapkan teks yang disalin di perangkat pengendali.
    pub fn receive_clipboard(&self, text: String) {
        self.clipboard.receive(text);
    }

    /// Kanal ke injector, untuk stream input dari pengendali. `None` kalau
    /// sesi hanya untuk melihat.
    pub fn input_handle(&self) -> Option<std_mpsc::Sender<InputEvent>> {
        self.input.clone()
    }
}

impl Drop for HostSession {
    fn drop(&mut self) {
        // Thread encoder dan injector berhenti sendiri; tidak di-join supaya
        // task async yang men-drop sesi tidak tertahan.
        self.stop.store(true, Ordering::Relaxed);
        self.capturer = None;
        // Menutup kanal membuat thread injector melepas semua tombol lalu berhenti.
        self.input = None;
    }
}

fn spawn_encoder(
    mut encoder: OpenH264Encoder,
    slot: Arc<LatestSlot<I420Frame>>,
    stop: Arc<AtomicBool>,
    keyframe: Arc<AtomicBool>,
    encoded: mpsc::Sender<EncodedFrame>,
) {
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            let Some(frame) = slot.take(IDLE_POLL) else {
                continue;
            };
            if keyframe.swap(false, Ordering::Relaxed) {
                encoder.request_keyframe();
            }
            match encoder.encode(&frame) {
                // Menunggu di sini saat jaringan penuh; frame baru menimpa slot.
                Ok(Some(frame)) => {
                    if encoded.blocking_send(frame).is_err() {
                        break;
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!("encoder berhenti: {error:#}");
                    break;
                }
            }
        }
    });
}

async fn send_frames(
    connection: quinn::Connection,
    mut frames: mpsc::Receiver<EncodedFrame>,
    stop: Arc<AtomicBool>,
    mut ended: watch::Receiver<()>,
) {
    let in_flight = Arc::new(Semaphore::new(MAX_FRAMES_IN_FLIGHT));
    let mut sequence = 0u64;
    loop {
        // `ended.changed()` gagal begitu sesi di-drop, juga saat sedang menunggu giliran kirim.
        let next = async {
            let frame = frames.recv().await?;
            let permit = Arc::clone(&in_flight).acquire_owned().await.ok()?;
            Some((frame, permit))
        };
        let (frame, permit) = tokio::select! {
            next = next => match next {
                Some(next) => next,
                None => break,
            },
            _ = ended.changed() => break,
        };
        let header = VideoHeader {
            sequence,
            timestamp_us: frame.timestamp_us,
            keyframe: frame.keyframe,
            width: frame.width,
            height: frame.height,
        };
        sequence += 1;

        let connection = connection.clone();
        let stop = Arc::clone(&stop);
        tokio::spawn(async move {
            if send_frame(&connection, &header, &frame.data).await.is_err() {
                stop.store(true, Ordering::Relaxed);
            }
            drop(permit);
        });
    }
}

/// Mengirim satu frame lewat stream sendiri dan menunggu sampai diterima.
async fn send_frame(connection: &quinn::Connection, header: &VideoHeader, data: &[u8]) -> anyhow::Result<()> {
    let mut stream = connection.open_uni().await?;
    let mut prefix = [0u8; 1 + VideoHeader::BYTES];
    prefix[0] = StreamKind::Video as u8;
    prefix[1..].copy_from_slice(&header.encode());
    stream.write_all(&prefix).await?;
    stream.write_all(data).await?;
    stream.finish()?;
    // Selesai saat lawan mengonfirmasi seluruh data, bukan saat data masuk buffer.
    let _ = stream.stopped().await;
    Ok(())
}

fn spawn_injector(
    mut injector: Box<dyn InputInjector>,
    display: Arc<Mutex<DisplayInfo>>,
) -> std_mpsc::Sender<InputEvent> {
    let (events, queued) = std_mpsc::channel::<InputEvent>();
    std::thread::spawn(move || {
        for event in queued {
            let display = display.lock().unwrap().clone();
            if let Err(error) = injector.inject(&event, &display) {
                tracing::debug!("event input gagal disuntikkan: {error:#}");
            }
        }
        injector.release_all();
    });
    events
}
