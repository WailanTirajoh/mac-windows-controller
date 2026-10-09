//! Transfer file antar dua node sungguhan di localhost.

mod common;

use std::path::{Path, PathBuf};

use common::{pair, TestNode};
use rand::RngCore;
use sambung_core::node::{NodeEvent, TransferEvent};
use sambung_core::transfer::CANCELLED;
use tempfile::TempDir;

const MEGABYTE: usize = 1024 * 1024;

/// Membuat file berisi `size` byte acak di folder sementara.
fn random_file(dir: &TempDir, name: &str, size: usize) -> (PathBuf, Vec<u8>) {
    let mut content = vec![0u8; size];
    rand::thread_rng().fill_bytes(&mut content);
    let path = dir.path().join(name);
    std::fs::write(&path, &content).unwrap();
    (path, content)
}

/// Membuat file besar berisi nol tanpa menulis isinya (sparse), untuk test
/// yang membatalkan transfer di tengah jalan.
fn large_file(dir: &TempDir, name: &str) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::File::create(&path).unwrap().set_len(1024 * MEGABYTE as u64).unwrap();
    path
}

struct Started {
    id: u64,
    name: String,
    size: u64,
    incoming: bool,
    from: String,
}

async fn started(node: &mut TestNode) -> Started {
    node.wait_for(|event| match event {
        NodeEvent::Transfer(TransferEvent::Started {
            id,
            peer,
            name,
            size,
            incoming,
        }) => Some(Started {
            id,
            name,
            size,
            incoming,
            from: peer.name,
        }),
        _ => None,
    })
    .await
}

/// Menunggu transfer `id` selesai. Mengembalikan hasilnya dan jumlah laporan
/// kemajuan yang diterima sebelum itu.
async fn finished(node: &mut TestNode, id: u64) -> (Result<PathBuf, String>, usize) {
    let mut progress_reports = 0;
    let outcome = node
        .wait_for(|event| match event {
            NodeEvent::Transfer(TransferEvent::Progress { id: reported, .. }) if reported == id => {
                progress_reports += 1;
                None
            }
            NodeEvent::Transfer(TransferEvent::Finished { id: done, outcome }) if done == id => Some(outcome),
            _ => None,
        })
        .await;
    (outcome, progress_reports)
}

async fn progressed(node: &mut TestNode, id: u64) {
    node.wait_for(|event| match event {
        NodeEvent::Transfer(TransferEvent::Progress { id: reported, .. }) if reported == id => Some(()),
        _ => None,
    })
    .await;
}

