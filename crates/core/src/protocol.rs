//! Protokol antar-perangkat: jenis stream QUIC dan pesan kanal kontrol.
//!
//! Tiap stream diawali satu byte [`StreamKind`]. Pesan kontrol dan event input
//! dikirim sebagai panjang (u32 big-endian) diikuti JSON. Stream video berisi
//! satu frame: [`VideoHeader`] diikuti access unit H.264.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::trust::Os;

pub const ALPN: &[u8] = b"sambung/1";
pub const PROTOCOL_VERSION: u32 = 1;
pub const DEFAULT_PORT: u16 = 47823;

/// Batas ukuran satu pesan kontrol. Cukup untuk clipboard 1 MB yang di-escape.
pub const MAX_CONTROL_MESSAGE: usize = 4 * 1024 * 1024;

/// Batas ukuran satu frame video ter-encode.
pub const MAX_VIDEO_FRAME: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum StreamKind {
    Control = 1,
    Input = 2,
    Video = 3,
    File = 4,
}

impl StreamKind {
    pub const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::Control),
            2 => Some(Self::Input),
            3 => Some(Self::Video),
            4 => Some(Self::File),
            _ => None,
        }
    }
}

/// Alasan pairing ditolak atau gagal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum PairFailure {
    #[error("kode salah, sisa percobaan: {attempts_left}")]
    WrongCode { attempts_left: u8 },
    #[error("kode sudah kedaluwarsa atau tidak ada permintaan pairing aktif")]
    Expired,
    #[error("terlalu banyak percobaan, coba lagi dalam {retry_after_secs} detik")]
    Cooldown { retry_after_secs: u64 },
    #[error("perangkat tujuan tidak menerima koneksi baru")]
    Disabled,
    #[error("urutan pesan pairing tidak valid")]
    Protocol,
}

/// Monitor di perangkat tujuan yang bisa dipilih pengendali.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayDesc {
    pub id: u32,
    /// Ukuran dalam piksel fisik.
    pub width: u32,
    pub height: u32,
    pub is_primary: bool,
}

/// Kualitas video yang diminta pengendali.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoParams {
    pub max_long_edge: u32,
    pub max_fps: u32,
    pub bitrate_bps: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Event input dari pengendali. Posisi dinyatakan 0..1 relatif terhadap
/// monitor yang sedang ditampilkan, jadi tidak bergantung resolusi video.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum InputEvent {
    Move { x: f64, y: f64 },
    Button { button: MouseButton, down: bool, x: f64, y: f64 },
    /// Jarak scroll dalam piksel; positif berarti ke kanan / ke bawah.
    Scroll { dx: f64, dy: f64 },
    /// `code` adalah `KeyboardEvent.code`, yaitu posisi fisik tombol.
    Key { code: String, down: bool },
}

/// Awal tiap stream video, setelah byte jenis stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoHeader {
    /// Nomor urut frame sejak sesi dimulai.
    pub sequence: u64,
    pub timestamp_us: u64,
    pub keyframe: bool,
    pub width: u32,
    pub height: u32,
}

impl VideoHeader {
    pub const BYTES: usize = 25;

