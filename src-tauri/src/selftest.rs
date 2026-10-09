//! Uji mandiri, dijalankan dengan variabel lingkungan `SAMBUNG_SELFTEST`.
//! Hasilnya dicetak ke stdout sebagai satu baris `SELFTEST {json}`, lalu
//! aplikasi keluar.
//!
//! - `video`: jendela viewer diberi stream sintetis langsung dari encoder.
//!   Memastikan webview di mesin ini bisa men-decode H.264.
//! - `session`: dua node dijalankan di proses ini (tujuan dengan layar
//!   sintetis, dan pengendali), di-pair, lalu jendela viewer sungguhan dibuka.
//!   Menguji jalur lengkap viewer → QUIC → sesi tanpa perangkat kedua dan
//!   tanpa izin Screen Recording.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use sambung_core::capture::synthetic::SyntheticCapture;
use sambung_core::input::recording::RecordingInput;
use sambung_core::node::{Node, NodeConfig, NodeEvent};
use sambung_core::pairing::otp::OtpPolicy;
use sambung_core::session::SessionServices;
use sambung_core::video::openh264_enc::OpenH264Encoder;
use sambung_core::video::testsrc::scrolling_document;
use sambung_core::video::{EncoderSettings, VideoEncoder};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::state::AppState;
use crate::viewer_frame;

const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1248;
const FPS: u32 = 60;
const FRAMES: u32 = 300;
const REPORT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Video,
    Session,
}

pub fn requested() -> Option<Mode> {
    match std::env::var("SAMBUNG_SELFTEST").ok()?.as_str() {
        "video" => Some(Mode::Video),
        "session" => Some(Mode::Session),
        _ => None,
    }
}

/// Yang perlu tetap hidup selama uji sesi, dan yang diperiksa di akhirnya.
pub struct SessionFixture {
    /// Node tujuan; berhenti kalau di-drop.
    _host: Node,
    input: RecordingInput,
    data_dir: PathBuf,
}

/// Kalau webview tidak pernah melapor (mis. halaman gagal dimuat), jangan menggantung.
fn exit_if_no_report(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(REPORT_TIMEOUT);
        println!("SELFTEST {{\"errors\":[\"tidak ada laporan dari webview\"]}}");
        app.exit(1);
    });
}

pub fn start(app: &AppHandle, mode: Mode) -> anyhow::Result<()> {
    match mode {
        Mode::Video => {
            WebviewWindowBuilder::new(app, "viewer-selftest", WebviewUrl::App("viewer.html?selftest=video".into()))
                .title("Sambung: uji video")
                .inner_size(960.0, 640.0)
                .build()?;
        }
        Mode::Session => start_session(app)?,
    }
    exit_if_no_report(app);
    Ok(())
}

fn start_session(app: &AppHandle) -> anyhow::Result<()> {
    let data_dir = std::env::temp_dir().join(format!("sambung-selftest-{}", std::process::id()));
    let input = RecordingInput::default();
    let node = |name: &str, services| {
        Node::start(NodeConfig {
            data_dir: data_dir.join(name),
            device_name: format!("Uji ({name})"),
            bind_address: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            otp_policy: OtpPolicy::default(),
            services,
        })
    };

    let (state, host, fingerprint) = tauri::async_runtime::block_on(async {
        let (host, mut host_events) = node(
            "tujuan",
            SessionServices {
                capture: Arc::new(SyntheticCapture::two_displays()),
                input: Arc::new(input.clone()),
            },
        )?;
        let (controller, _events) = node("pengendali", SessionServices::default())?;

        let peer = controller.connect(host.local_address()?, None).await?;
        peer.request_pairing().await?;
        let code = loop {
            match host_events.recv().await.context("node tujuan berhenti")? {
                NodeEvent::PairingRequested { code, .. } => break code,
                _ => continue,
            }
        };
        peer.submit_code(&code, false).await?;

        let fingerprint = peer.info.fingerprint;
        let state = AppState::new(controller.clone(), controller.local_address()?.port(), None);
        state.peers.lock().unwrap().insert(fingerprint, Arc::new(peer));
        anyhow::Ok((state, host, fingerprint))
    })?;

    app.manage(state);
    app.manage(SessionFixture {
        _host: host,
        input,
        data_dir,
    });

    let url = format!("viewer.html?peer={fingerprint}&selftest=session");
    WebviewWindowBuilder::new(app, format!("viewer-{}", fingerprint.device_id()), WebviewUrl::App(url.into()))
        .title("Sambung: uji sesi")
        .inner_size(1280.0, 800.0)
        .build()?;
    Ok(())
}

#[tauri::command]
pub fn selftest_stream(on_frame: Channel<InvokeResponseBody>) {
    std::thread::spawn(move || {
        if let Err(error) = stream(&on_frame) {
            eprintln!("SELFTEST gagal mengirim stream: {error:#}");
        }
        // Pesan kosong menandai akhir stream.
        let _ = on_frame.send(InvokeResponseBody::Raw(Vec::new()));
    });
}

fn stream(on_frame: &Channel<InvokeResponseBody>) -> anyhow::Result<()> {
    let mut encoder = OpenH264Encoder::new(EncoderSettings {
        bitrate_bps: 10_000_000,
        max_fps: FPS,
    })?;
    let interval = Duration::from_secs(1) / FPS;
    let started = Instant::now();

    for index in 0..FRAMES {
        let frame = scrolling_document(WIDTH, HEIGHT, index);
        if let Some(encoded) = encoder.encode(&frame)? {
            on_frame.send(InvokeResponseBody::Raw(viewer_frame::pack(&encoded)))?;
        }
        if let Some(wait) = (interval * (index + 1)).checked_sub(started.elapsed()) {
            std::thread::sleep(wait);
        }
    }
    Ok(())
}

/// Dipanggil webview di akhir uji. Untuk uji sesi, event input yang sampai ke
/// node tujuan ikut dilaporkan.
#[tauri::command]
pub fn selftest_report(app: AppHandle, report: String) {
    println!("SELFTEST {report}");
    if let Some(fixture) = app.try_state::<SessionFixture>() {
        println!("SELFTEST_INPUT {:?}", fixture.input.events());
        let _ = std::fs::remove_dir_all(&fixture.data_dir);
    }
    app.exit(0);
}
