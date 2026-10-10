//! Koneksi keluar ke perangkat lain: pairing dan sesi remote desktop dari
//! sisi pengendali.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::{
    ConnectError, DeviceInfo, Inner, PairError, TransferTracker, CLOSE_IDENTITY, CLOSE_NORMAL, CLOSE_PROTOCOL,
    HANDSHAKE_TIMEOUT,
};
use crate::clipboard::ClipboardSync;
use crate::identity::Fingerprint;
use crate::net::Link;
use crate::pairing::pake::{self, Binding};
use crate::protocol::{
    read_message, write_message, ControlMessage, PairFailure, StreamKind, VideoParams, PROTOCOL_VERSION,
};
use crate::session::controller::{self, InputSender, SessionEvent, SessionInfo, EVENT_QUEUE};
use crate::transfer;

/// Batas waktu menunggu balasan. Memulai sesi bisa lama karena perangkat
/// tujuan harus menyalakan capture layar dulu.
const REPLY_TIMEOUT: Duration = Duration::from_secs(20);

/// Tujuan pesan sesi yang dikirim perangkat tujuan tanpa diminta.
struct ActiveSession {
    events: mpsc::Sender<SessionEvent>,
    /// Baru ada setelah perangkat tujuan menerima sesinya.
    clipboard: Option<Arc<ClipboardSync>>,
}

type SessionSlot = Arc<Mutex<Option<ActiveSession>>>;

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("perangkat tujuan belum mengizinkan perangkat ini; hubungkan ulang dengan kode")]
    NotPaired,
    #[error("perangkat tujuan tidak bisa memulai sesi: {0}")]
    Refused(String),
    #[error("koneksi bermasalah: {0:#}")]
    Connection(#[from] anyhow::Error),
}

/// Koneksi keluar ke perangkat lain.
pub struct Peer {
    inner: Arc<Inner>,
    link: Link,
    pub info: DeviceInfo,
    /// Alamat yang dipakai untuk koneksi ini.
    pub address: SocketAddr,
    paired: AtomicBool,
    /// Nomor untuk sesi berikutnya di koneksi ini.
    next_session: AtomicU32,
    /// Pesan kontrol keluar; ditulis ke stream oleh satu task.
    outbox: mpsc::UnboundedSender<ControlMessage>,
    /// Balasan atas permintaan. Kuncinya juga membuat permintaan berjalan satu per satu.
    replies: tokio::sync::Mutex<mpsc::UnboundedReceiver<ControlMessage>>,
    session: SessionSlot,
}

impl Peer {
    pub(super) async fn connect(
        inner: &Arc<Inner>,
        address: SocketAddr,
        expected: Option<Fingerprint>,
    ) -> Result<Self, ConnectError> {
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

        let session = SessionSlot::default();
        let (outbox, outgoing) = mpsc::unbounded_channel();
        let (reply_sender, replies) = mpsc::unbounded_channel();
        tokio::spawn(write_control(send, outgoing));
        tokio::spawn(read_control(recv, reply_sender, Arc::clone(&session)));

        Ok(Self {
            inner: Arc::clone(inner),
            info,
            address,
            paired: AtomicBool::new(known.is_some() && trusts_peer),
            next_session: AtomicU32::new(1),
            outbox,
            replies: tokio::sync::Mutex::new(replies),
            session,
            link,
        })
    }

    /// `false` kalau koneksi sudah ditutup oleh salah satu sisi atau terputus.
    pub fn is_connected(&self) -> bool {
        self.link.connection.close_reason().is_none()
    }

    /// `true` kalau kedua perangkat sudah saling menyimpan, jadi tidak perlu OTP.
    pub fn is_paired(&self) -> bool {
        self.paired.load(Ordering::Relaxed)
    }

    /// Mengirim permintaan dan menunggu balasannya.
    async fn exchange(&self, message: ControlMessage) -> anyhow::Result<ControlMessage> {
        let mut replies = self.replies.lock().await;
        self.outbox
            .send(message)
            .ok()
            .context("koneksi ke perangkat tujuan sudah tertutup")?;
        tokio::time::timeout(REPLY_TIMEOUT, replies.recv())
            .await
            .context("perangkat tujuan tidak merespons")?
            .context("perangkat tujuan menutup koneksi")
    }

