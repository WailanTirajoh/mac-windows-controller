//! Node: satu instalasi Sambung di jaringan. Menerima koneksi dari perangkat
//! lain, memulai koneksi keluar, dan menjalankan pairing di kedua arah.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::identity::{Fingerprint, Identity};
use crate::net::{self, Link};
use crate::pairing::otp::{OtpManager, OtpPolicy};
use crate::pairing::pake::{self, Binding, Confirmations};
use crate::protocol::{
    read_message, write_message, ControlMessage, PairFailure, StreamKind, PROTOCOL_VERSION,
};
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
            events,
        });

        let accepting = Arc::clone(&inner);
        tokio::spawn(async move {
            while let Some(incoming) = accepting.endpoint.accept().await {
                let inner = Arc::clone(&accepting);
                tokio::spawn(async move {
                    if let Err(error) = serve(inner, incoming).await {
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

    /// Menghapus perangkat dari daftar terpercaya. Koneksi berikutnya dari
    /// perangkat itu harus pairing ulang.
    pub fn forget(&self, fingerprint: &Fingerprint) -> anyhow::Result<bool> {
        let removed = self.inner.trust.remove(fingerprint)?;
        if removed {
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

    /// Mengatur apakah perangkat lain boleh mengendalikan atau pairing ke perangkat ini.
    pub fn set_accept_incoming(&self, accept: bool) {
        self.inner.accept_incoming.store(accept, Ordering::Relaxed);
    }

    /// Menghubungi perangkat di `address`. Kalau `expected` diisi, koneksi
    /// dibatalkan bila sertifikat perangkat itu bukan yang diharapkan.
    pub async fn connect(&self, address: SocketAddr, expected: Option<Fingerprint>) -> Result<Peer, ConnectError> {
        let inner = &self.inner;
        let link = Link::connect(&inner.endpoint, address).await?;

        if link.peer_fingerprint == inner.identity.fingerprint() {
            link.connection.close(CLOSE_NORMAL.into(), b"self");
            return Err(ConnectError::SelfConnection);
        }
        if expected.is_some_and(|expected| expected != link.peer_fingerprint) {
            link.connection.close(CLOSE_IDENTITY.into(), b"identity");
            return Err(ConnectError::IdentityChanged);
        }

        let known = inner.trust.get(&link.peer_fingerprint);
        let handshake = async {
            let (mut send, mut recv) = link.connection.open_bi().await?;
            send.write_u8(StreamKind::Control as u8).await?;
            write_message(&mut send, &inner.hello(known.is_some())).await?;
            let reply = read_message(&mut recv).await?;
            anyhow::Ok((send, recv, reply))
        };
        let (send, recv, reply) = tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake)
            .await
            .context("perangkat tujuan tidak merespons")??;

        let Some(ControlMessage::Hello {
            protocol,
            name,
            os,
            trusts_peer,
        }) = reply
        else {
            link.connection.close(CLOSE_PROTOCOL.into(), b"hello");
            return Err(anyhow::anyhow!("perangkat tujuan mengirim balasan yang tidak dikenal").into());
        };
        if protocol != PROTOCOL_VERSION {
            link.connection.close(CLOSE_PROTOCOL.into(), b"version");
            return Err(ConnectError::Incompatible { theirs: protocol });
        }

        let info = DeviceInfo {
            fingerprint: link.peer_fingerprint,
            name,
            os,
        };
        if let Some(known) = &known {
            inner.refresh(known, &info, address);
        }

        Ok(Peer {
            inner: Arc::clone(inner),
            info,
            address,
            paired: AtomicBool::new(known.is_some() && trusts_peer),
            control: tokio::sync::Mutex::new((send, recv)),
            link,
        })
    }

    /// Menutup semua koneksi dan berhenti menerima koneksi baru.
    pub fn shutdown(&self) {
        self.inner.endpoint.close(CLOSE_NORMAL.into(), b"shutdown");
    }
}

/// Koneksi keluar ke perangkat lain.
pub struct Peer {
    inner: Arc<Inner>,
    link: Link,
    control: tokio::sync::Mutex<(quinn::SendStream, quinn::RecvStream)>,
    pub info: DeviceInfo,
    /// Alamat yang dipakai untuk koneksi ini.
    pub address: SocketAddr,
    paired: AtomicBool,
}

impl Peer {
    /// `true` kalau kedua perangkat sudah saling menyimpan, jadi tidak perlu OTP.
    pub fn is_paired(&self) -> bool {
        self.paired.load(Ordering::Relaxed)
    }

    async fn exchange(&self, message: &ControlMessage) -> anyhow::Result<ControlMessage> {
        let mut control = self.control.lock().await;
        let (send, recv) = &mut *control;
        write_message(send, message).await?;
        read_message(recv).await?.context("perangkat tujuan menutup koneksi")
    }

    /// Meminta perangkat tujuan menampilkan OTP. Mengembalikan masa berlaku kodenya.
    pub async fn request_pairing(&self) -> Result<Duration, PairError> {
        match self.exchange(&ControlMessage::PairRequest).await? {
            ControlMessage::PairChallenge { expires_in_secs } => Ok(Duration::from_secs(expires_in_secs)),
            ControlMessage::PairFailed(failure) => Err(failure.into()),
            _ => Err(PairFailure::Protocol.into()),
        }
    }

    /// Menyelesaikan pairing dengan OTP yang dibaca pengguna dari perangkat
    /// tujuan. `allow_reverse` menentukan apakah perangkat tujuan juga boleh
    /// mengendalikan perangkat ini.
    pub async fn submit_code(&self, code: &str, allow_reverse: bool) -> Result<(), PairError> {
        let binding = Binding {
            tls_exporter: self.link.pairing_binding()?,
            initiator: self.inner.identity.fingerprint(),
            responder: self.link.peer_fingerprint,
        };
        let start = pake::start_initiator(code, &binding);

        let reply = self
            .exchange(&ControlMessage::PairPakeA {
                message: start.message.clone(),
            })
            .await?;
        let peer_message = match reply {
            ControlMessage::PairPakeB { message } => message,
            ControlMessage::PairFailed(failure) => return Err(failure.into()),
            _ => return Err(PairFailure::Protocol.into()),
        };
        let confirmations = start.finish(&peer_message, &binding).ok_or(PairError::Unverified)?;

        let reply = self
            .exchange(&ControlMessage::PairConfirmA {
                confirm: confirmations.initiator.to_vec(),
            })
            .await?;
        match reply {
            ControlMessage::PairDone { confirm } if pake::confirmations_match(&confirmations.responder, &confirm) => {}
            ControlMessage::PairDone { .. } => return Err(PairError::Unverified),
            ControlMessage::PairFailed(failure) => return Err(failure.into()),
            _ => return Err(PairFailure::Protocol.into()),
        }

        self.inner.remember(&self.info, allow_reverse, self.address)?;
        self.paired.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Memeriksa bahwa perangkat tujuan menganggap kita terpercaya.
    pub async fn ping(&self) -> anyhow::Result<bool> {
        match self.exchange(&ControlMessage::Ping).await? {
            ControlMessage::Pong => Ok(true),
            ControlMessage::NotPaired => Ok(false),
            other => anyhow::bail!("balasan tidak terduga: {other:?}"),
        }
    }

    pub fn close(&self) {
        self.link.connection.close(CLOSE_NORMAL.into(), b"bye");
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.close();
    }
}

/// Melayani satu koneksi masuk sampai ditutup.
async fn serve(inner: Arc<Inner>, incoming: quinn::Incoming) -> anyhow::Result<()> {
    let link = Link::new(incoming.await?)?;

    let handshake = async {
        let (send, mut recv) = link.connection.accept_bi().await?;
        let kind = recv.read_u8().await?;
        let hello = read_message(&mut recv).await?;
        anyhow::Ok((send, recv, kind, hello))
    };
    let (mut send, recv, kind, hello) = tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake)
        .await
        .context("lawan tidak mengirim hello")??;

    let (name, os) = match hello {
        Some(ControlMessage::Hello { protocol, name, os, .. })
            if StreamKind::from_byte(kind) == Some(StreamKind::Control) && protocol == PROTOCOL_VERSION =>
        {
            (name, os)
        }
        Some(ControlMessage::Hello { .. }) => {
            // Balas dulu supaya lawan bisa menampilkan pesan versi yang tidak cocok.
            write_message(&mut send, &inner.hello(false)).await?;
            let _ = send.finish();
            link.connection.closed().await;
            anyhow::bail!("versi protokol lawan tidak cocok");
        }
        _ => {
            link.connection.close(CLOSE_PROTOCOL.into(), b"hello");
            anyhow::bail!("stream pertama bukan kanal kontrol");
        }
    };
    let hello = inner.hello(inner.allows_incoming(&link.peer_fingerprint));
    write_message(&mut send, &hello).await?;

    let mut responder = Responder {
        peer: DeviceInfo {
            fingerprint: link.peer_fingerprint,
            name,
            os,
        },
        inner,
        link,
        send,
        recv,
        pending: None,
        code_shown: false,
    };
    let result = responder.run().await;
    responder.finish();
    result
}

/// Sisi penerima dari satu koneksi.
struct Responder {
    inner: Arc<Inner>,
    link: Link,
    peer: DeviceInfo,
    send: quinn::SendStream,
    recv: quinn::RecvStream,
    /// Konfirmasi yang ditunggu dari percobaan pairing yang sedang berjalan.
    pending: Option<Confirmations>,
    /// Apakah dialog OTP untuk koneksi ini masih terbuka di UI.
    code_shown: bool,
}

impl Responder {
    /// Apakah lawan boleh melakukan hal selain pairing.
    fn trusted(&self) -> bool {
        self.inner.allows_incoming(&self.peer.fingerprint)
    }

    async fn reply(&mut self, message: ControlMessage) -> anyhow::Result<()> {
        write_message(&mut self.send, &message).await
    }

    async fn run(&mut self) -> anyhow::Result<()> {
        loop {
            let message = tokio::select! {
                message = read_message(&mut self.recv) => message?,
                // Stream selain kanal kontrol belum dilayani; dari perangkat
                // yang belum di-pair, membukanya memutus koneksi.
                _ = self.link.connection.accept_uni() => None,
                _ = self.link.connection.accept_bi() => None,
            };
            let Some(message) = message else {
                if !self.trusted() {
                    self.link.connection.close(CLOSE_NOT_PAIRED.into(), b"not paired");
                }
                return Ok(());
            };

            match message {
                ControlMessage::PairRequest => self.on_pair_request().await?,
                ControlMessage::PairPakeA { message } => self.on_pake_a(&message).await?,
                ControlMessage::PairConfirmA { confirm } => self.on_confirm_a(&confirm).await?,
                ControlMessage::Ping => {
                    let reply = if self.trusted() {
                        ControlMessage::Pong
                    } else {
                        ControlMessage::NotPaired
                    };
                    self.reply(reply).await?;
                }
                other => {
                    self.link.connection.close(CLOSE_PROTOCOL.into(), b"unexpected");
                    anyhow::bail!("pesan tidak terduga dari lawan: {other:?}");
                }
            }
        }
    }

    async fn on_pair_request(&mut self) -> anyhow::Result<()> {
        if !self.inner.accept_incoming.load(Ordering::Relaxed) {
            return self.reply(ControlMessage::PairFailed(PairFailure::Disabled)).await;
        }
        self.abandon_attempt();

        let issued = self.inner.otp.lock().unwrap().issue(self.peer.fingerprint, Instant::now());
        match issued {
            Ok(code) => {
                self.code_shown = true;
                self.inner.emit(NodeEvent::PairingRequested {
                    peer: self.peer.clone(),
                    code,
                    expires_in: self.inner.otp_ttl,
                });
                self.reply(ControlMessage::PairChallenge {
                    expires_in_secs: self.inner.otp_ttl.as_secs(),
                })
                .await
            }
            Err(failure) => self.reply(ControlMessage::PairFailed(failure)).await,
        }
    }

    async fn on_pake_a(&mut self, peer_message: &[u8]) -> anyhow::Result<()> {
        self.abandon_attempt();

        let now = Instant::now();
        let code = self.inner.otp.lock().unwrap().begin_attempt(self.peer.fingerprint, now);
        let code = match code {
            Ok(code) => code,
            Err(failure) => {
                self.close_dialog(false);
                return self.reply(ControlMessage::PairFailed(failure)).await;
            }
        };

        let binding = Binding {
            tls_exporter: self.link.pairing_binding()?,
            initiator: self.peer.fingerprint,
            responder: self.inner.identity.fingerprint(),
        };
        let start = pake::start_responder(&code, &binding);
        let message = start.message.clone();
        match start.finish(peer_message, &binding) {
            Some(confirmations) => {
                self.pending = Some(confirmations);
                self.reply(ControlMessage::PairPakeB { message }).await
            }
            None => self.fail_attempt().await,
        }
    }

    async fn on_confirm_a(&mut self, confirm: &[u8]) -> anyhow::Result<()> {
        let Some(expected) = self.pending.take() else {
            return self.reply(ControlMessage::PairFailed(PairFailure::Protocol)).await;
        };
        if !pake::confirmations_match(&expected.initiator, confirm) {
            return self.fail_attempt().await;
        }

        self.inner.otp.lock().unwrap().succeed(self.peer.fingerprint);
        // Koneksi keluar lawan memakai soket yang sama dengan port dengarnya,
        // jadi alamat asal koneksi ini juga alamat untuk menghubunginya balik.
        self.inner
            .remember(&self.peer, true, self.link.connection.remote_address())?;
        self.close_dialog(true);
        self.reply(ControlMessage::PairDone {
            confirm: expected.responder.to_vec(),
        })
        .await
    }

    /// Mencatat percobaan yang gagal dan memberi tahu lawan.
    async fn fail_attempt(&mut self) -> anyhow::Result<()> {
        let failure = self.inner.otp.lock().unwrap().fail(self.peer.fingerprint, Instant::now());
        if matches!(failure, PairFailure::WrongCode { attempts_left: 0 }) {
            self.close_dialog(false);
        }
        self.reply(ControlMessage::PairFailed(failure)).await
    }

    /// Percobaan yang sudah dimulai tetapi tidak pernah dikonfirmasi dihitung gagal.
    fn abandon_attempt(&mut self) {
        if self.pending.take().is_some() {
            let failure = self.inner.otp.lock().unwrap().fail(self.peer.fingerprint, Instant::now());
            if matches!(failure, PairFailure::WrongCode { attempts_left: 0 }) {
                self.close_dialog(false);
            }
        }
    }

    fn close_dialog(&mut self, paired: bool) {
        if std::mem::take(&mut self.code_shown) {
            self.inner.emit(NodeEvent::PairingFinished {
                peer: self.peer.fingerprint,
                paired,
            });
        }
    }

    /// Dipanggil saat koneksi berakhir.
    fn finish(&mut self) {
        self.abandon_attempt();
        self.inner.otp.lock().unwrap().cancel(self.peer.fingerprint);
        self.close_dialog(false);
    }
}