    pub fn encode(&self) -> [u8; Self::BYTES] {
        let mut bytes = [0u8; Self::BYTES];
        bytes[0..8].copy_from_slice(&self.sequence.to_le_bytes());
        bytes[8..16].copy_from_slice(&self.timestamp_us.to_le_bytes());
        bytes[16] = u8::from(self.keyframe);
        bytes[17..21].copy_from_slice(&self.width.to_le_bytes());
        bytes[21..25].copy_from_slice(&self.height.to_le_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8; Self::BYTES]) -> Self {
        let u64_at = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().expect("8 byte"));
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 byte"));
        Self {
            sequence: u64_at(0),
            timestamp_us: u64_at(8),
            keyframe: bytes[16] & 1 != 0,
            width: u32_at(17),
            height: u32_at(21),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlMessage {
    /// Pesan pertama dari kedua sisi.
    Hello {
        protocol: u32,
        name: String,
        os: Os,
        /// Dari pengendali: apakah ia sudah menyimpan perangkat tujuan.
        /// Dari tujuan: apakah pengendali boleh mengendalikannya tanpa pairing.
        trusts_peer: bool,
    },

    /// Pengendali meminta pairing; tujuan menampilkan OTP.
    PairRequest,
    /// Tujuan sudah menampilkan OTP.
    PairChallenge { expires_in_secs: u64 },
    /// Pesan SPAKE2 dari pengendali, dibuat dari OTP yang diketik pengguna.
    PairPakeA { message: Vec<u8> },
    /// Pesan SPAKE2 dari tujuan.
    PairPakeB { message: Vec<u8> },
    /// Bukti dari pengendali bahwa ia menurunkan kunci yang sama.
    PairConfirmA { confirm: Vec<u8> },
    /// Bukti dari tujuan; pairing selesai.
    PairDone { confirm: Vec<u8> },
    PairFailed(PairFailure),

    /// Pengendali meminta sesi remote desktop dimulai.
    SessionStart {
        /// Monitor yang diminta; `None` berarti monitor utama.
        display_id: Option<u32>,
        video: VideoParams,
    },
    SessionStarted {
        displays: Vec<DisplayDesc>,
        display_id: u32,
        /// `false` berarti tujuan tidak bisa menyuntikkan input (mis. izin
        /// belum diberikan), jadi sesi hanya untuk melihat.
        input_enabled: bool,
    },
    SessionFailed { reason: String },
    /// Pengendali meminta pindah monitor.
    SelectDisplay { display_id: u32 },
    /// Tujuan sudah pindah ke monitor ini.
    DisplayChanged { display_id: u32 },
    /// Pengendali kehilangan frame dan butuh keyframe baru.
    RequestKeyframe,
    /// Sesi diakhiri oleh salah satu sisi.
    SessionStop,
    /// Teks yang baru disalin di sisi pengirim, selama sesi berjalan.
    Clipboard { text: String },

    Ping,
    Pong,
    /// Permintaan ditolak karena pengirim belum di-pair.
    NotPaired,
}

pub async fn write_message<W>(writer: &mut W, message: &ControlMessage) -> anyhow::Result<()>
where
    W: AsyncWrite + Unpin,
{
    write_frame(writer, message).await
}

/// Membaca satu pesan. `Ok(None)` berarti stream ditutup dengan rapi di batas pesan.
pub async fn read_message<R>(reader: &mut R) -> anyhow::Result<Option<ControlMessage>>
where
    R: AsyncRead + Unpin,
{
    read_frame(reader).await
}

/// Menulis satu nilai sebagai panjang + JSON.
pub async fn write_frame<W, T>(writer: &mut W, value: &T) -> anyhow::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let body = serde_json::to_vec(value)?;
    anyhow::ensure!(body.len() <= MAX_CONTROL_MESSAGE, "pesan terlalu besar");
    writer.write_all(&(body.len() as u32).to_be_bytes()).await?;
    writer.write_all(&body).await?;
    Ok(())
}

/// Membaca satu nilai. `Ok(None)` berarti stream ditutup dengan rapi di batas pesan.
pub async fn read_frame<R, T>(reader: &mut R) -> anyhow::Result<Option<T>>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut length = [0u8; 4];
    match reader.read_exact(&mut length).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let length = u32::from_be_bytes(length) as usize;
    anyhow::ensure!(length <= MAX_CONTROL_MESSAGE, "pesan terlalu besar ({length} byte)");

    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).await?;
    Ok(Some(serde_json::from_slice(&body)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn messages_round_trip_in_order() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        let sent = [
            ControlMessage::PairRequest,
            ControlMessage::PairFailed(PairFailure::WrongCode { attempts_left: 2 }),
            ControlMessage::PairPakeA { message: vec![1, 2, 3] },
        ];
        for message in &sent {
            write_message(&mut a, message).await.unwrap();
        }
        drop(a);

        for message in &sent {
            assert_eq!(read_message(&mut b).await.unwrap().as_ref(), Some(message));
        }
        assert_eq!(read_message(&mut b).await.unwrap(), None);
    }

    #[tokio::test]
    async fn input_events_round_trip() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        let sent = [
            InputEvent::Move { x: 0.25, y: 0.75 },
            InputEvent::Button {
                button: MouseButton::Right,
                down: true,
                x: 1.0,
                y: 0.0,
            },
            InputEvent::Scroll { dx: 0.0, dy: -120.0 },
            InputEvent::Key {
                code: "KeyA".to_owned(),
                down: false,
            },
        ];
        for event in &sent {
            write_frame(&mut a, event).await.unwrap();
        }
        drop(a);

        for event in &sent {
            let received: Option<InputEvent> = read_frame(&mut b).await.unwrap();
            assert_eq!(received.as_ref(), Some(event));
        }
    }

    #[test]
    fn video_header_round_trips() {
        let header = VideoHeader {
            sequence: u64::MAX - 1,
            timestamp_us: 123_456_789,
            keyframe: true,
            width: 1920,
            height: 1248,
        };
        assert_eq!(VideoHeader::decode(&header.encode()), header);
    }

    #[tokio::test]
    async fn oversized_length_prefix_is_rejected_before_allocating() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&u32::MAX.to_be_bytes()).await.unwrap();
        assert!(read_message(&mut b).await.is_err());
    }

    #[tokio::test]
    async fn truncated_message_is_an_error() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&10u32.to_be_bytes()).await.unwrap();
        a.write_all(b"{\"ty").await.unwrap();
        drop(a);
        assert!(read_message(&mut b).await.is_err());
    }
}