    /// Meminta perangkat tujuan menampilkan OTP. Mengembalikan masa berlaku kodenya.
    pub async fn request_pairing(&self) -> Result<Duration, PairError> {
        match self.exchange(ControlMessage::PairRequest).await? {
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
            .exchange(ControlMessage::PairPakeA {
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
            .exchange(ControlMessage::PairConfirmA {
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
        match self.exchange(ControlMessage::Ping).await? {
            ControlMessage::Pong => Ok(true),
            ControlMessage::NotPaired => Ok(false),
            other => anyhow::bail!("balasan tidak terduga: {other:?}"),
        }
    }

    /// Memulai sesi remote desktop. Sesi berakhir saat [`Session`] di-drop.
    pub async fn start_session(&self, display_id: Option<u32>, video: VideoParams) -> Result<Session, SessionError> {
        let (events, receiver) = mpsc::channel(EVENT_QUEUE);
        let session_id = self.next_session.fetch_add(1, Ordering::Relaxed);
        // Penerima video dinyalakan sebelum meminta sesi supaya frame pertama tidak terlewat.
        let video_task = tokio::spawn(controller::receive_video(
            self.link.connection.clone(),
            session_id,
            events.clone(),
            self.outbox.clone(),
        ));
        *self.session.lock().unwrap() = Some(ActiveSession {
            events,
            clipboard: None,
        });

        let request = ControlMessage::SessionStart {
            session_id,
            display_id,
            video,
        };
        let started = match self.exchange(request).await {
            Ok(ControlMessage::SessionStarted {
                displays,
                display_id,
                input_enabled,
            }) => Ok(SessionInfo {
                displays,
                display_id,
                input_enabled,
            }),
            Ok(ControlMessage::SessionFailed { reason }) => Err(SessionError::Refused(reason)),
            Ok(ControlMessage::NotPaired) => Err(SessionError::NotPaired),
            Ok(other) => Err(anyhow::anyhow!("balasan tidak terduga: {other:?}").into()),
            Err(error) => Err(error.into()),
        };

        match started {
            Ok(info) => {
                let outbox = self.outbox.clone();
                let clipboard = Arc::new(ClipboardSync::start(
                    Arc::clone(&self.inner.services.clipboard),
                    move |text| {
                        let _ = outbox.send(ControlMessage::Clipboard { text });
                    },
                ));
                if let Some(active) = self.session.lock().unwrap().as_mut() {
                    active.clipboard = Some(Arc::clone(&clipboard));
                }
                Ok(Session {
                    info,
                    events: receiver,
                    input: InputSender::open(self.link.connection.clone()),
                    clipboard,
                    video_task,
                    slot: Arc::clone(&self.session),
                    outbox: self.outbox.clone(),
                })
            }
            Err(error) => {
                video_task.abort();
                *self.session.lock().unwrap() = None;
                Err(error)
            }
        }
    }

    /// Mulai mengirim file di `path`. Kemajuan dan hasilnya dilaporkan lewat
    /// `NodeEvent::Transfer` dengan ID yang dikembalikan di sini. Transfer
    /// terputus kalau koneksi ini ditutup sebelum selesai.
    pub async fn send_file(&self, path: PathBuf) -> anyhow::Result<u64> {
        let metadata = tokio::fs::metadata(&path).await.context("file tidak bisa dibaca")?;
        anyhow::ensure!(metadata.is_file(), "yang bisa dikirim hanya file, bukan folder");
        let name = path
            .file_name()
            .context("path bukan file")?
            .to_string_lossy()
            .into_owned();

        let mut tracker = TransferTracker::start(&self.inner, self.info.clone(), name, metadata.len(), false);
        let id = tracker.id;
        let connection = self.link.connection.clone();
        tokio::spawn(async move {
            let cancel = Arc::clone(&tracker.cancel);
            let sending = async {
                let (mut send, mut recv) = connection.open_bi().await?;
                send.write_u8(StreamKind::File as u8).await?;
                let sent = transfer::send_file(&mut send, &mut recv, &path, &cancel, |sent| tracker.progress(sent)).await;
                match &sent {
                    Ok(()) => drop(send.finish()),
                    // Membatalkan stream supaya penerima langsung tahu kirimannya gagal.
                    Err(_) => drop(send.reset(CLOSE_NORMAL.into())),
                }
                sent
            };
            let outcome = sending.await.map(|()| path.clone());
            tracker.finish(outcome);
        });
        Ok(id)
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

/// Sesi remote desktop yang sedang berjalan, dari sisi pengendali.
pub struct Session {
    pub info: SessionInfo,
    /// Frame video dan perubahan keadaan sesi.
    pub events: mpsc::Receiver<SessionEvent>,
    /// Pengirim event input; bisa di-clone dan dipakai dari task lain.
    pub input: InputSender,
    clipboard: Arc<ClipboardSync>,
    video_task: JoinHandle<()>,
    slot: SessionSlot,
    outbox: mpsc::UnboundedSender<ControlMessage>,
}

impl Session {
    /// Meminta pindah monitor. Hasilnya datang sebagai `SessionEvent::DisplayChanged`.
    pub fn select_display(&self, display_id: u32) {
        let _ = self.outbox.send(ControlMessage::SelectDisplay { display_id });
    }

    pub fn request_keyframe(&self) {
        let _ = self.outbox.send(ControlMessage::RequestKeyframe);
    }

    /// Menyalakan atau mematikan sinkronisasi clipboard untuk sesi ini.
    pub fn set_clipboard_sync(&self, enabled: bool) {
        self.clipboard.set_enabled(enabled);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.video_task.abort();
        *self.slot.lock().unwrap() = None;
        let _ = self.outbox.send(ControlMessage::SessionStop);
    }
}

async fn write_control(mut send: quinn::SendStream, mut outgoing: mpsc::UnboundedReceiver<ControlMessage>) {
    while let Some(message) = outgoing.recv().await {
        if write_message(&mut send, &message).await.is_err() {
            break;
        }
    }
    let _ = send.finish();
}

async fn read_control(
    mut recv: quinn::RecvStream,
    replies: mpsc::UnboundedSender<ControlMessage>,
    session: SessionSlot,
) {
    let forward = |event: SessionEvent| {
        let events = session.lock().unwrap().as_ref().map(|active| active.events.clone());
        async move {
            if let Some(events) = events {
                let _ = events.send(event).await;
            }
        }
    };

    while let Ok(Some(message)) = read_message(&mut recv).await {
        match message {
            // Dikirim perangkat tujuan tanpa diminta, jadi bukan balasan.
            ControlMessage::DisplayChanged { display_id } => forward(SessionEvent::DisplayChanged(display_id)).await,
            ControlMessage::SessionStop => forward(SessionEvent::Ended).await,
            ControlMessage::Clipboard { text } => {
                let clipboard = session.lock().unwrap().as_ref().and_then(|active| active.clipboard.clone());
                if let Some(clipboard) = clipboard {
                    clipboard.receive(text);
                }
            }
            reply => {
                if replies.send(reply).is_err() {
                    break;
                }
            }
        }
    }
    forward(SessionEvent::Ended).await;
}
