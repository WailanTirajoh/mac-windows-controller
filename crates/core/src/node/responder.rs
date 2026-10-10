//! Sisi penerima dari satu koneksi masuk: pairing dan sesi remote desktop
//! saat perangkat ini dikendalikan.

use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;

use super::{DeviceInfo, Inner, NodeEvent, TransferTracker, CLOSE_NOT_PAIRED, CLOSE_PROTOCOL, HANDSHAKE_TIMEOUT};
use crate::net::Link;
use crate::pairing::pake::{self, Binding, Confirmations};
use crate::protocol::{
    read_frame, read_message, write_message, ControlMessage, InputEvent, PairFailure, StreamKind, VideoParams,
    PROTOCOL_VERSION,
};
use crate::session::host::HostSession;
use crate::transfer;

/// Batas permintaan kualitas video dari pengendali.
const LONG_EDGE_RANGE: (u32, u32) = (320, 3840);
const FPS_RANGE: (u32, u32) = (1, 60);
const BITRATE_RANGE: (u32, u32) = (500_000, 50_000_000);

/// Menerima satu file dari `peer` ke folder unduhan.
async fn receive_file(inner: Arc<Inner>, peer: DeviceInfo, mut send: quinn::SendStream, mut recv: quinn::RecvStream) {
    let Ok(offer) = transfer::read_offer(&mut recv).await else {
        return;
    };
    let name = transfer::filename::sanitize(&offer.name);
    let mut tracker = TransferTracker::start(&inner, peer, name, offer.size, true);
    let cancel = Arc::clone(&tracker.cancel);

    let outcome = transfer::receive_file(&mut send, &mut recv, &offer, &inner.downloads_dir, &cancel, |received| {
        tracker.progress(received);
    })
    .await;
    if outcome.is_err() {
        // Menghentikan pengirim yang masih mengirim, mis. saat dibatalkan di sisi ini.
        let _ = recv.stop(CLOSE_PROTOCOL.into());
    }
    let _ = send.finish();
    tracker.finish(outcome);
}

/// Teks berikutnya dari clipboard lokal. Tidak pernah selesai kalau tidak ada sesi.
async fn next_copied(copied: &mut Option<mpsc::UnboundedReceiver<String>>) -> Option<String> {
    match copied {
        Some(copied) => copied.recv().await,
        None => std::future::pending().await,
    }
}

/// Melayani satu koneksi masuk sampai ditutup.
pub(super) async fn serve(inner: Arc<Inner>, incoming: quinn::Incoming) -> anyhow::Result<()> {
    let link = Link::new(incoming.await?)?;

    let handshake = async {
        let (send, mut recv) = link.connection.accept_bi().await?;
        let kind = recv.read_u8().await?;
        let hello = read_message(&mut recv).await?;
        anyhow::Ok((send, recv, kind, hello))
    };
    let (mut send, mut recv, kind, hello) = tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake)
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

    // Pesan kontrol dibaca oleh task sendiri: `read_message` tidak aman
    // dibatalkan di tengah jalan, sedangkan `recv` pada kanal aman.
    let (received, messages) = mpsc::unbounded_channel();
    let reader = tokio::spawn(async move {
        while let Ok(Some(message)) = read_message(&mut recv).await {
            if received.send(message).is_err() {
                break;
            }
        }
    });

    let mut responder = Responder {
        peer: DeviceInfo {
            fingerprint: link.peer_fingerprint,
            name,
            os,
        },
        inner,
        link,
        send,
        messages,
        pending: None,
        code_shown: false,
        session: None,
        copied: None,
    };
    let result = responder.run().await;
    reader.abort();
    responder.finish().await;
    result
}

