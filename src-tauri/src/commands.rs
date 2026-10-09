//! Perintah yang dipanggil UI lewat `invoke`.
//!
//! Logikanya ada di fungsi biasa yang menerima `&AppState`, supaya bisa dites
//! tanpa Tauri; fungsi ber-`#[tauri::command]` hanya meneruskan argumen.

use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use sambung_core::identity::Fingerprint;
use sambung_core::node::{ConnectError, DeviceInfo, Node, Peer};
use sambung_core::protocol::DEFAULT_PORT;
use sambung_core::trust::Os;
use serde::Serialize;
use tauri::State;
use tokio::task::JoinSet;

use crate::state::AppState;

/// Batas waktu mencoba satu alamat.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);

type CommandResult<T> = Result<T, String>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelfView {
    fingerprint: String,
    id: String,
    name: String,
    os: Os,
    port: u16,
    accept_incoming: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceView {
    fingerprint: String,
    id: String,
    name: String,
    os: Os,
    /// Terlihat lewat discovery saat ini.
    online: bool,
    /// Sudah tersimpan sebagai perangkat terpercaya.
    paired: bool,
    /// Boleh mengendalikan perangkat ini.
    allow_incoming: bool,
}

impl DeviceView {
    fn new(info: &DeviceInfo, online: bool, paired: bool, allow_incoming: bool) -> Self {
        Self {
            fingerprint: info.fingerprint.to_string(),
            id: info.fingerprint.device_id(),
            name: info.name.clone(),
            os: info.os,
            online,
            paired,
            allow_incoming,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum ConnectOutcome {
    /// Sudah di-pair; siap dipakai.
    Connected { device: DeviceView },
    /// Perangkat tujuan menampilkan kode yang harus diketik di sini.
    NeedsCode { device: DeviceView, expires_in_secs: u64 },
}

fn parse_fingerprint(fingerprint: &str) -> CommandResult<Fingerprint> {
    fingerprint.parse().map_err(|_| "ID perangkat tidak valid".to_owned())
}

/// Gabungan perangkat tersimpan dan perangkat yang terlihat di jaringan.
fn devices(state: &AppState) -> Vec<DeviceView> {
    let online = state.online.lock().unwrap();
    let trusted = state.node.trusted_devices();

    let mut devices: Vec<DeviceView> = trusted
        .iter()
        .map(|device| {
            // Nama dari discovery lebih baru daripada yang tersimpan.
            let info = online.get(&device.fingerprint).map_or_else(
                || DeviceInfo {
                    fingerprint: device.fingerprint,
                    name: device.name.clone(),
                    os: device.os,
                },
                |found| found.info.clone(),
            );
            DeviceView::new(&info, online.contains_key(&device.fingerprint), true, device.allow_incoming)
        })
        .collect();
    devices.extend(
        online
            .values()
            .filter(|found| trusted.iter().all(|device| device.fingerprint != found.info.fingerprint))
            .map(|found| DeviceView::new(&found.info, true, false, false)),
    );

    devices.sort_by(|a, b| {
        (b.online, b.paired)
            .cmp(&(a.online, a.paired))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.fingerprint.cmp(&b.fingerprint))
    });
    devices
}

/// Mencoba semua alamat sekaligus dan memakai yang pertama berhasil. Alamat
/// yang tidak terjangkau (antarmuka virtual, VPN) jadi tidak menambah waktu tunggu.
async fn connect_any(node: &Node, addresses: Vec<SocketAddr>, expected: Fingerprint) -> CommandResult<Peer> {
    let mut attempts = JoinSet::new();
    for address in addresses {
        let node = node.clone();
        attempts.spawn(async move {
            tokio::time::timeout(CONNECT_TIMEOUT, node.connect(address, Some(expected)))
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("tidak ada jawaban dari {address}").into()))
        });
    }

    let mut failure = None;
    while let Some(attempt) = attempts.join_next().await {
        match attempt {
            // Sisa percobaan dibatalkan saat `attempts` di-drop.
            Ok(Ok(peer)) => return Ok(peer),
            // Identitas yang berubah lebih penting dilaporkan daripada timeout di alamat lain.
            Ok(Err(error @ ConnectError::IdentityChanged)) => failure = Some(error.to_string()),
            Ok(Err(error)) => failure = failure.or(Some(error.to_string())),
            Err(error) => failure = failure.or(Some(error.to_string())),
        }
    }
    Err(format!(
        "Tidak bisa terhubung: {}",
        failure.unwrap_or_else(|| "tidak ada alamat".to_owned())
    ))
}

/// Menyimpan koneksi dan, kalau belum di-pair, meminta perangkat tujuan menampilkan kode.
async fn finish_connect(state: &AppState, peer: Peer) -> CommandResult<ConnectOutcome> {
    let peer = Arc::new(peer);
    let fingerprint = peer.info.fingerprint;
    state.peers.lock().unwrap().insert(fingerprint, Arc::clone(&peer));

    let online = state.online.lock().unwrap().contains_key(&fingerprint);
    if peer.is_paired() {
        let allow_incoming = state
            .node
            .trusted_devices()
            .iter()
            .any(|device| device.fingerprint == fingerprint && device.allow_incoming);
        return Ok(ConnectOutcome::Connected {
            device: DeviceView::new(&peer.info, online, true, allow_incoming),
        });
    }

    match peer.request_pairing().await {
        Ok(expires_in) => Ok(ConnectOutcome::NeedsCode {
            device: DeviceView::new(&peer.info, online, false, false),
            expires_in_secs: expires_in.as_secs(),
        }),
        Err(error) => {
            state.peers.lock().unwrap().remove(&fingerprint);
            Err(error.to_string())
        }
    }
}

async fn connect_to_device(state: &AppState, fingerprint: &str) -> CommandResult<ConnectOutcome> {
    let fingerprint = parse_fingerprint(fingerprint)?;

    let mut addresses: Vec<SocketAddr> = state
        .online
        .lock()
        .unwrap()
        .get(&fingerprint)
        .map(|found| found.addresses.clone())
        .unwrap_or_default();
    let last_address = state
        .node
        .trusted_devices()
        .into_iter()
        .find(|device| device.fingerprint == fingerprint)
        .and_then(|device| device.last_address);
    if let Some(last_address) = last_address.filter(|address| !addresses.contains(address)) {
        addresses.push(last_address);
    }
    if addresses.is_empty() {
        return Err("Perangkat itu sedang tidak terlihat di jaringan.".to_owned());
    }

    let peer = connect_any(&state.node, addresses, fingerprint).await?;
    finish_connect(state, peer).await
}

/// `address` berbentuk `ip` atau `ip:port`; tanpa port berarti port bawaan.
async fn connect_to_address(state: &AppState, address: &str) -> CommandResult<ConnectOutcome> {
    let address = address.trim();
    let with_port = if address.contains(':') {
        address.to_owned()
    } else {
        format!("{address}:{DEFAULT_PORT}")
    };
    let resolved = tokio::net::lookup_host(&with_port)
        .await
        .ok()
        .and_then(|mut found| found.find(SocketAddr::is_ipv4))
        .ok_or_else(|| format!("Alamat \"{address}\" tidak valid."))?;

    let attempt = tokio::time::timeout(CONNECT_TIMEOUT, state.node.connect(resolved, None)).await;
    match attempt {
        Ok(Ok(peer)) => finish_connect(state, peer).await,
        Ok(Err(error)) => Err(format!("Tidak bisa terhubung: {error}")),
        Err(_) => Err(format!("Tidak ada jawaban dari {resolved}.")),
    }
}

async fn submit_code(state: &AppState, fingerprint: &str, code: &str, allow_reverse: bool) -> CommandResult<()> {
    let fingerprint = parse_fingerprint(fingerprint)?;
    let code: String = code.chars().filter(|character| !character.is_whitespace()).collect();
    if code.len() != 6 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Kode terdiri dari 6 angka.".to_owned());
    }

    let peer = state
        .peers
        .lock()
        .unwrap()
        .get(&fingerprint)
        .cloned()
        .ok_or("Koneksi ke perangkat itu sudah tertutup. Hubungkan lagi.")?;
    peer.submit_code(&code, allow_reverse)
        .await
        .map_err(|error| error.to_string())
}

