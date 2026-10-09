//! Sisi pengendali: menerima frame video dan mengirim input.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;

use crate::protocol::{
    write_frame, ControlMessage, DisplayDesc, InputEvent, StreamKind, VideoHeader, MAX_VIDEO_FRAME,
};
use crate::video::EncodedFrame;

/// Jumlah frame yang boleh menunggu frame sebelumnya sebelum dianggap macet.
const MAX_WAITING_FRAMES: usize = 4;
/// Jarak minimum antar permintaan keyframe.
const KEYFRAME_REQUEST_INTERVAL: Duration = Duration::from_millis(500);
/// Kapasitas antrean frame menuju viewer.
pub(crate) const EVENT_QUEUE: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub displays: Vec<DisplayDesc>,
    pub display_id: u32,
    /// `false` berarti sesi hanya untuk melihat.
    pub input_enabled: bool,
}

pub enum SessionEvent {
    Frame(EncodedFrame),
    DisplayChanged(u32),
    /// Sesi berakhir, oleh perangkat tujuan atau karena koneksi putus.
    Ended,
}

/// Mengirim event input ke perangkat tujuan.
#[derive(Clone)]
pub struct InputSender {
    events: mpsc::UnboundedSender<InputEvent>,
}

impl InputSender {
    /// Membuka stream input di `connection` dan mengembalikan pengirimnya.
    pub(crate) fn open(connection: quinn::Connection) -> Self {
        let (events, mut queued) = mpsc::unbounded_channel::<InputEvent>();
        tokio::spawn(async move {
            let Ok(mut stream) = connection.open_uni().await else {
                return;
            };
            if tokio::io::AsyncWriteExt::write_u8(&mut stream, StreamKind::Input as u8)
                .await
                .is_err()
            {
                return;
            }
            while let Some(event) = queued.recv().await {
                if write_frame(&mut stream, &event).await.is_err() {
                    break;
                }
            }
            let _ = stream.finish();
        });
        Self { events }
    }

    /// Event dibuang diam-diam kalau sesi sudah berakhir.
    pub fn send(&self, event: InputEvent) {
        let _ = self.events.send(event);
    }
}

/// Mengurutkan frame berdasarkan nomor urut. Tiap frame tiba lewat stream
/// sendiri, jadi urutan tiba tidak dijamin.
///
/// Decoder butuh frame berurutan tanpa celah. Kalau sebuah frame terlalu lama
/// tidak datang, pengurut melompat ke keyframe berikutnya.
#[derive(Default)]
pub struct FrameOrder {
    /// Nomor frame berikutnya yang boleh diteruskan; `None` berarti menunggu keyframe.
    next: Option<u64>,
    waiting: BTreeMap<u64, EncodedFrame>,
}

impl FrameOrder {
    /// Memasukkan satu frame dan mengembalikan frame yang sekarang bisa
    /// diteruskan, sudah berurutan.
    pub fn push(&mut self, sequence: u64, frame: EncodedFrame) -> Vec<EncodedFrame> {
        // Frame yang datang setelah kita melompat melewatinya tidak berguna lagi.
        if self.next.is_some_and(|next| sequence < next) {
            return Vec::new();
        }
        self.waiting.insert(sequence, frame);

        if self.next.is_some() && self.waiting.len() > MAX_WAITING_FRAMES {
            self.next = None;
        }
        self.drain()
    }

    /// `true` kalau decoder tidak bisa lanjut tanpa keyframe baru.
    pub fn needs_keyframe(&self) -> bool {
        self.next.is_none()
    }

    /// Membuang semua yang menunggu dan mulai lagi dari keyframe berikutnya.
    pub fn resync(&mut self) {
        self.next = None;
        self.waiting.clear();
    }

    fn drain(&mut self) -> Vec<EncodedFrame> {
        if self.next.is_none() {
            // Mulai dari keyframe paling awal yang ada; frame sebelumnya dibuang.
            let keyframe = self.waiting.iter().find(|(_, frame)| frame.keyframe).map(|(seq, _)| *seq);
            match keyframe {
                Some(sequence) => {
                    self.waiting = self.waiting.split_off(&sequence);
                    self.next = Some(sequence);
                }
                None => {
                    self.waiting.clear();
                    return Vec::new();
                }
            }
        }

        let mut ready = Vec::new();
        while let Some(next) = self.next {
            let Some(frame) = self.waiting.remove(&next) else {
                break;
            };
            ready.push(frame);
            self.next = Some(next + 1);
        }
        ready
    }
}

