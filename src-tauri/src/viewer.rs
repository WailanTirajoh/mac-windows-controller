//! Jendela viewer: sesi remote desktop dari sisi pengendali.
//!
//! Tiap perangkat yang dikendalikan punya satu jendela `viewer-<id>`. Sebuah
//! task memompa frame dari sesi ke webview lewat `Channel`.

use std::sync::{Arc, Mutex};

use sambung_core::identity::Fingerprint;
use sambung_core::node::{Peer, Session};
use sambung_core::protocol::{DisplayDesc, InputEvent, VideoParams};
use sambung_core::session::controller::{InputSender, SessionEvent};
use serde::{Deserialize, Serialize};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tokio::sync::mpsc;

use crate::state::AppState;
use crate::viewer_frame;

const EVENT_DISPLAY_CHANGED: &str = "viewer-display-changed";
const EVENT_ENDED: &str = "viewer-ended";

/// Pilihan kualitas video di toolbar viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    /// Untuk jaringan lambat atau perangkat tujuan yang CPU-nya terbatas.
    Low,
    #[default]
    Balanced,
    /// Lebih tajam, dengan beban encoder yang jauh lebih berat di perangkat tujuan.
    Sharp,
}

impl Quality {
    const fn video(self) -> VideoParams {
        let (max_long_edge, max_fps, bitrate_bps) = match self {
            Self::Low => (1280, 30, 4_000_000),
            Self::Balanced => (1920, 60, 10_000_000),
            Self::Sharp => (2560, 60, 20_000_000),
        };
        VideoParams {
            max_long_edge,
            max_fps,
            bitrate_bps,
        }
    }
}

type CommandResult<T> = Result<T, String>;

enum ViewerCommand {
    SelectDisplay(u32),
    RequestKeyframe,
    SetClipboardSync(bool),
    SetQuality(Quality),
}

/// Pegangan ke sesi yang sedang berjalan. Men-drop-nya mengakhiri sesi.
pub struct ViewerHandle {
    /// Diganti oleh task pemompa saat sesi dimulai ulang dengan kualitas lain.
    input: Arc<Mutex<InputSender>>,
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

    let session = peer
        .start_session(None, Quality::default().video())
        .await
        .map_err(|error| error.to_string())?;
    let view = SessionView {
        name: peer.info.name.clone(),
        displays: session.info.displays.iter().map(DisplayView::from).collect(),
        display_id: session.info.display_id,
        input_enabled: session.info.input_enabled,
    };

    let (commands, queued) = mpsc::unbounded_channel();
    let input = Arc::new(Mutex::new(session.input.clone()));
    let pump = tauri::async_runtime::spawn(pump(
        app,
        peer,
        session,
        queued,
        on_frame,
        Arc::clone(&input),
    ));
    state.viewers.lock().unwrap().insert(fingerprint, ViewerHandle { input, commands, pump });
    Ok(view)
}

/// Meneruskan frame dan perubahan sesi ke jendela viewer sampai sesi berakhir.
async fn pump(
    app: AppHandle,
    peer: Arc<Peer>,
    mut session: Session,
    mut commands: mpsc::UnboundedReceiver<ViewerCommand>,
    on_frame: Channel<InvokeResponseBody>,
    input: Arc<Mutex<InputSender>>,
) {
    let label = window_label(&peer.info.fingerprint);
    let mut display_id = session.info.display_id;
    let mut clipboard_sync = true;
    loop {
        let quality = tokio::select! {
            event = session.events.recv() => match event {
                Some(SessionEvent::Frame(frame)) => {
                    // Gagal mengirim berarti jendelanya sudah ditutup.
                    if on_frame.send(InvokeResponseBody::Raw(viewer_frame::pack(&frame))).is_err() {
                        break;
                    }
                    continue;
                }
                Some(SessionEvent::DisplayChanged(changed)) => {
                    display_id = changed;
                    let _ = app.emit_to(&label, EVENT_DISPLAY_CHANGED, changed);
                    continue;
                }
                Some(SessionEvent::Ended) | None => {
                    let _ = app.emit_to(&label, EVENT_ENDED, ());
                    break;
                }
            },
            command = commands.recv() => match command {
                Some(ViewerCommand::SelectDisplay(wanted)) => {
                    session.select_display(wanted);
                    continue;
                }
                Some(ViewerCommand::RequestKeyframe) => {
                    session.request_keyframe();
                    continue;
                }
                Some(ViewerCommand::SetClipboardSync(enabled)) => {
                    clipboard_sync = enabled;
                    session.set_clipboard_sync(enabled);
                    continue;
                }
                Some(ViewerCommand::SetQuality(quality)) => quality,
                // Pegangan di-drop: jendela ditutup atau sesi diganti.
                None => break,
            },
        };

        // Kualitas diganti dengan memulai ulang sesi di layar yang sama. Sesi
        // lama diakhiri lebih dulu supaya pesan berhentinya tidak mengenai sesi baru.
        drop(session);
        session = match peer.start_session(Some(display_id), quality.video()).await {
            Ok(restarted) => restarted,
            Err(error) => {
                tracing::warn!("gagal memulai ulang sesi dengan kualitas baru: {error}");
                let _ = app.emit_to(&label, EVENT_ENDED, ());
                break;
            }
        };
        session.set_clipboard_sync(clipboard_sync);
        *input.lock().unwrap() = session.input.clone();
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
        let input = viewer.input.lock().unwrap();
        for event in events {
            input.send(event);
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

/// Mengganti kualitas video. Gambar berhenti sejenak selama sesi dimulai ulang.
#[tauri::command]
pub fn viewer_set_quality(state: State<'_, AppState>, fingerprint: String, quality: Quality) -> CommandResult<()> {
    with_viewer(&state, &fingerprint, |viewer| {
        let _ = viewer.commands.send(ViewerCommand::SetQuality(quality));
    })
}