fn forget(state: &AppState, fingerprint: &str) -> CommandResult<()> {
    let fingerprint = parse_fingerprint(fingerprint)?;
    state.peers.lock().unwrap().remove(&fingerprint);
    state
        .node
        .forget(&fingerprint)
        .map(|_| ())
        .map_err(|error| format!("{error:#}"))
}

#[tauri::command]
pub fn get_self(state: State<'_, AppState>) -> SelfView {
    let info = state.node.info();
    SelfView {
        fingerprint: info.fingerprint.to_string(),
        id: info.fingerprint.device_id(),
        name: info.name,
        os: info.os,
        port: state.port,
        accept_incoming: state.accept_incoming.load(Ordering::Relaxed),
    }
}

#[tauri::command]
pub fn list_devices(state: State<'_, AppState>) -> Vec<DeviceView> {
    devices(&state)
}

/// Menghubungi perangkat dari daftar.
#[tauri::command]
pub async fn connect_device(state: State<'_, AppState>, fingerprint: String) -> CommandResult<ConnectOutcome> {
    connect_to_device(&state, &fingerprint).await
}

/// Menghubungi perangkat lewat alamat yang diketik pengguna.
#[tauri::command]
pub async fn connect_address(state: State<'_, AppState>, address: String) -> CommandResult<ConnectOutcome> {
    connect_to_address(&state, &address).await
}