fn files_in(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test(flavor = "multi_thread")]
async fn large_file_arrives_intact_with_progress_on_both_sides() {
    let mut mac = TestNode::start("MacBook");
    let mut pc = TestNode::start("PC");
    let peer = pair(&mac, &mut pc).await;
    let source = TempDir::new().unwrap();
    let (path, content) = random_file(&source, "arsip.bin", 50 * MEGABYTE);

    let id = peer.send_file(path.clone()).await.unwrap();

    let outgoing = started(&mut mac).await;
    assert_eq!(outgoing.id, id);
    assert_eq!((outgoing.name.as_str(), outgoing.size, outgoing.incoming), ("arsip.bin", content.len() as u64, false));
    let incoming = started(&mut pc).await;
    assert_eq!((incoming.name.as_str(), incoming.size, incoming.incoming), ("arsip.bin", content.len() as u64, true));
    assert_eq!(incoming.from, "MacBook");

    let (sent, sender_reports) = finished(&mut mac, id).await;
    let (received, receiver_reports) = finished(&mut pc, incoming.id).await;
    assert_eq!(sent, Ok(path));
    let saved = received.unwrap();
    assert_eq!(saved, pc.downloads.join("arsip.bin"));
    assert!(std::fs::read(&saved).unwrap() == content, "isi file berbeda dari sumbernya");
    assert!(sender_reports > 0 && receiver_reports > 0, "kemajuan harus dilaporkan");
    assert_eq!(files_in(&pc.downloads), ["arsip.bin"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn files_can_be_sent_in_both_directions() {
    let mut mac = TestNode::start("MacBook");
    let mut pc = TestNode::start("PC");
    drop(pair(&mac, &mut pc).await);
    let source = TempDir::new().unwrap();
    let (path, content) = random_file(&source, "dari-pc.txt", 1000);

    // PC, yang tadi menampilkan kode, sekarang yang mengirim.
    let reverse = pc.node.connect(mac.address, Some(mac.node.info().fingerprint)).await.unwrap();
    reverse.send_file(path).await.unwrap();

    let incoming = started(&mut mac).await;
    let saved = finished(&mut mac, incoming.id).await.0.unwrap();
    assert_eq!(std::fs::read(saved).unwrap(), content);
}

#[tokio::test(flavor = "multi_thread")]
async fn paired_device_without_control_permission_can_still_send_files() {
    let mut mac = TestNode::start("MacBook");
    let mut pc = TestNode::start("PC");
    let peer = mac.node.connect(pc.address, None).await.unwrap();
    peer.request_pairing().await.unwrap();
    let code = pc.shown_code().await;
    // Mac tidak mengizinkan PC mengendalikannya.
    peer.submit_code(&code, false).await.unwrap();
    let source = TempDir::new().unwrap();
    let (path, _) = random_file(&source, "hasil.pdf", 1000);

    let reverse = pc.node.connect(mac.address, Some(mac.node.info().fingerprint)).await.unwrap();
    assert!(!reverse.ping().await.unwrap(), "PC memang tidak boleh mengendalikan Mac");
    reverse.send_file(path).await.unwrap();

    let incoming = started(&mut mac).await;
    assert!(finished(&mut mac, incoming.id).await.0.is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn unpaired_device_cannot_send_files() {
    let mut mac = TestNode::start("MacBook");
    let pc = TestNode::start("PC");
    let peer = mac.node.connect(pc.address, None).await.unwrap();
    let source = TempDir::new().unwrap();
    let (path, _) = random_file(&source, "tamu.txt", 1000);

    let id = peer.send_file(path).await.unwrap();

    assert!(finished(&mut mac, id).await.0.is_err());
    assert!(files_in(&pc.downloads).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn device_not_accepting_connections_rejects_files() {
    let mut mac = TestNode::start("MacBook");
    let mut pc = TestNode::start("PC");
    let peer = pair(&mac, &mut pc).await;
    pc.node.set_accept_incoming(false);
    let source = TempDir::new().unwrap();
    let (path, _) = random_file(&source, "ditolak.txt", 1000);

    let id = peer.send_file(path).await.unwrap();

    assert!(finished(&mut mac, id).await.0.is_err());
    assert!(files_in(&pc.downloads).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn sender_can_cancel_a_transfer_in_progress() {
    let mut mac = TestNode::start("MacBook");
    let mut pc = TestNode::start("PC");
    let peer = pair(&mac, &mut pc).await;
    let source = TempDir::new().unwrap();

    let id = peer.send_file(large_file(&source, "besar.bin")).await.unwrap();
    let incoming = started(&mut pc).await;
    progressed(&mut mac, id).await;
    mac.node.cancel_transfer(id);

    assert_eq!(finished(&mut mac, id).await.0, Err(CANCELLED.to_owned()));
    assert!(finished(&mut pc, incoming.id).await.0.is_err());
    assert!(files_in(&pc.downloads).is_empty(), "file setengah jadi harus dihapus");
}

#[tokio::test(flavor = "multi_thread")]
async fn receiver_can_cancel_a_transfer_in_progress() {
    let mut mac = TestNode::start("MacBook");
    let mut pc = TestNode::start("PC");
    let peer = pair(&mac, &mut pc).await;
    let source = TempDir::new().unwrap();

    let id = peer.send_file(large_file(&source, "besar.bin")).await.unwrap();
    let incoming = started(&mut pc).await;
    progressed(&mut pc, incoming.id).await;
    pc.node.cancel_transfer(incoming.id);

    assert_eq!(finished(&mut pc, incoming.id).await.0, Err(CANCELLED.to_owned()));
    assert!(finished(&mut mac, id).await.0.is_err(), "pengirim harus tahu kirimannya gagal");
    assert!(files_in(&pc.downloads).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn folders_and_missing_files_are_refused_up_front() {
    let mac = TestNode::start("MacBook");
    let mut pc = TestNode::start("PC");
    let peer = pair(&mac, &mut pc).await;
    let source = TempDir::new().unwrap();

    assert!(peer.send_file(source.path().to_owned()).await.is_err());
    assert!(peer.send_file(source.path().join("tidak-ada.txt")).await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn several_files_can_be_sent_at_once() {
    let mac = TestNode::start("MacBook");
    let mut pc = TestNode::start("PC");
    let peer = pair(&mac, &mut pc).await;
    let source = TempDir::new().unwrap();

    let mut expected = Vec::new();
    for index in 0..4 {
        let name = format!("file-{index}.bin");
        let (path, content) = random_file(&source, &name, 2 * MEGABYTE);
        peer.send_file(path).await.unwrap();
        expected.push((name, content));
    }

    let mut done = 0;
    while done < expected.len() {
        let finished = pc
            .wait_for(|event| match event {
                NodeEvent::Transfer(TransferEvent::Finished { outcome, .. }) => Some(outcome),
                _ => None,
            })
            .await;
        finished.unwrap();
        done += 1;
    }
    for (name, content) in expected {
        assert!(std::fs::read(pc.downloads.join(&name)).unwrap() == content, "{name}");
    }
}
