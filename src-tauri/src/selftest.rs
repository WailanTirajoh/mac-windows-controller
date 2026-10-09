//! Uji mandiri video: `SAMBUNG_SELFTEST=video` membuka jendela viewer yang
//! diberi stream sintetis, mencetak laporan decode ke stdout, lalu keluar.
//!
//! Berguna untuk memastikan webview di sebuah mesin bisa men-decode H.264
//! tanpa perlu perangkat kedua.

use std::time::{Duration, Instant};

use sambung_core::video::openh264_enc::OpenH264Encoder;
use sambung_core::video::testsrc::scrolling_document;
use sambung_core::video::{EncoderSettings, VideoEncoder};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, WebviewUrl, WebviewWindowBuilder};

use crate::viewer_frame;

const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1248;
const FPS: u32 = 60;
const FRAMES: u32 = 300;
const REPORT_TIMEOUT: Duration = Duration::from_secs(30);

pub fn requested() -> bool {
    std::env::var("SAMBUNG_SELFTEST").is_ok_and(|value| value == "video")
}

pub fn open_window(app: &AppHandle) -> tauri::Result<()> {
    WebviewWindowBuilder::new(app, "viewer-selftest", WebviewUrl::App("viewer.html?selftest".into()))
        .title("Sambung: uji video")
        .inner_size(960.0, 640.0)
        .build()?;

    // Kalau webview tidak pernah melapor (mis. halaman gagal dimuat), jangan menggantung.
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(REPORT_TIMEOUT);
        println!("SELFTEST {{\"errors\":[\"tidak ada laporan dari webview\"]}}");
        app.exit(1);
    });
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

#[tauri::command]
pub fn selftest_report(app: AppHandle, report: String) {
    println!("SELFTEST {report}");
    app.exit(0);
}
