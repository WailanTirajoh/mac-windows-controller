//! Sesi remote desktop antar dua node sungguhan di localhost, dengan capture
//! layar dan injeksi input tiruan.

mod common;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{pair, TestNode, WAIT};
use openh264::decoder::Decoder;
use openh264::formats::YUVSource;
use sambung_core::capture::{CaptureBackend, CaptureConfig, DisplayInfo, FrameSink, ScreenCapturer};
use sambung_core::input::{InputBackend, InputInjector};
use sambung_core::node::{NodeEvent, Session, SessionError};
use sambung_core::pairing::otp::OtpPolicy;
use sambung_core::protocol::{InputEvent, MouseButton, VideoParams};
use sambung_core::session::controller::SessionEvent;
use sambung_core::session::SessionServices;
use sambung_core::video::testsrc::scrolling_document;
use sambung_core::video::{fit_even, EncodedFrame};

const VIDEO: VideoParams = VideoParams {
    max_long_edge: 1920,
    max_fps: 60,
    bitrate_bps: 4_000_000,
};

fn display(id: u32, x: f64, width: u32, height: u32) -> DisplayInfo {
    DisplayInfo {
        id,
        x,
        y: 0.0,
        width: f64::from(width),
        height: f64::from(height),
        pixel_width: width,
        pixel_height: height,
        is_primary: id == 1,
    }
}

/// Dua "monitor" berukuran berbeda, supaya frame-nya bisa dibedakan.
fn two_displays() -> Vec<DisplayInfo> {
    vec![display(1, 0.0, 640, 360), display(2, 640.0, 320, 240)]
}

/// Capture tiruan: menghasilkan dokumen sintetis yang di-scroll, 60 fps.
struct FakeCapture {
    displays: Vec<DisplayInfo>,
    /// Jumlah capture yang sedang berjalan.
    running: Arc<AtomicUsize>,
}

struct FakeCapturer {
    size: (u32, u32),
    stop: Arc<AtomicBool>,
    running: Arc<AtomicUsize>,
}

impl ScreenCapturer for FakeCapturer {
    fn frame_size(&self) -> (u32, u32) {
        self.size
    }
}

impl Drop for FakeCapturer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.running.fetch_sub(1, Ordering::Relaxed);
    }
}

impl CaptureBackend for FakeCapture {
    fn displays(&self) -> anyhow::Result<Vec<DisplayInfo>> {
        anyhow::ensure!(!self.displays.is_empty(), "izin Screen Recording belum diberikan");
        Ok(self.displays.clone())
    }

    fn start(&self, config: CaptureConfig, sink: FrameSink) -> anyhow::Result<Box<dyn ScreenCapturer>> {
        let shown = self
            .displays
            .iter()
            .find(|display| display.id == config.display_id)
            .expect("layar yang diminta ada");
        let size = fit_even(shown.pixel_width, shown.pixel_height, config.max_long_edge);
        let stop = Arc::new(AtomicBool::new(false));

        let stopped = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut index = 0;
            while !stopped.load(Ordering::Relaxed) {
                sink(scrolling_document(size.0, size.1, index));
                index += 1;
                std::thread::sleep(Duration::from_millis(16));
            }
        });

        self.running.fetch_add(1, Ordering::Relaxed);
        Ok(Box::new(FakeCapturer {
            size,
            stop,
            running: Arc::clone(&self.running),
        }))
    }
}

/// Catatan dari injector tiruan.
#[derive(Default)]
struct InputLog {
    /// Event beserta ID monitor yang sedang ditampilkan saat event itu diterima.
    events: Mutex<Vec<(InputEvent, u32)>>,
    releases: AtomicUsize,
}

struct FakeInput {
    log: Arc<InputLog>,
    available: bool,
}

struct FakeInjector(Arc<InputLog>);

impl InputInjector for FakeInjector {
    fn inject(&mut self, event: &InputEvent, display: &DisplayInfo) -> anyhow::Result<()> {
        self.0.events.lock().unwrap().push((event.clone(), display.id));
        Ok(())
    }

