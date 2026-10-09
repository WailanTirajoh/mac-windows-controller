//! Node: satu instalasi Sambung di jaringan. Menerima koneksi dari perangkat
//! lain (`responder`) dan memulai koneksi keluar (`peer`).

mod peer;
mod responder;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use serde::Serialize;
use tokio::sync::mpsc;

pub use self::peer::{Peer, Session, SessionError};
use crate::identity::{Fingerprint, Identity};
use crate::net;
use crate::pairing::otp::{OtpManager, OtpPolicy};
use crate::protocol::{ControlMessage, PairFailure, PROTOCOL_VERSION};
use crate::session::SessionServices;
use crate::trust::{Os, TrustStore, TrustedDevice};

const TRUST_FILE: &str = "trusted-devices.json";
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

const CLOSE_NORMAL: u32 = 0;
const CLOSE_NOT_PAIRED: u32 = 1;
const CLOSE_PROTOCOL: u32 = 2;
const CLOSE_IDENTITY: u32 = 3;

pub struct NodeConfig {
    /// Tempat identitas dan daftar perangkat terpercaya disimpan.
    pub data_dir: PathBuf,
    pub device_name: String,
    pub bind_address: SocketAddr,
    pub otp_policy: OtpPolicy,
    /// Capture layar dan injeksi input yang dipakai saat perangkat ini dikendalikan.
    pub services: SessionServices,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceInfo {
    pub fingerprint: Fingerprint,
    pub name: String,
    pub os: Os,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeEvent {
    /// Perangkat lain meminta pairing. `code` harus ditampilkan ke pengguna.
    PairingRequested {
        peer: DeviceInfo,
        code: String,
        expires_in: Duration,
    },
    /// Permintaan pairing dari `peer` sudah selesai; dialog kode bisa ditutup.
    PairingFinished { peer: Fingerprint, paired: bool },
    /// Daftar perangkat terpercaya berubah.
    TrustChanged,
    /// `peer` mulai melihat (dan, kalau `input_enabled`, mengendalikan) layar ini.
    SessionStarted { peer: DeviceInfo, input_enabled: bool },
    SessionEnded { peer: Fingerprint },
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("identitas perangkat tujuan berbeda dari yang tersimpan; koneksi dibatalkan")]
    IdentityChanged,
    #[error("alamat itu adalah perangkat ini sendiri")]
    SelfConnection,
    #[error("versi protokol perangkat tujuan ({theirs}) tidak cocok dengan versi ini ({PROTOCOL_VERSION})")]
    Incompatible { theirs: u32 },
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum PairError {
    #[error(transparent)]
    Rejected(#[from] PairFailure),
    #[error("perangkat tujuan tidak bisa membuktikan bahwa ia mengetahui kodenya; pairing dibatalkan")]
    Unverified,
    #[error("koneksi bermasalah saat pairing: {0:#}")]
    Connection(#[from] anyhow::Error),
}

struct Inner {
    identity: Identity,
    name: String,
    trust: TrustStore,
    endpoint: quinn::Endpoint,
    otp: Mutex<OtpManager>,
    otp_ttl: Duration,
    accept_incoming: AtomicBool,
    services: SessionServices,
    /// Koneksi dari perangkat yang sedang mengendalikan perangkat ini.
    controllers: Mutex<HashMap<Fingerprint, quinn::Connection>>,
    events: mpsc::UnboundedSender<NodeEvent>,
}

impl Inner {
    fn emit(&self, event: NodeEvent) {
        // Penerima sudah tidak ada berarti aplikasi sedang ditutup.
        let _ = self.events.send(event);
    }

    fn hello(&self, trusts_peer: bool) -> ControlMessage {
        ControlMessage::Hello {
            protocol: PROTOCOL_VERSION,
            name: self.name.clone(),
            os: Os::current(),
            trusts_peer,
        }
    }

    /// Apakah `peer` boleh mengendalikan perangkat ini tanpa pairing.
    fn allows_incoming(&self, peer: &Fingerprint) -> bool {
        self.accept_incoming.load(Ordering::Relaxed)
            && self.trust.get(peer).is_some_and(|device| device.allow_incoming)
    }

    fn remember(&self, peer: &DeviceInfo, allow_incoming: bool, address: SocketAddr) -> anyhow::Result<()> {
        let paired_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        self.trust.upsert(TrustedDevice {
            fingerprint: peer.fingerprint,
            name: peer.name.clone(),
            os: peer.os,
            allow_incoming,
            paired_at,
            last_address: Some(address),
        })?;
        self.emit(NodeEvent::TrustChanged);
        Ok(())
    }

    /// Memperbarui nama dan alamat perangkat tersimpan kalau berubah.
    fn refresh(&self, known: &TrustedDevice, peer: &DeviceInfo, address: SocketAddr) {
        if known.name == peer.name && known.os == peer.os && known.last_address == Some(address) {
            return;
        }
        let updated = TrustedDevice {
            name: peer.name.clone(),
            os: peer.os,
            last_address: Some(address),
            ..known.clone()
        };
        match self.trust.upsert(updated) {
            Ok(()) => self.emit(NodeEvent::TrustChanged),
            Err(error) => tracing::warn!("gagal memperbarui perangkat tersimpan: {error:#}"),
        }
    }
}

#[derive(Clone)]
pub struct Node {
    inner: Arc<Inner>,
}

impl Node {
    /// Membuka port dan mulai menerima koneksi. Harus dipanggil dari dalam
    /// runtime tokio.
    pub fn start(config: NodeConfig) -> anyhow::Result<(Self, mpsc::UnboundedReceiver<NodeEvent>)> {
        let identity = Identity::load_or_create(&config.data_dir)?;
        let trust = TrustStore::load(config.data_dir.join(TRUST_FILE))?;
        let endpoint = net::bind(&identity, config.bind_address)?;
        let (events, receiver) = mpsc::unbounded_channel();

        let inner = Arc::new(Inner {
            identity,
            name: config.device_name,
            trust,
            endpoint,
            otp: Mutex::new(OtpManager::new(config.otp_policy)),
            otp_ttl: config.otp_policy.ttl,
            accept_incoming: AtomicBool::new(true),
            services: config.services,
            controllers: Mutex::new(HashMap::new()),
            events,
        });

        let accepting = Arc::clone(&inner);
        tokio::spawn(async move {
            while let Some(incoming) = accepting.endpoint.accept().await {
                let inner = Arc::clone(&accepting);
                tokio::spawn(async move {
                    if let Err(error) = responder::serve(inner, incoming).await {
                        tracing::debug!("koneksi masuk berakhir: {error:#}");
                    }
                });
            }
        });

        Ok((Self { inner }, receiver))
    }

    pub fn local_address(&self) -> anyhow::Result<SocketAddr> {
        self.inner.endpoint.local_addr().context("alamat lokal tidak tersedia")
    }

    pub fn info(&self) -> DeviceInfo {
        DeviceInfo {
            fingerprint: self.inner.identity.fingerprint(),
            name: self.inner.name.clone(),
            os: Os::current(),
        }
    }

    pub fn trusted_devices(&self) -> Vec<TrustedDevice> {
        self.inner.trust.list()
    }

    /// Menghapus perangkat dari daftar terpercaya dan memutus sesinya kalau
    /// sedang mengendalikan perangkat ini.
    pub fn forget(&self, fingerprint: &Fingerprint) -> anyhow::Result<bool> {
        let removed = self.inner.trust.remove(fingerprint)?;
        if removed {
            self.end_incoming_session(fingerprint);
            self.inner.emit(NodeEvent::TrustChanged);
        }
        Ok(removed)
    }

    /// Menolak permintaan pairing dari `peer`: kode yang sedang tampil dicabut.
    pub fn reject_pairing(&self, peer: &Fingerprint) {
        if self.inner.otp.lock().unwrap().cancel(*peer) {
            self.inner.emit(NodeEvent::PairingFinished {
                peer: *peer,
                paired: false,
            });
        }
    }

    /// Memutus `peer` yang sedang mengendalikan perangkat ini.
    pub fn end_incoming_session(&self, peer: &Fingerprint) {
        if let Some(connection) = self.inner.controllers.lock().unwrap().remove(peer) {
            connection.close(CLOSE_NORMAL.into(), b"ended by host");
        }
    }

    /// Mengatur apakah perangkat lain boleh mengendalikan atau pairing ke
    /// perangkat ini. Mematikannya juga memutus sesi yang sedang berjalan.
    pub fn set_accept_incoming(&self, accept: bool) {
        self.inner.accept_incoming.store(accept, Ordering::Relaxed);
        if !accept {
            let controllers: Vec<_> = self.inner.controllers.lock().unwrap().drain().collect();
            for (_, connection) in controllers {
                connection.close(CLOSE_NORMAL.into(), b"ended by host");
            }
        }
    }

    /// Menghubungi perangkat di `address`. Kalau `expected` diisi, koneksi
    /// dibatalkan bila sertifikat perangkat itu bukan yang diharapkan.
    pub async fn connect(&self, address: SocketAddr, expected: Option<Fingerprint>) -> Result<Peer, ConnectError> {
        Peer::connect(&self.inner, address, expected).await
    }

    /// Menutup semua koneksi dan berhenti menerima koneksi baru.
    pub fn shutdown(&self) {
        self.inner.endpoint.close(CLOSE_NORMAL.into(), b"shutdown");
    }
}
