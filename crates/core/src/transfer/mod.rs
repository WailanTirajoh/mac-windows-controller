//! Transfer file antar perangkat yang sudah di-pair.
//!
//! Tiap file dikirim lewat satu stream dua arah: tawaran (nama dan ukuran),
//! isi file, lalu SHA-256 dari isinya. Penerima menulis ke berkas `.part`,
//! memeriksa hash, baru memberi nama akhir dan membalas hasilnya.

pub mod filename;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::protocol::{read_frame, write_frame};

const CHUNK_BYTES: usize = 64 * 1024;
const DIGEST_BYTES: usize = 32;

/// Pesan untuk transfer yang dihentikan pengguna, di sisi mana pun.
pub const CANCELLED: &str = "dibatalkan";

/// Awal stream file: apa yang akan dikirim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileOffer {
    pub name: String,
    pub size: u64,
}

/// Balasan penerima setelah seluruh file diterima dan diperiksa.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
enum FileReply {
    Saved,
    Failed { reason: String },
}

/// Mengirim file di `path` lewat stream yang sudah dibuka. `on_progress`
/// dipanggil dengan jumlah byte yang sudah terkirim.
pub async fn send_file<W, R>(
    send: &mut W,
    recv: &mut R,
    path: &Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u64),
) -> anyhow::Result<()>
where
    W: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
{
    let mut file = fs::File::open(path).await.context("file tidak bisa dibuka")?;
    let size = file.metadata().await?.len();
    let name = path
        .file_name()
        .context("path bukan file")?
        .to_string_lossy()
        .into_owned();
    write_frame(send, &FileOffer { name, size }).await?;

    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; CHUNK_BYTES];
    let mut sent = 0u64;
    while sent < size {
        anyhow::ensure!(!cancel.load(Ordering::Relaxed), CANCELLED);
        let wanted = buffer.len().min((size - sent) as usize);
        let read = file.read(&mut buffer[..wanted]).await?;
        anyhow::ensure!(read > 0, "file mengecil saat sedang dikirim");
        send.write_all(&buffer[..read]).await?;
        hasher.update(&buffer[..read]);
        sent += read as u64;
        on_progress(sent);
    }
    send.write_all(&hasher.finalize()).await?;
    send.flush().await?;

    match read_frame::<_, FileReply>(recv).await? {
        Some(FileReply::Saved) => Ok(()),
        Some(FileReply::Failed { reason }) => anyhow::bail!("perangkat tujuan menolak file: {reason}"),
        None => anyhow::bail!("perangkat tujuan menutup koneksi sebelum file tersimpan"),
    }
}

/// File yang sedang diterima. Berkas `.part`-nya dihapus kalau penerimaan
/// tidak sampai selesai.
struct Incoming {
    part: PathBuf,
    keep: bool,
}

impl Drop for Incoming {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(&self.part);
        }
    }
}

/// Membaca tawaran file dari stream. Nama di dalamnya belum dibersihkan.
pub async fn read_offer<R: AsyncRead + Unpin>(recv: &mut R) -> anyhow::Result<FileOffer> {
    read_frame(recv).await?.context("stream file ditutup sebelum ada tawaran")
}

/// Menerima isi file untuk `offer` ke dalam `dir` dan membalas hasilnya ke
/// pengirim. Mengembalikan path akhir file.
pub async fn receive_file<W, R>(
    send: &mut W,
    recv: &mut R,
    offer: &FileOffer,
    dir: &Path,
    cancel: &AtomicBool,
    on_progress: impl FnMut(u64),
) -> anyhow::Result<PathBuf>
where
    W: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
{
    let result = receive_into(recv, offer, dir, cancel, on_progress).await;
    let reply = match &result {
        Ok(_) => FileReply::Saved,
        Err(error) => FileReply::Failed {
            reason: format!("{error:#}"),
        },
    };
    // Pengirim mungkin sudah pergi; hasil di sisi ini tetap berlaku.
    let _ = write_frame(send, &reply).await;
    let _ = send.flush().await;
    result
}