#[tauri::command]
pub async fn submit_pairing_code(
    state: State<'_, AppState>,
    fingerprint: String,
    code: String,
    allow_reverse: bool,
) -> CommandResult<()> {
    submit_code(&state, &fingerprint, &code, allow_reverse).await
}

/// Menutup koneksi keluar ke sebuah perangkat, misalnya saat dialog kode dibatalkan.
#[tauri::command]
pub fn disconnect_device(state: State<'_, AppState>, fingerprint: String) -> CommandResult<()> {
    let fingerprint = parse_fingerprint(&fingerprint)?;
    state.peers.lock().unwrap().remove(&fingerprint);
    Ok(())
}

#[tauri::command]
pub fn forget_device(state: State<'_, AppState>, fingerprint: String) -> CommandResult<()> {
    forget(&state, &fingerprint)
}

/// Menolak permintaan pairing yang kodenya sedang tampil.
#[tauri::command]
pub fn reject_pairing(state: State<'_, AppState>, fingerprint: String) -> CommandResult<()> {
    state.node.reject_pairing(&parse_fingerprint(&fingerprint)?);
    Ok(())
}

#[tauri::command]
pub fn set_accept_incoming(state: State<'_, AppState>, accept: bool) {
    state.accept_incoming.store(accept, Ordering::Relaxed);
    state.node.set_accept_incoming(accept);
}

#[cfg(test)]
mod tests {
    use sambung_core::discovery::DiscoveredDevice;
    use sambung_core::node::{NodeConfig, NodeEvent};
    use sambung_core::pairing::otp::OtpPolicy;
    use tempfile::TempDir;
    use tokio::sync::mpsc::UnboundedReceiver;

    use super::*;

    struct TestApp {
        state: AppState,
        events: UnboundedReceiver<NodeEvent>,
        address: SocketAddr,
        _data: TempDir,
    }

    impl TestApp {
        fn start(name: &str) -> Self {
            let data = TempDir::new().unwrap();
            let (node, events) = Node::start(NodeConfig {
                data_dir: data.path().to_owned(),
                device_name: name.to_owned(),
                bind_address: "127.0.0.1:0".parse().unwrap(),
                otp_policy: OtpPolicy::default(),
            })
            .unwrap();
            let address = node.local_address().unwrap();
            Self {
                state: AppState::new(node, address.port(), None),
                events,
                address,
                _data: data,
            }
        }

        fn fingerprint(&self) -> String {
            self.state.node.info().fingerprint.to_string()
        }

        /// Menandai `other` sebagai terlihat lewat discovery di alamat yang diberikan.
        fn sees(&self, other: &TestApp, addresses: Vec<SocketAddr>) {
            let info = other.state.node.info();
            self.state
                .online
                .lock()
                .unwrap()
                .insert(info.fingerprint, DiscoveredDevice { info, addresses });
        }

        async fn shown_code(&mut self) -> String {
            loop {
                let event = tokio::time::timeout(Duration::from_secs(5), self.events.recv())
                    .await
                    .expect("kode tidak muncul")
                    .unwrap();
                if let NodeEvent::PairingRequested { code, .. } = event {
                    return code;
                }
            }
        }
    }

    #[tokio::test]
    async fn discovered_device_pairs_then_reconnects_without_a_code() {
        let mac = TestApp::start("MacBook");
        let mut pc = TestApp::start("PC");
        mac.sees(&pc, vec![pc.address]);
        assert_eq!(devices(&mac.state)[0].name, "PC");
        assert!(!devices(&mac.state)[0].paired);

        let outcome = connect_to_device(&mac.state, &pc.fingerprint()).await.unwrap();
        assert!(matches!(outcome, ConnectOutcome::NeedsCode { expires_in_secs: 120, .. }));
        let code = pc.shown_code().await;

        // Kode diketik dengan spasi di tengah, seperti yang tampil di layar.
        let spaced = format!("{} {}", &code[..3], &code[3..]);
        submit_code(&mac.state, &pc.fingerprint(), &spaced, true).await.unwrap();

        let listed = devices(&mac.state);
        assert!(listed[0].paired && listed[0].online && listed[0].allow_incoming);
        assert_eq!(devices(&pc.state)[0].name, "MacBook");

        let again = connect_to_device(&mac.state, &pc.fingerprint()).await.unwrap();
        assert!(matches!(again, ConnectOutcome::Connected { .. }));
    }