struct Responder {
    inner: Arc<Inner>,
    link: Link,
    peer: DeviceInfo,
    send: quinn::SendStream,
    messages: mpsc::UnboundedReceiver<ControlMessage>,
    /// Konfirmasi yang ditunggu dari percobaan pairing yang sedang berjalan.
    pending: Option<Confirmations>,
    /// Apakah dialog OTP untuk koneksi ini masih terbuka di UI.
    code_shown: bool,
    /// Sesi remote desktop yang sedang berjalan untuk koneksi ini.
    session: Option<HostSession>,
    /// Teks yang disalin di perangkat ini selama sesi, untuk dikirim ke pengendali.
    copied: Option<mpsc::UnboundedReceiver<String>>,
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
            tokio::select! {
                message = self.messages.recv() => match message {
                    Some(message) => self.on_message(message).await?,
                    None => return Ok(()),
                },
                stream = self.link.connection.accept_uni() => self.on_stream(None, stream?)?,
                stream = self.link.connection.accept_bi() => {
                    let (send, recv) = stream?;
                    self.on_stream(Some(send), recv)?;
                }
                Some(text) = next_copied(&mut self.copied) => {
                    self.reply(ControlMessage::Clipboard { text }).await?;
                }
            }
        }
    }

    async fn on_message(&mut self, message: ControlMessage) -> anyhow::Result<()> {
        match message {
            ControlMessage::PairRequest => self.on_pair_request().await,
            ControlMessage::PairPakeA { message } => self.on_pake_a(&message).await,
            ControlMessage::PairConfirmA { confirm } => self.on_confirm_a(&confirm).await,
            ControlMessage::Ping => {
                let reply = if self.trusted() {
                    ControlMessage::Pong
                } else {
                    ControlMessage::NotPaired
                };
                self.reply(reply).await
            }
            ControlMessage::SessionStart {
                session_id,
                display_id,
                video,
            } => self.on_session_start(session_id, display_id, video).await,
            ControlMessage::SelectDisplay { display_id } => self.on_select_display(display_id).await,
            ControlMessage::RequestKeyframe => {
                if let Some(session) = &self.session {
                    session.request_keyframe();
                }
                Ok(())
            }
            ControlMessage::SessionStop => {
                self.end_session().await;
                Ok(())
            }
            ControlMessage::Clipboard { text } => {
                if let Some(session) = &self.session {
                    session.receive_clipboard(text);
                }
                Ok(())
            }
            other => {
                self.link.connection.close(CLOSE_PROTOCOL.into(), b"unexpected");
                anyhow::bail!("pesan tidak terduga dari lawan: {other:?}");
            }
        }
    }

    /// Stream selain kanal kontrol. Hanya perangkat yang sudah di-pair yang
    /// boleh membukanya; `send` hanya ada untuk stream dua arah.
    fn on_stream(&mut self, send: Option<quinn::SendStream>, mut stream: quinn::RecvStream) -> anyhow::Result<()> {
        if !self.inner.accepts_files_from(&self.peer.fingerprint) {
            self.link.connection.close(CLOSE_NOT_PAIRED.into(), b"not paired");
            anyhow::bail!("perangkat yang belum di-pair membuka stream");
        }

        // Stream input hanya berarti kalau ada sesi, dan sesi hanya dimulai
        // untuk perangkat yang boleh mengendalikan.
        let input = self.session.as_ref().and_then(HostSession::input_handle);
        let inner = Arc::clone(&self.inner);
        let peer = self.peer.clone();
        tokio::spawn(async move {
            match (stream.read_u8().await.ok().and_then(StreamKind::from_byte), send) {
                (Some(StreamKind::Input), None) => {
                    let Some(input) = input else {
                        return;
                    };
                    while let Ok(Some(event)) = read_frame::<_, InputEvent>(&mut stream).await {
                        if input.send(event).is_err() {
                            break;
                        }
                    }
                }
                (Some(StreamKind::File), Some(send)) => receive_file(inner, peer, send, stream).await,
                _ => {
                    let _ = stream.stop(CLOSE_PROTOCOL.into());
                }
            }
        });
        Ok(())
    }

    async fn on_session_start(
        &mut self,
        session_id: u32,
        display_id: Option<u32>,
        video: VideoParams,
    ) -> anyhow::Result<()> {
        if !self.trusted() {
            return self.reply(ControlMessage::NotPaired).await;
        }
        self.end_session().await;

        let video = VideoParams {
            max_long_edge: video.max_long_edge.clamp(LONG_EDGE_RANGE.0, LONG_EDGE_RANGE.1),
            max_fps: video.max_fps.clamp(FPS_RANGE.0, FPS_RANGE.1),
            bitrate_bps: video.bitrate_bps.clamp(BITRATE_RANGE.0, BITRATE_RANGE.1),
        };
        // Menyalakan capture bisa menunggu OS beberapa saat, jadi tidak di thread async.
        let connection = self.link.connection.clone();
        let services = self.inner.services.clone();
        let (copied, copied_texts) = mpsc::unbounded_channel();
        let started = tokio::task::spawn_blocking(move || {
            HostSession::start(connection, &services, session_id, display_id, video, copied)
        })
        .await?;

        match started {
            Ok((session, displays)) => {
                self.copied = Some(copied_texts);
                let reply = ControlMessage::SessionStarted {
                    displays,
                    display_id: session.display_id(),
                    input_enabled: session.input_enabled(),
                };
                self.inner.emit(NodeEvent::SessionStarted {
                    peer: self.peer.clone(),
                    input_enabled: session.input_enabled(),
                });
                self.inner
                    .controllers
                    .lock()
                    .unwrap()
                    .insert(self.peer.fingerprint, self.link.connection.clone());
                self.session = Some(session);
                self.reply(reply).await
            }
            Err(error) => {
                self.reply(ControlMessage::SessionFailed {
                    reason: format!("{error:#}"),
                })
                .await
            }
        }
    }

    async fn on_select_display(&mut self, display_id: u32) -> anyhow::Result<()> {
        let Some(mut session) = self.session.take() else {
            return Ok(());
        };
        let session = tokio::task::spawn_blocking(move || {
            if let Err(error) = session.select_display(display_id) {
                tracing::warn!("gagal pindah monitor: {error:#}");
            }
            session
        })
        .await?;

        // Kalau gagal, ID yang dilaporkan tetap monitor lama.
        let display_id = session.display_id();
        self.session = Some(session);
        self.reply(ControlMessage::DisplayChanged { display_id }).await
    }

    async fn end_session(&mut self) {
        let Some(session) = self.session.take() else {
            return;
        };
        self.copied = None;
        self.inner.controllers.lock().unwrap().remove(&self.peer.fingerprint);
        // Menghentikan capture bisa menunggu OS sebentar.
        let _ = tokio::task::spawn_blocking(move || drop(session)).await;
        self.inner.emit(NodeEvent::SessionEnded {
            peer: self.peer.fingerprint,
        });
    }

    async fn on_pair_request(&mut self) -> anyhow::Result<()> {
        if !self.inner.accept_incoming.load(std::sync::atomic::Ordering::Relaxed) {
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
    async fn finish(&mut self) {
        self.end_session().await;
        self.abandon_attempt();
        self.inner.otp.lock().unwrap().cancel(self.peer.fingerprint);
        self.close_dialog(false);
    }
}
