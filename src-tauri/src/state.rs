//! Keadaan aplikasi: node, discovery, koneksi keluar, dan sesi yang berjalan.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use sambung_core::discovery::{DiscoveredDevice, Discovery, DiscoveryEvent};
use sambung_core::identity::Fingerprint;
use sambung_core::node::{Node, NodeConfig, NodeEvent, Peer};
use sambung_core::pairing::otp::OtpPolicy;
use sambung_core::protocol::DEFAULT_PORT;
use sambung_core::session::SessionServices;
use sambung_core::trust::Os;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::transfers::{self, TransferView};
use crate::tray;
use crate::viewer::ViewerHandle;

pub const EVENT_DEVICES_CHANGED: &str = "devices-changed";
pub const EVENT_PAIRING_REQUESTED: &str = "pairing-requested";
pub const EVENT_PAIRING_FINISHED: &str = "pairing-finished";
pub const EVENT_CONTROLLERS_CHANGED: &str = "controllers-changed";
pub const EVENT_TRANSFERS_CHANGED: &str = "transfers-changed";

/// Perangkat yang sedang melihat atau mengendalikan layar ini.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControllerView {
    pub fingerprint: String,
    pub name: String,
    pub input_enabled: bool,
}

pub struct AppState {
    pub node: Node,
    pub port: u16,
    /// Perangkat yang saat ini terlihat lewat discovery.
    pub online: Mutex<HashMap<Fingerprint, DiscoveredDevice>>,
    /// Koneksi keluar yang sedang terbuka, per perangkat tujuan.
    pub peers: Mutex<HashMap<Fingerprint, Arc<Peer>>>,
    /// Sesi yang sedang ditampilkan di jendela viewer, per perangkat tujuan.
    pub viewers: Mutex<HashMap<Fingerprint, ViewerHandle>>,
    /// Perangkat yang sedang mengendalikan perangkat ini.
    pub controllers: Mutex<HashMap<Fingerprint, ControllerView>>,
    /// Transfer file yang sedang berjalan dan yang sudah selesai, terbaru di akhir.
    pub transfers: Mutex<Vec<TransferView>>,
    pub accept_incoming: AtomicBool,
    /// `None` hanya di test, yang tidak memakai mDNS.
    _discovery: Option<Discovery>,
}