async fn read_video_stream(mut stream: quinn::RecvStream) -> anyhow::Result<(u64, EncodedFrame)> {
    let kind = stream.read_u8().await?;
    anyhow::ensure!(
        StreamKind::from_byte(kind) == Some(StreamKind::Video),
        "stream bukan video"
    );
    let mut header = [0u8; VideoHeader::BYTES];
    stream.read_exact(&mut header).await?;
    let header = VideoHeader::decode(&header);
    let data = stream.read_to_end(MAX_VIDEO_FRAME).await?;
    Ok((
        header.sequence,
        EncodedFrame {
            data,
            keyframe: header.keyframe,
            width: header.width,
            height: header.height,
            timestamp_us: header.timestamp_us,
        },
    ))
}

/// Menerima stream video dari `connection` dan meneruskan frame berurutan ke
/// `events`. `control` dipakai untuk meminta keyframe saat ada frame hilang
/// atau viewer tertinggal.
pub(crate) async fn receive_video(
    connection: quinn::Connection,
    events: mpsc::Sender<SessionEvent>,
    control: mpsc::UnboundedSender<ControlMessage>,
) {
    let (arrived, mut frames) = mpsc::unbounded_channel();
    let accepting = tokio::spawn(async move {
        while let Ok(stream) = connection.accept_uni().await {
            let arrived = arrived.clone();
            tokio::spawn(async move {
                if let Ok(frame) = read_video_stream(stream).await {
                    let _ = arrived.send(frame);
                }
            });
        }
    });

    let mut order = FrameOrder::default();
    let mut last_request: Option<Instant> = None;
    while let Some((sequence, frame)) = frames.recv().await {
        for frame in order.push(sequence, frame) {
            match events.try_send(SessionEvent::Frame(frame)) {
                Ok(()) => {}
                // Viewer tertinggal: buang sisanya dan mulai lagi dari keyframe.
                Err(mpsc::error::TrySendError::Full(_)) => {
                    order.resync();
                    break;
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    accepting.abort();
                    return;
                }
            }
        }
        let may_request = last_request.is_none_or(|at| at.elapsed() >= KEYFRAME_REQUEST_INTERVAL);
        if order.needs_keyframe() && may_request {
            last_request = Some(Instant::now());
            if control.send(ControlMessage::RequestKeyframe).is_err() {
                break;
            }
        }
    }
    accepting.abort();
    let _ = events.send(SessionEvent::Ended).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(sequence: u64, keyframe: bool) -> EncodedFrame {
        EncodedFrame {
            data: vec![sequence as u8],
            keyframe,
            width: 640,
            height: 360,
            timestamp_us: sequence * 16_667,
        }
    }

    /// Nomor urut (disimpan di byte pertama data) dari frame yang diteruskan.
    fn sequences(frames: &[EncodedFrame]) -> Vec<u8> {
        frames.iter().map(|frame| frame.data[0]).collect()
    }

    #[test]
    fn in_order_frames_pass_straight_through() {
        let mut order = FrameOrder::default();
        assert_eq!(sequences(&order.push(0, frame(0, true))), [0]);
        assert_eq!(sequences(&order.push(1, frame(1, false))), [1]);
        assert!(!order.needs_keyframe());
    }

    #[test]
    fn nothing_is_delivered_before_the_first_keyframe() {
        let mut order = FrameOrder::default();
        assert!(order.push(3, frame(3, false)).is_empty());
        assert!(order.needs_keyframe());
        assert_eq!(sequences(&order.push(4, frame(4, true))), [4]);
    }

    #[test]
    fn swapped_frames_are_put_back_in_order() {
        let mut order = FrameOrder::default();
        order.push(0, frame(0, true));
        assert!(order.push(2, frame(2, false)).is_empty());
        assert_eq!(sequences(&order.push(1, frame(1, false))), [1, 2]);
    }

    #[test]
    fn stuck_frame_forces_a_jump_to_the_next_keyframe() {
        let mut order = FrameOrder::default();
        order.push(0, frame(0, true));
        // Frame 1 tidak pernah datang; frame 2..=5 menumpuk.
        for sequence in 2..=5 {
            assert!(order.push(sequence, frame(sequence, false)).is_empty());
        }
        assert!(order.push(6, frame(6, false)).is_empty());
        assert!(order.needs_keyframe(), "harus meminta keyframe");

        assert_eq!(sequences(&order.push(7, frame(7, true))), [7]);
        assert!(order.push(1, frame(1, false)).is_empty(), "frame lama yang terlambat dibuang");
        assert_eq!(sequences(&order.push(8, frame(8, false))), [8]);
    }

    #[test]
    fn resync_drops_waiting_frames_until_a_keyframe() {
        let mut order = FrameOrder::default();
        order.push(0, frame(0, true));
        order.resync();
        assert!(order.push(1, frame(1, false)).is_empty());
        assert_eq!(sequences(&order.push(2, frame(2, true))), [2]);
    }
}