async fn receive_into<R: AsyncRead + Unpin>(
    recv: &mut R,
    offer: &FileOffer,
    dir: &Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u64),
) -> anyhow::Result<PathBuf> {
    fs::create_dir_all(dir).await.context("folder unduhan tidak bisa dibuat")?;
    let name = filename::sanitize(&offer.name);

    let (mut file, incoming) = create_part(dir, &name).await?;
    let mut incoming = incoming;

    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; CHUNK_BYTES];
    let mut received = 0u64;
    while received < offer.size {
        anyhow::ensure!(!cancel.load(Ordering::Relaxed), CANCELLED);
        let wanted = buffer.len().min((offer.size - received) as usize);
        let read = recv.read(&mut buffer[..wanted]).await?;
        anyhow::ensure!(read > 0, "kiriman terputus sebelum selesai");
        file.write_all(&buffer[..read]).await.context("gagal menulis file")?;
        hasher.update(&buffer[..read]);
        received += read as u64;
        on_progress(received);
    }
    file.flush().await?;
    drop(file);

    let mut expected = [0u8; DIGEST_BYTES];
    recv.read_exact(&mut expected)
        .await
        .context("kiriman terputus sebelum hash diterima")?;
    anyhow::ensure!(hasher.finalize().as_slice() == expected, "isi file rusak di perjalanan");

    let saved = publish(&incoming.part, dir, &name)?;
    incoming.keep = true;
    Ok(saved)
}

/// Membuat berkas `.part` baru untuk `name`, tanpa menimpa berkas yang ada.
async fn create_part(dir: &Path, name: &str) -> anyhow::Result<(fs::File, Incoming)> {
    for attempt in 0.. {
        let mut part = filename::candidate(dir, name, attempt).into_os_string();
        part.push(".part");
        let part = PathBuf::from(part);
        match fs::OpenOptions::new().write(true).create_new(true).open(&part).await {
            Ok(file) => return Ok((file, Incoming { part, keep: false })),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("file tidak bisa dibuat di folder unduhan"),
        }
    }
    unreachable!("pencarian nama berhenti lewat return")
}