    fn release_all(&mut self) {
        self.0.releases.fetch_add(1, Ordering::Relaxed);
    }
}

impl InputBackend for FakeInput {
    fn open(&self) -> anyhow::Result<Box<dyn InputInjector>> {
        anyhow::ensure!(self.available, "izin Accessibility belum diberikan");
        Ok(Box::new(FakeInjector(Arc::clone(&self.log))))
    }
}

/// Perangkat tujuan dengan layanan tiruan, beserta pegangan untuk memeriksanya.
struct Host {
    node: TestNode,
    input: Arc<InputLog>,
    captures_running: Arc<AtomicUsize>,
}

fn host_with(displays: Vec<DisplayInfo>, input_available: bool) -> Host {
    let input = Arc::new(InputLog::default());
    let captures_running = Arc::new(AtomicUsize::new(0));
    let services = SessionServices {
        capture: Arc::new(FakeCapture {
            displays,
            running: Arc::clone(&captures_running),
        }),
        input: Arc::new(FakeInput {
            log: Arc::clone(&input),
            available: input_available,
        }),
    };
    Host {
        node: TestNode::start_with("PC", OtpPolicy::default(), services),
        input,
        captures_running,
    }
}

fn host() -> Host {
    host_with(two_displays(), true)
}

async fn next(session: &mut Session) -> SessionEvent {
    tokio::time::timeout(WAIT, session.events.recv())
        .await
        .expect("event sesi tidak datang")
        .expect("sesi sudah ditutup")
}

async fn next_frame(session: &mut Session) -> EncodedFrame {
    loop {
        if let SessionEvent::Frame(frame) = next(session).await {
            return frame;
        }
    }
}

/// Menunggu sampai `condition` benar, paling lama [`WAIT`].
async fn eventually(what: &str, condition: impl Fn() -> bool) {
    let waiting = async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::time::timeout(WAIT, waiting)
        .await
        .unwrap_or_else(|_| panic!("tidak terjadi: {what}"));
}