impl AppState {
    pub fn new(node: Node, port: u16, discovery: Option<Discovery>) -> Self {
        Self {
            node,
            port,
            online: Mutex::new(HashMap::new()),
            peers: Mutex::new(HashMap::new()),
            viewers: Mutex::new(HashMap::new()),
            controllers: Mutex::new(HashMap::new()),
            transfers: Mutex::new(Vec::new()),
            accept_incoming: AtomicBool::new(true),
            _discovery: discovery,
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PairingRequested {
    fingerprint: String,
    name: String,
    os: Os,
    code: String,
    expires_in_secs: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PairingFinished {
    fingerprint: String,
    paired: bool,
}

/// Nama perangkat bawaan: hostname tanpa akhiran `.local`.
fn device_name() -> String {
    let hostname = gethostname::gethostname().to_string_lossy().into_owned();
    let name = hostname.strip_suffix(".local").unwrap_or(&hostname);
    if name.is_empty() {
        "Perangkat".to_owned()
    } else {
        name.to_owned()
    }
}

/// Membuka node di port bawaan, atau di port acak kalau port itu sudah dipakai
/// (misalnya oleh instance lain di mesin yang sama).
fn start_node(
    data_dir: PathBuf,
    downloads_dir: PathBuf,
    name: String,
    prefer_default_port: bool,
) -> anyhow::Result<(Node, UnboundedReceiver<NodeEvent>)> {
    let config = |port| NodeConfig {
        data_dir: data_dir.clone(),
        device_name: name.clone(),
        bind_address: SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)),
        otp_policy: OtpPolicy::default(),
        services: SessionServices::default(),
        downloads_dir: downloads_dir.clone(),
    };
    if prefer_default_port {
        if let Ok(started) = Node::start(config(DEFAULT_PORT)) {
            return Ok(started);
        }
    }
    Node::start(config(0))
}

/// Memulai node dan discovery, lalu mendaftarkannya sebagai state aplikasi.
///
/// `SAMBUNG_PROFILE=<nama>` memberi instance ini direktori data dan port
/// sendiri, untuk menjalankan dua instance di satu mesin saat pengembangan.
pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let profile = std::env::var("SAMBUNG_PROFILE").ok().filter(|profile| !profile.is_empty());
    let mut data_dir = app.path().app_data_dir().context("direktori data tidak tersedia")?;
    // File yang diterima masuk ke subfolder sendiri di folder Downloads.
    let mut downloads_dir = app
        .path()
        .download_dir()
        .context("folder Downloads tidak tersedia")?
        .join("Sambung");
    let mut name = device_name();
    if let Some(profile) = &profile {
        data_dir = data_dir.join(format!("profile-{profile}"));
        downloads_dir = downloads_dir.join(format!("profile-{profile}"));
        name = format!("{name} ({profile})");
    }

    // Node dan discovery membuat task tokio, jadi harus dibuat di dalam runtime.
    let (state, node_events, discovery_events) = tauri::async_runtime::block_on(async {
        let (node, node_events) = start_node(data_dir, downloads_dir, name, profile.is_none())?;
        let port = node.local_address()?.port();
        let info = node.info();
        tracing::info!(name = %info.name, id = %info.fingerprint.device_id(), port, "node berjalan");
        let (discovery, discovery_events) = Discovery::start(&info, port)?;
        let state = AppState::new(node, port, Some(discovery));
        anyhow::Ok((state, node_events, discovery_events))
    })?;

    app.manage(state);
    tauri::async_runtime::spawn(forward_node_events(app.clone(), node_events));
    tauri::async_runtime::spawn(forward_discovery_events(app.clone(), discovery_events));
    Ok(())
}

/// Membawa jendela utama ke depan, untuk hal yang harus dilihat pengguna.
pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

async fn forward_node_events(app: AppHandle, mut events: UnboundedReceiver<NodeEvent>) {
    while let Some(event) = events.recv().await {
        let sent = match event {
            NodeEvent::PairingRequested { peer, code, expires_in } => {
                show_main_window(&app);
                app.emit(
                    EVENT_PAIRING_REQUESTED,
                    PairingRequested {
                        fingerprint: peer.fingerprint.to_string(),
                        name: peer.name,
                        os: peer.os,
                        code,
                        expires_in_secs: expires_in.as_secs(),
                    },
                )
            }
            NodeEvent::PairingFinished { peer, paired } => app.emit(
                EVENT_PAIRING_FINISHED,
                PairingFinished {
                    fingerprint: peer.to_string(),
                    paired,
                },
            ),
            NodeEvent::TrustChanged => app.emit(EVENT_DEVICES_CHANGED, ()),
            NodeEvent::SessionStarted { peer, input_enabled } => {
                tracing::info!(name = %peer.name, input_enabled, "sesi masuk dimulai");
                app.state::<AppState>().controllers.lock().unwrap().insert(
                    peer.fingerprint,
                    ControllerView {
                        fingerprint: peer.fingerprint.to_string(),
                        name: peer.name,
                        input_enabled,
                    },
                );
                tray::refresh(&app);
                app.emit(EVENT_CONTROLLERS_CHANGED, ())
            }
            NodeEvent::SessionEnded { peer } => {
                tracing::info!(id = %peer.device_id(), "sesi masuk berakhir");
                app.state::<AppState>().controllers.lock().unwrap().remove(&peer);
                tray::refresh(&app);
                app.emit(EVENT_CONTROLLERS_CHANGED, ())
            }
            NodeEvent::Transfer(event) => {
                transfers::apply(&mut app.state::<AppState>().transfers.lock().unwrap(), event);
                app.emit(EVENT_TRANSFERS_CHANGED, ())
            }
        };
        if let Err(error) = sent {
            tracing::warn!("gagal mengirim event ke UI: {error}");
        }
    }
}

async fn forward_discovery_events(app: AppHandle, mut events: UnboundedReceiver<DiscoveryEvent>) {
    while let Some(event) = events.recv().await {
        {
            let state = app.state::<AppState>();
            let mut online = state.online.lock().unwrap();
            match event {
                DiscoveryEvent::Found(device) => {
                    // mDNS mengulang pengumuman yang sama; hanya perubahan yang diteruskan.
                    if online.get(&device.info.fingerprint) == Some(&device) {
                        continue;
                    }
                    tracing::info!(name = %device.info.name, addresses = ?device.addresses, "perangkat terlihat");
                    online.insert(device.info.fingerprint, device);
                }
                DiscoveryEvent::Lost(fingerprint) => {
                    tracing::info!(id = %fingerprint.device_id(), "perangkat hilang");
                    online.remove(&fingerprint);
                }
            }
        }
        let _ = app.emit(EVENT_DEVICES_CHANGED, ());
    }
}