    #[tokio::test]
    async fn manual_address_pairs_and_last_address_allows_reconnect_without_discovery() {
        let mac = TestApp::start("MacBook");
        let mut pc = TestApp::start("PC");

        let outcome = connect_to_address(&mac.state, &pc.address.to_string()).await.unwrap();
        assert!(matches!(outcome, ConnectOutcome::NeedsCode { .. }));
        let code = pc.shown_code().await;
        submit_code(&mac.state, &pc.fingerprint(), &code, true).await.unwrap();

        // PC tidak pernah terlihat lewat discovery; alamat tersimpan yang dipakai.
        let listed = devices(&mac.state);
        assert!(listed[0].paired && !listed[0].online);
        let again = connect_to_device(&mac.state, &pc.fingerprint()).await.unwrap();
        assert!(matches!(again, ConnectOutcome::Connected { .. }));
    }

    #[tokio::test]
    async fn unreachable_addresses_do_not_prevent_connecting() {
        let mac = TestApp::start("MacBook");
        let pc = TestApp::start("PC");
        // Alamat pertama tidak dijawab siapa pun (TEST-NET-1), alamat kedua benar.
        mac.sees(&pc, vec!["192.0.2.1:47823".parse().unwrap(), pc.address]);

        let started = std::time::Instant::now();
        let outcome = connect_to_device(&mac.state, &pc.fingerprint()).await.unwrap();

        assert!(matches!(outcome, ConnectOutcome::NeedsCode { .. }));
        assert!(started.elapsed() < CONNECT_TIMEOUT, "tidak boleh menunggu alamat yang mati");
    }

    #[tokio::test]
    async fn wrong_device_at_advertised_address_is_reported_as_identity_change() {
        let mac = TestApp::start("MacBook");
        let pc = TestApp::start("PC");
        let impostor = TestApp::start("PC");
        // Record discovery mengaku sebagai PC tetapi alamatnya milik perangkat lain.
        mac.sees(&pc, vec![impostor.address]);

        let error = connect_to_device(&mac.state, &pc.fingerprint()).await.unwrap_err();

        assert!(error.contains("identitas perangkat tujuan berbeda"), "{error}");
        assert!(mac.state.peers.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn malformed_code_is_rejected_before_reaching_the_device() {
        let mac = TestApp::start("MacBook");
        let pc = TestApp::start("PC");
        connect_to_address(&mac.state, &pc.address.to_string()).await.unwrap();

        for code in ["12345", "1234567", "12a456", ""] {
            let error = submit_code(&mac.state, &pc.fingerprint(), code, true).await.unwrap_err();
            assert_eq!(error, "Kode terdiri dari 6 angka.");
        }
    }

    #[tokio::test]
    async fn forgetting_a_device_requires_a_new_code() {
        let mac = TestApp::start("MacBook");
        let mut pc = TestApp::start("PC");
        mac.sees(&pc, vec![pc.address]);
        connect_to_device(&mac.state, &pc.fingerprint()).await.unwrap();
        let code = pc.shown_code().await;
        submit_code(&mac.state, &pc.fingerprint(), &code, true).await.unwrap();

        forget(&mac.state, &pc.fingerprint()).unwrap();

        assert!(!devices(&mac.state)[0].paired);
        let outcome = connect_to_device(&mac.state, &pc.fingerprint()).await.unwrap();
        assert!(matches!(outcome, ConnectOutcome::NeedsCode { .. }));
    }

    #[tokio::test]
    async fn invalid_address_is_reported() {
        let mac = TestApp::start("MacBook");
        let error = connect_to_address(&mac.state, "bukan alamat").await.unwrap_err();
        assert_eq!(error, "Alamat \"bukan alamat\" tidak valid.");
    }

    /// Bentuk JSON harus cocok dengan tipe `ConnectOutcome` dan `Device` di `ui/src/api.ts`.
    #[test]
    fn connect_outcome_serializes_in_the_shape_the_ui_expects() {
        let info = DeviceInfo {
            fingerprint: "ab".repeat(32).parse().unwrap(),
            name: "PC".to_owned(),
            os: Os::Windows,
        };
        let outcome = ConnectOutcome::NeedsCode {
            device: DeviceView::new(&info, true, false, false),
            expires_in_secs: 120,
        };

        let json = serde_json::to_value(outcome).unwrap();

        assert_eq!(
            json,
            serde_json::json!({
                "status": "needs_code",
                "expiresInSecs": 120,
                "device": {
                    "fingerprint": "ab".repeat(32),
                    "id": "ab".repeat(8),
                    "name": "PC",
                    "os": "windows",
                    "online": true,
                    "paired": false,
                    "allowIncoming": false,
                },
            })
        );
    }
}