/// Memberi `part` nama akhirnya tanpa menimpa file lain: mencoba "nama",
/// "nama (1)", dan seterusnya.
fn publish(part: &Path, dir: &Path, name: &str) -> anyhow::Result<PathBuf> {
    for attempt in 0.. {
        let target = filename::candidate(dir, name, attempt);
        // Hard link gagal kalau target sudah ada, jadi tidak ada jeda antara
        // memeriksa dan memberi nama.
        match std::fs::hard_link(part, &target) {
            Ok(()) => {
                std::fs::remove_file(part)?;
                return Ok(target);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            // Filesystem tanpa hard link (mis. exFAT): periksa lalu rename.
            Err(_) if !target.exists() => {
                std::fs::rename(part, &target).context("file tidak bisa diberi nama akhir")?;
                return Ok(target);
            }
            Err(_) => continue,
        }
    }
    unreachable!("pencarian nama berhenti lewat return")
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    /// Mengirim `content` bernama `name` dari satu ujung dan menerimanya di
    /// folder sementara; mengembalikan hasil kedua sisi.
    async fn transfer(name: &str, content: &[u8], dir: &Path) -> (anyhow::Result<()>, anyhow::Result<PathBuf>) {
        let source_dir = TempDir::new().unwrap();
        let source = source_dir.path().join(name);
        std::fs::write(&source, content).unwrap();

        let (sender_io, receiver_io) = tokio::io::duplex(256 * 1024);
        let (mut sender_recv, mut sender_send) = tokio::io::split(sender_io);
        let (mut receiver_recv, mut receiver_send) = tokio::io::split(receiver_io);
        let never = AtomicBool::new(false);

        let sending = send_file(&mut sender_send, &mut sender_recv, &source, &never, |_| {});
        let receiving = async {
            let offer = read_offer(&mut receiver_recv).await?;
            receive_file(&mut receiver_send, &mut receiver_recv, &offer, dir, &never, |_| {}).await
        };
        tokio::join!(sending, receiving)
    }

    /// Menulis kiriman mentah ke penerima, untuk meniru pengirim yang tidak jujur.
    async fn receive_raw(offer: FileOffer, body: &[u8], dir: &Path) -> anyhow::Result<PathBuf> {
        let (mut sender, receiver_io) = tokio::io::duplex(256 * 1024);
        let (mut receiver_recv, mut receiver_send) = tokio::io::split(receiver_io);
        write_frame(&mut sender, &offer).await.unwrap();
        sender.write_all(body).await.unwrap();
        sender.shutdown().await.unwrap();

        let never = AtomicBool::new(false);
        let offer = read_offer(&mut receiver_recv).await?;
        receive_file(&mut receiver_send, &mut receiver_recv, &offer, dir, &never, |_| {}).await
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn with_digest(content: &[u8]) -> Vec<u8> {
        let mut body = content.to_vec();
        body.extend_from_slice(&Sha256::digest(content));
        body
    }

    #[tokio::test]
    async fn file_arrives_intact_under_its_own_name() {
        let dir = TempDir::new().unwrap();
        let content: Vec<u8> = (0..300_000u32).map(|index| (index % 251) as u8).collect();

        let (sent, received) = transfer("data.bin", &content, dir.path()).await;

        sent.unwrap();
        let saved = received.unwrap();
        assert_eq!(saved, dir.path().join("data.bin"));
        assert_eq!(std::fs::read(saved).unwrap(), content);
        assert_eq!(listing(dir.path()), ["data.bin"], "berkas .part tidak boleh tersisa");
    }

    #[tokio::test]
    async fn empty_file_is_transferred() {
        let dir = TempDir::new().unwrap();
        let (sent, received) = transfer("kosong.txt", b"", dir.path()).await;
        sent.unwrap();
        assert_eq!(std::fs::read(received.unwrap()).unwrap(), b"");
    }

    #[tokio::test]
    async fn existing_file_is_not_overwritten() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("catatan.txt"), b"lama").unwrap();

        let (_, first) = transfer("catatan.txt", b"baru", dir.path()).await;
        let (_, second) = transfer("catatan.txt", b"lebih baru", dir.path()).await;

        assert_eq!(first.unwrap(), dir.path().join("catatan (1).txt"));
        assert_eq!(second.unwrap(), dir.path().join("catatan (2).txt"));
        assert_eq!(std::fs::read(dir.path().join("catatan.txt")).unwrap(), b"lama");
    }

    #[tokio::test]
    async fn offered_path_cannot_escape_the_download_folder() {
        let outer = TempDir::new().unwrap();
        let dir = outer.path().join("unduhan");
        let offer = FileOffer {
            name: "../../evil.txt".to_owned(),
            size: 4,
        };

        let saved = receive_raw(offer, &with_digest(b"isi!"), &dir).await.unwrap();

        assert_eq!(saved, dir.join("evil.txt"));
        assert_eq!(listing(outer.path()), ["unduhan"], "tidak ada file di luar folder unduhan");
    }

    #[tokio::test]
    async fn corrupted_content_is_rejected_and_cleaned_up() {
        let dir = TempDir::new().unwrap();
        let offer = FileOffer {
            name: "rusak.bin".to_owned(),
            size: 4,
        };
        let mut body = with_digest(b"isi!");
        body[0] ^= 0xFF;

        let error = receive_raw(offer, &body, dir.path()).await.unwrap_err();

        assert!(error.to_string().contains("rusak"), "{error}");
        assert!(listing(dir.path()).is_empty());
    }

    #[tokio::test]
    async fn truncated_transfer_leaves_nothing_behind() {
        let dir = TempDir::new().unwrap();
        let offer = FileOffer {
            name: "putus.bin".to_owned(),
            size: 1000,
        };

        let error = receive_raw(offer, b"baru sebagian", dir.path()).await.unwrap_err();

        assert!(error.to_string().contains("terputus"), "{error}");
        assert!(listing(dir.path()).is_empty());
    }

    #[tokio::test]
    async fn sender_is_told_when_the_receiver_fails() {
        let blocked = TempDir::new().unwrap();
        // Folder unduhan tidak bisa dibuat karena path-nya ditempati sebuah file.
        let dir = blocked.path().join("bukan-folder");
        std::fs::write(&dir, b"").unwrap();

        let (sent, received) = transfer("data.bin", b"isi", &dir).await;

        assert!(received.is_err());
        let error = sent.unwrap_err().to_string();
        assert!(error.contains("perangkat tujuan menolak file"), "{error}");
    }

    #[tokio::test]
    async fn cancelled_receive_removes_the_partial_file() {
        let dir = TempDir::new().unwrap();
        let (mut sender, receiver_io) = tokio::io::duplex(256 * 1024);
        let (mut receiver_recv, mut receiver_send) = tokio::io::split(receiver_io);
        let offer = FileOffer {
            name: "batal.bin".to_owned(),
            size: 1_000_000,
        };
        write_frame(&mut sender, &offer).await.unwrap();
        sender.write_all(&[7u8; 100_000]).await.unwrap();

        let cancel = AtomicBool::new(false);
        let offer = read_offer(&mut receiver_recv).await.unwrap();
        let result = receive_file(&mut receiver_send, &mut receiver_recv, &offer, dir.path(), &cancel, |received| {
            // Dibatalkan pengguna setelah sebagian data masuk.
            if received >= 50_000 {
                cancel.store(true, Ordering::Relaxed);
            }
        })
        .await;

        assert_eq!(result.unwrap_err().to_string(), CANCELLED);
        assert!(listing(dir.path()).is_empty());
    }
}