#[tokio::test(flavor = "multi_thread")]
async fn controller_receives_video_that_decodes_frame_by_frame() {
    let mac = TestNode::start("MacBook");
    let mut pc = host();
    let peer = pair(&mac, &mut pc.node).await;

    let mut session = peer.start_session(None, VIDEO).await.unwrap();

    assert_eq!(session.info.display_id, 1, "tanpa permintaan khusus, monitor utama yang dipakai");
    assert_eq!(session.info.displays.len(), 2);
    assert!(session.info.input_enabled);

    // Decoder hanya berhasil kalau frame datang lengkap dan berurutan.
    let mut decoder = Decoder::new().unwrap();
    for index in 0..30 {
        let frame = next_frame(&mut session).await;
        assert_eq!(frame.keyframe, index == 0, "frame {index}");
        let decoded = decoder.decode(&frame.data).unwrap().expect("frame harus terdekode");
        assert_eq!(decoded.dimensions(), (640, 360));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn host_is_told_when_a_session_starts_and_ends() {
    let mac = TestNode::start("MacBook");
    let mut pc = host();
    let peer = pair(&mac, &mut pc.node).await;

    let session = peer.start_session(None, VIDEO).await.unwrap();
    let started = pc
        .node
        .wait_for(|event| match event {
            NodeEvent::SessionStarted { peer, input_enabled } => Some((peer.name, input_enabled)),
            _ => None,
        })
        .await;
    assert_eq!(started, ("MacBook".to_owned(), true));
    assert_eq!(pc.captures_running.load(Ordering::Relaxed), 1);

    drop(session);

    let ended = pc
        .node
        .wait_for(|event| match event {
            NodeEvent::SessionEnded { peer } => Some(peer),
            _ => None,
        })
        .await;
    assert_eq!(ended, mac.node.info().fingerprint);
    eventually("capture berhenti", || pc.captures_running.load(Ordering::Relaxed) == 0).await;
    eventually("tombol dilepas", || pc.input.releases.load(Ordering::Relaxed) == 1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn input_events_reach_the_injector_in_order() {
    let mac = TestNode::start("MacBook");
    let mut pc = host();
    let peer = pair(&mac, &mut pc.node).await;
    let session = peer.start_session(None, VIDEO).await.unwrap();

    let sent = [
        InputEvent::Move { x: 0.5, y: 0.25 },
        InputEvent::Button {
            button: MouseButton::Left,
            down: true,
            x: 0.5,
            y: 0.25,
        },
        InputEvent::Key {
            code: "KeyA".to_owned(),
            down: true,
        },
        InputEvent::Scroll { dx: 0.0, dy: 48.0 },
    ];
    for event in &sent {
        session.input.send(event.clone());
    }

    eventually("semua event sampai", || pc.input.events.lock().unwrap().len() == sent.len()).await;
    let received = pc.input.events.lock().unwrap().clone();
    let expected: Vec<_> = sent.iter().cloned().map(|event| (event, 1)).collect();
    assert_eq!(received, expected);
}

#[tokio::test(flavor = "multi_thread")]
async fn selecting_another_display_switches_video_and_input_target() {
    let mac = TestNode::start("MacBook");
    let mut pc = host();
    let peer = pair(&mac, &mut pc.node).await;
    let mut session = peer.start_session(None, VIDEO).await.unwrap();
    assert_eq!(next_frame(&mut session).await.width, 640);

    session.select_display(2);

    // Frame monitor lama masih boleh lewat sebelum pemberitahuan pindah.
    loop {
        match next(&mut session).await {
            SessionEvent::DisplayChanged(display_id) => {
                assert_eq!(display_id, 2);
                break;
            }
            SessionEvent::Frame(_) => {}
            SessionEvent::Ended => panic!("sesi berakhir"),
        }
    }
    let first_of_new_display = loop {
        let frame = next_frame(&mut session).await;
        if frame.width != 640 {
            break frame;
        }
    };
    assert_eq!((first_of_new_display.width, first_of_new_display.height), (320, 240));
    assert!(first_of_new_display.keyframe, "monitor baru harus dimulai dengan keyframe");
    assert_eq!(pc.captures_running.load(Ordering::Relaxed), 1, "capture lama harus berhenti");

    session.input.send(InputEvent::Move { x: 0.0, y: 0.0 });
    eventually("event sampai", || !pc.input.events.lock().unwrap().is_empty()).await;
    assert_eq!(pc.input.events.lock().unwrap()[0].1, 2, "input dipetakan ke monitor baru");
}

#[tokio::test(flavor = "multi_thread")]
async fn selecting_an_unknown_display_keeps_the_current_one() {
    let mac = TestNode::start("MacBook");
    let mut pc = host();
    let peer = pair(&mac, &mut pc.node).await;
    let mut session = peer.start_session(None, VIDEO).await.unwrap();

    session.select_display(99);

    loop {
        if let SessionEvent::DisplayChanged(display_id) = next(&mut session).await {
            assert_eq!(display_id, 1);
            break;
        }
    }
    assert_eq!(next_frame(&mut session).await.width, 640);
}

#[tokio::test(flavor = "multi_thread")]
async fn requested_keyframe_arrives() {
    let mac = TestNode::start("MacBook");
    let mut pc = host();
    let peer = pair(&mac, &mut pc.node).await;
    let mut session = peer.start_session(None, VIDEO).await.unwrap();
    assert!(next_frame(&mut session).await.keyframe);
    assert!(!next_frame(&mut session).await.keyframe);

    session.request_keyframe();

    let found = async {
        loop {
            if next_frame(&mut session).await.keyframe {
                return;
            }
        }
    };
    tokio::time::timeout(WAIT, found).await.expect("keyframe tidak datang");
}

#[tokio::test(flavor = "multi_thread")]
async fn unpaired_device_cannot_start_a_session() {
    let mac = TestNode::start("MacBook");
    let pc = host();
    let peer = mac.node.connect(pc.node.address, None).await.unwrap();

    let result = peer.start_session(None, VIDEO).await;

    assert!(matches!(result, Err(SessionError::NotPaired)));
    assert_eq!(pc.captures_running.load(Ordering::Relaxed), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn device_that_may_not_control_cannot_start_a_session() {
    let mac = TestNode::start("MacBook");
    let mut pc = host();
    // Mac menghubungi PC tanpa mengizinkan arah sebaliknya.
    let peer = mac.node.connect(pc.node.address, None).await.unwrap();
    peer.request_pairing().await.unwrap();
    let code = pc.node.shown_code().await;
    peer.submit_code(&code, false).await.unwrap();

    let reverse = pc.node.node.connect(mac.address, Some(mac.node.info().fingerprint)).await.unwrap();
    let result = reverse.start_session(None, VIDEO).await;

    assert!(matches!(result, Err(SessionError::NotPaired)));
}

#[tokio::test(flavor = "multi_thread")]
async fn session_without_input_access_is_view_only() {
    let mac = TestNode::start("MacBook");
    let mut pc = host_with(two_displays(), false);
    let peer = pair(&mac, &mut pc.node).await;

    let mut session = peer.start_session(None, VIDEO).await.unwrap();

    assert!(!session.info.input_enabled);
    assert!(next_frame(&mut session).await.keyframe, "video tetap berjalan");
    session.input.send(InputEvent::Move { x: 0.5, y: 0.5 });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(pc.input.events.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn capture_failure_is_reported_to_the_controller() {
    let mac = TestNode::start("MacBook");
    let mut pc = host_with(Vec::new(), true);
    let peer = pair(&mac, &mut pc.node).await;

    match peer.start_session(None, VIDEO).await {
        Err(SessionError::Refused(reason)) => assert!(reason.contains("Screen Recording"), "{reason}"),
        Ok(_) => panic!("sesi seharusnya gagal dimulai"),
        Err(other) => panic!("seharusnya ditolak dengan alasan, hasilnya {other}"),
    }
    // Koneksi tetap bisa dipakai setelah penolakan.
    assert!(peer.ping().await.unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn host_can_end_a_running_session() {
    let mac = TestNode::start("MacBook");
    let mut pc = host();
    let peer = pair(&mac, &mut pc.node).await;
    let mut session = peer.start_session(None, VIDEO).await.unwrap();
    next_frame(&mut session).await;

    pc.node.node.end_incoming_session(&mac.node.info().fingerprint);

    let ended = async {
        loop {
            if matches!(next(&mut session).await, SessionEvent::Ended) {
                return;
            }
        }
    };
    tokio::time::timeout(WAIT, ended).await.expect("pengendali tidak diberi tahu");
    eventually("capture berhenti", || pc.captures_running.load(Ordering::Relaxed) == 0).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn turning_off_incoming_connections_ends_running_sessions() {
    let mac = TestNode::start("MacBook");
    let mut pc = host();
    let peer = pair(&mac, &mut pc.node).await;
    let mut session = peer.start_session(None, VIDEO).await.unwrap();
    next_frame(&mut session).await;

    pc.node.node.set_accept_incoming(false);

    eventually("capture berhenti", || pc.captures_running.load(Ordering::Relaxed) == 0).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn forgetting_the_controller_ends_its_session() {
    let mac = TestNode::start("MacBook");
    let mut pc = host();
    let peer = pair(&mac, &mut pc.node).await;
    let mut session = peer.start_session(None, VIDEO).await.unwrap();
    next_frame(&mut session).await;

    pc.node.node.forget(&mac.node.info().fingerprint).unwrap();

    eventually("capture berhenti", || pc.captures_running.load(Ordering::Relaxed) == 0).await;
}
