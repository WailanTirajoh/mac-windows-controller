//! Jendela viewer: sesi remote desktop dari sisi pengendali.
//!
//! Tiap perangkat yang dikendalikan punya satu jendela `viewer-<id>`. Sebuah
//! task memompa frame dari sesi ke webview lewat `Channel`.

use sambung_core::identity::Fingerprint;
use sambung_core::node::Session;
use sambung_core::protocol::{DisplayDesc, InputEvent, VideoParams};
use sambung_core::session::controller::{InputSender, SessionEvent};
use serde::Serialize;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tokio::sync::mpsc;

use crate::state::AppState;
use crate::viewer_frame;

const EVENT_DISPLAY_CHANGED: &str = "viewer-display-changed";
const EVENT_ENDED: &str = "viewer-ended";

/// Kualitas video bawaan: sampai 1920 piksel di sisi panjang, 60 fps, 10 Mbps.
const VIDEO: VideoParams = VideoParams {
    max_long_edge: 1920,
    max_fps: 60,
    bitrate_bps: 10_000_000,
};

type CommandResult<T> = Result<T, String>;

enum ViewerCommand {
    SelectDisplay(u32),
    RequestKeyframe,
    SetClipboardSync(bool),
}

/// Pegangan ke sesi yang sedang berjalan. Men-drop-nya mengakhiri sesi.
pub struct ViewerHandle {
    input: InputSender,
    commands: mpsc::UnboundedSender<ViewerCommand>,
    pump: tauri::async_runtime::JoinHandle<()>,
}

