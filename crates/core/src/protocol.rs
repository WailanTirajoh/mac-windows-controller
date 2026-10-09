//! Protokol antar-perangkat: jenis stream QUIC dan pesan kanal kontrol.
//!
//! Tiap stream diawali satu byte [`StreamKind`]. Pesan kontrol dikirim sebagai
//! panjang (u32 big-endian) diikuti JSON.

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::trust::Os;

pub const ALPN: &[u8] = b"sambung/1";
pub const PROTOCOL_VERSION: u32 = 1;
pub const DEFAULT_PORT: u16 = 47823;

/// Batas ukuran satu pesan kontrol. Cukup untuk clipboard 1 MB yang di-escape.
pub const MAX_CONTROL_MESSAGE: usize = 4 * 1024 * 1024;

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

    Ping,
    Pong,
    /// Permintaan ditolak karena pengirim belum di-pair.
    NotPaired,
}

pub async fn write_message<W>(writer: &mut W, message: &ControlMessage) -> anyhow::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let body = serde_json::to_vec(message)?;
    anyhow::ensure!(body.len() <= MAX_CONTROL_MESSAGE, "pesan kontrol terlalu besar");
    writer.write_all(&(body.len() as u32).to_be_bytes()).await?;
    writer.write_all(&body).await?;
    Ok(())
}

/// Membaca satu pesan. `Ok(None)` berarti stream ditutup dengan rapi di batas pesan.
pub async fn read_message<R>(reader: &mut R) -> anyhow::Result<Option<ControlMessage>>
where
    R: AsyncRead + Unpin,
{
    let mut length = [0u8; 4];
    match reader.read_exact(&mut length).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let length = u32::from_be_bytes(length) as usize;
    anyhow::ensure!(length <= MAX_CONTROL_MESSAGE, "pesan kontrol terlalu besar ({length} byte)");

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