impl ViewerHandle {
    /// Mengakhiri sesi dan menunggu sampai benar-benar selesai, supaya pesan
    /// "berhenti" sesi lama terkirim sebelum sesi baru dimulai.
    async fn stop(self) {
        drop(self.commands);
        let _ = self.pump.await;
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayView {
    id: u32,
    width: u32,
    height: u32,
    is_primary: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    name: String,
    displays: Vec<DisplayView>,
    display_id: u32,
    input_enabled: bool,
}

impl From<&DisplayDesc> for DisplayView {
    fn from(display: &DisplayDesc) -> Self {
        Self {
            id: display.id,
            width: display.width,
            height: display.height,
            is_primary: display.is_primary,
        }
    }
}

fn parse_fingerprint(fingerprint: &str) -> CommandResult<Fingerprint> {
    fingerprint.parse().map_err(|_| "ID perangkat tidak valid".to_owned())
}

fn window_label(fingerprint: &Fingerprint) -> String {
    format!("viewer-{}", fingerprint.device_id())
}

/// Membuka jendela viewer untuk perangkat yang sudah terhubung dan di-pair.
#[tauri::command]
pub fn open_viewer(app: AppHandle, state: State<'_, AppState>, fingerprint: String) -> CommandResult<()> {
    let fingerprint = parse_fingerprint(&fingerprint)?;
    let name = state
        .peers
        .lock()
        .unwrap()
        .get(&fingerprint)
        .map(|peer| peer.info.name.clone())
        .ok_or("Koneksi ke perangkat itu sudah tertutup. Hubungkan lagi.")?;

    let label = window_label(&fingerprint);
    if let Some(window) = app.get_webview_window(&label) {
        let _ = window.set_focus();
        return Ok(());
    }

    let window = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(format!("viewer.html?peer={fingerprint}").into()))
        .title(format!("{name} - Sambung"))
        .inner_size(1280.0, 800.0)
        .min_inner_size(480.0, 320.0)
        .build()
        .map_err(|error| format!("Gagal membuka jendela: {error}"))?;

    // Menutup jendela mengakhiri sesi. Koneksinya dibiarkan, supaya file yang
    // sedang dikirim ke perangkat itu tetap selesai.
    let app = app.clone();
    window.on_window_event(move |event| {
        if matches!(event, WindowEvent::Destroyed) {
            app.state::<AppState>().viewers.lock().unwrap().remove(&fingerprint);
        }
    });
    Ok(())
}

/// Dipanggil jendela viewer: memulai sesi dan mengalirkan frame ke `on_frame`.
#[tauri::command]
pub async fn viewer_start(
    app: AppHandle,
    state: State<'_, AppState>,
    fingerprint: String,
    on_frame: Channel<InvokeResponseBody>,
) -> CommandResult<SessionView> {
    let fingerprint = parse_fingerprint(&fingerprint)?;
    let peer = state
        .peers
        .lock()
        .unwrap()
        .get(&fingerprint)
        .cloned()
        .ok_or("Koneksi ke perangkat itu sudah tertutup. Hubungkan lagi.")?;

    // Jendela yang dimuat ulang memulai sesi lagi; sesi lamanya diakhiri dulu.
    let previous = state.viewers.lock().unwrap().remove(&fingerprint);
    if let Some(previous) = previous {
        previous.stop().await;
    }

    let session = peer.start_session(None, VIDEO).await.map_err(|error| error.to_string())?;
    let view = SessionView {
        name: peer.info.name.clone(),
        displays: session.info.displays.iter().map(DisplayView::from).collect(),
        display_id: session.info.display_id,
        input_enabled: session.info.input_enabled,
    };

    let (commands, queued) = mpsc::unbounded_channel();
    let input = session.input.clone();
    let pump = tauri::async_runtime::spawn(pump(app, fingerprint, session, queued, on_frame));
    state.viewers.lock().unwrap().insert(fingerprint, ViewerHandle { input, commands, pump });
    Ok(view)
}

/// Meneruskan frame dan perubahan sesi ke jendela viewer sampai sesi berakhir.
async fn pump(
    app: AppHandle,
    fingerprint: Fingerprint,
    mut session: Session,
    mut commands: mpsc::UnboundedReceiver<ViewerCommand>,
    on_frame: Channel<InvokeResponseBody>,
) {
    let label = window_label(&fingerprint);
    loop {
        tokio::select! {
            event = session.events.recv() => match event {
                Some(SessionEvent::Frame(frame)) => {
                    // Gagal mengirim berarti jendelanya sudah ditutup.
                    if on_frame.send(InvokeResponseBody::Raw(viewer_frame::pack(&frame))).is_err() {
                        break;
                    }
                }
                Some(SessionEvent::DisplayChanged(display_id)) => {
                    let _ = app.emit_to(&label, EVENT_DISPLAY_CHANGED, display_id);
                }
                Some(SessionEvent::Ended) | None => {
                    let _ = app.emit_to(&label, EVENT_ENDED, ());
                    break;
                }
            },
            command = commands.recv() => match command {
                Some(ViewerCommand::SelectDisplay(display_id)) => session.select_display(display_id),
                Some(ViewerCommand::RequestKeyframe) => session.request_keyframe(),
                Some(ViewerCommand::SetClipboardSync(enabled)) => session.set_clipboard_sync(enabled),
                // Pegangan di-drop: jendela ditutup atau sesi diganti.
                None => break,
            },
        }
    }
}

fn with_viewer<T>(
    state: &AppState,
    fingerprint: &str,
    action: impl FnOnce(&ViewerHandle) -> T,
) -> CommandResult<T> {
    let fingerprint = parse_fingerprint(fingerprint)?;
    let viewers = state.viewers.lock().unwrap();
    let viewer = viewers.get(&fingerprint).ok_or("Sesi sudah berakhir.")?;
    Ok(action(viewer))
}

/// Event input dari jendela viewer, dikirim per kelompok tiap frame animasi.
#[tauri::command]
pub fn viewer_input(state: State<'_, AppState>, fingerprint: String, events: Vec<InputEvent>) -> CommandResult<()> {
    with_viewer(&state, &fingerprint, |viewer| {
        for event in events {
            viewer.input.send(event);
        }
    })
}

#[tauri::command]
pub fn viewer_select_display(state: State<'_, AppState>, fingerprint: String, display_id: u32) -> CommandResult<()> {
    with_viewer(&state, &fingerprint, |viewer| {
        let _ = viewer.commands.send(ViewerCommand::SelectDisplay(display_id));
    })
}

#[tauri::command]
pub fn viewer_request_keyframe(state: State<'_, AppState>, fingerprint: String) -> CommandResult<()> {
    with_viewer(&state, &fingerprint, |viewer| {
        let _ = viewer.commands.send(ViewerCommand::RequestKeyframe);
    })
}

/// Menyalakan atau mematikan sinkronisasi clipboard untuk sesi ini.
#[tauri::command]
pub fn viewer_set_clipboard_sync(state: State<'_, AppState>, fingerprint: String, enabled: bool) -> CommandResult<()> {
    with_viewer(&state, &fingerprint, |viewer| {
        let _ = viewer.commands.send(ViewerCommand::SetClipboardSync(enabled));
    })
}
