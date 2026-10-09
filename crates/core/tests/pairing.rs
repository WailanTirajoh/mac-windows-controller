//! Pairing dan kepercayaan antar dua node sungguhan di localhost.

use std::net::SocketAddr;
use std::time::Duration;

use sambung_core::identity::Identity;
use sambung_core::net::{self, Link};
use sambung_core::node::{ConnectError, Node, NodeConfig, NodeEvent, PairError, Peer};
use sambung_core::pairing::otp::OtpPolicy;
use sambung_core::protocol::{read_message, write_message, ControlMessage, PairFailure, StreamKind, PROTOCOL_VERSION};
use sambung_core::trust::Os;
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc::UnboundedReceiver;

const LOCALHOST: &str = "127.0.0.1:0";
const WAIT: Duration = Duration::from_secs(5);

struct TestNode {
    node: Node,
    events: UnboundedReceiver<NodeEvent>,
    address: SocketAddr,
    _data: TempDir,
}

impl TestNode {
    fn start(name: &str) -> Self {
        Self::start_with(name, OtpPolicy::default())
    }

    fn start_with(name: &str, otp_policy: OtpPolicy) -> Self {
        let data = TempDir::new().unwrap();
        let (node, events) = Node::start(NodeConfig {
            data_dir: data.path().to_owned(),
            device_name: name.to_owned(),
            bind_address: LOCALHOST.parse().unwrap(),
            otp_policy,
        })
        .unwrap();
        let address = node.local_address().unwrap();
        Self {
            node,
            events,
            address,
            _data: data,
        }
    }

    async fn next_event(&mut self) -> NodeEvent {
        tokio::time::timeout(WAIT, self.events.recv())
            .await
            .expect("event tidak datang")
            .expect("node sudah berhenti")
    }

    /// Menunggu dialog OTP muncul dan mengembalikan kodenya.
    async fn shown_code(&mut self) -> String {
        loop {
            if let NodeEvent::PairingRequested { code, .. } = self.next_event().await {
                return code;
            }
        }
    }

    /// Menunggu dialog OTP ditutup dan mengembalikan apakah pairing berhasil.
    async fn dialog_result(&mut self) -> bool {
        loop {
            if let NodeEvent::PairingFinished { paired, .. } = self.next_event().await {
                return paired;
            }
        }
    }
}

/// Kode yang pasti berbeda dari `code`.
fn wrong(code: &str) -> String {
    format!("{:06}", (code.parse::<u32>().unwrap() + 1) % 1_000_000)
}

/// `a` terhubung ke `b` dan menyelesaikan pairing dengan kode yang benar.
async fn pair(a: &TestNode, b: &mut TestNode) -> Peer {
    let peer = a.node.connect(b.address, None).await.unwrap();
    peer.request_pairing().await.unwrap();
    let code = b.shown_code().await;
    peer.submit_code(&code, true).await.unwrap();
    peer
}

fn rejected(result: Result<(), PairError>) -> PairFailure {
    match result {
        Err(PairError::Rejected(failure)) => failure,
        other => panic!("seharusnya ditolak, hasilnya {other:?}"),
    }
}

#[tokio::test]
async fn correct_code_makes_both_devices_trust_each_other() {
    let a = TestNode::start("MacBook");
    let mut b = TestNode::start("PC");

    let peer = a.node.connect(b.address, None).await.unwrap();
    assert!(!peer.is_paired());
    assert_eq!(peer.info.name, "PC");
    assert!(!peer.ping().await.unwrap(), "sebelum pairing, permintaan lain ditolak");

    let expires_in = peer.request_pairing().await.unwrap();
    assert_eq!(expires_in, Duration::from_secs(120));
    let code = b.shown_code().await;
    peer.submit_code(&code, true).await.unwrap();

    assert!(peer.is_paired());
    assert!(peer.ping().await.unwrap());
    assert!(b.dialog_result().await);

    let stored_on_a = a.node.trusted_devices();
    assert_eq!(stored_on_a.len(), 1);
    assert_eq!(stored_on_a[0].fingerprint, b.node.info().fingerprint);
    assert_eq!(stored_on_a[0].name, "PC");
    assert_eq!(stored_on_a[0].os, Os::current());
    assert_eq!(stored_on_a[0].last_address, Some(b.address));

    let stored_on_b = b.node.trusted_devices();
    assert_eq!(stored_on_b.len(), 1);
    assert_eq!(stored_on_b[0].fingerprint, a.node.info().fingerprint);
    assert!(stored_on_b[0].allow_incoming);
    assert_eq!(
        stored_on_b[0].last_address,
        Some(a.address),
        "B harus bisa menghubungi A balik lewat alamat asal koneksi"
    );
}

#[tokio::test]
async fn paired_devices_reconnect_without_a_code() {
    let a = TestNode::start("MacBook");
    let mut b = TestNode::start("PC");
    drop(pair(&a, &mut b).await);

    let peer = a.node.connect(b.address, Some(b.node.info().fingerprint)).await.unwrap();

    assert!(peer.is_paired());
    assert!(peer.ping().await.unwrap());
}

#[tokio::test]
async fn pairing_works_in_the_reverse_direction_too() {
    let a = TestNode::start("MacBook");
    let mut b = TestNode::start("PC");
    drop(pair(&a, &mut b).await);

    // B (yang tadi menampilkan kode) sekarang menjadi pengendali.
    let peer = b.node.connect(a.address, Some(a.node.info().fingerprint)).await.unwrap();

    assert!(peer.is_paired());
    assert!(peer.ping().await.unwrap());
}

#[tokio::test]
async fn reverse_control_is_refused_when_not_allowed() {
    let a = TestNode::start("MacBook");
    let mut b = TestNode::start("PC");
    let peer = a.node.connect(b.address, None).await.unwrap();
    peer.request_pairing().await.unwrap();
    let code = b.shown_code().await;
    peer.submit_code(&code, false).await.unwrap();

    let reverse = b.node.connect(a.address, Some(a.node.info().fingerprint)).await.unwrap();

    assert!(!reverse.is_paired());
    assert!(!reverse.ping().await.unwrap());
}

#[tokio::test]
async fn wrong_code_is_rejected_and_third_failure_revokes_the_code() {
    let a = TestNode::start("MacBook");
    let mut b = TestNode::start("PC");
    let peer = a.node.connect(b.address, None).await.unwrap();
    peer.request_pairing().await.unwrap();
    let code = b.shown_code().await;

    for attempts_left in [2, 1, 0] {
        let failure = rejected(peer.submit_code(&wrong(&code), true).await);
        assert_eq!(failure, PairFailure::WrongCode { attempts_left });
    }
    assert!(!b.dialog_result().await, "dialog ditutup setelah kode dicabut");

    // Kode yang benar pun sudah tidak berlaku.
    assert_eq!(rejected(peer.submit_code(&code, true).await), PairFailure::Expired);
    assert!(a.node.trusted_devices().is_empty());
    assert!(b.node.trusted_devices().is_empty());
    assert!(!peer.is_paired());
}

#[tokio::test]
async fn correct_code_after_a_mistake_still_pairs() {
    let a = TestNode::start("MacBook");
    let mut b = TestNode::start("PC");
    let peer = a.node.connect(b.address, None).await.unwrap();
    peer.request_pairing().await.unwrap();
    let code = b.shown_code().await;

    rejected(peer.submit_code(&wrong(&code), true).await);
    peer.submit_code(&code, true).await.unwrap();

    assert!(peer.is_paired());
    assert_eq!(b.node.trusted_devices().len(), 1);
}

#[tokio::test]
async fn expired_code_is_rejected() {
    let a = TestNode::start("MacBook");
    let mut b = TestNode::start_with(
        "PC",
        OtpPolicy {
            ttl: Duration::from_millis(50),
            ..OtpPolicy::default()
        },
    );
    let peer = a.node.connect(b.address, None).await.unwrap();
    peer.request_pairing().await.unwrap();
    let code = b.shown_code().await;

    tokio::time::sleep(Duration::from_millis(120)).await;

    assert_eq!(rejected(peer.submit_code(&code, true).await), PairFailure::Expired);
    assert!(b.node.trusted_devices().is_empty());
}

#[tokio::test]
async fn code_is_single_use() {
    let a = TestNode::start("MacBook");
    let other = TestNode::start("Laptop lain");
    let mut b = TestNode::start("PC");
    let peer = a.node.connect(b.address, None).await.unwrap();
    peer.request_pairing().await.unwrap();
    let code = b.shown_code().await;
    peer.submit_code(&code, true).await.unwrap();

    // Perangkat lain yang mengetahui kode yang sama tidak bisa memakainya.
    let intruder = other.node.connect(b.address, None).await.unwrap();
    assert_eq!(rejected(intruder.submit_code(&code, true).await), PairFailure::Expired);
    assert_eq!(b.node.trusted_devices().len(), 1);
}

#[tokio::test]
async fn repeated_failures_close_pairing_temporarily() {
    let a = TestNode::start("MacBook");
    let mut b = TestNode::start("PC");
    let peer = a.node.connect(b.address, None).await.unwrap();

    // Lima tebakan salah, lintas dua kode.
    let mut failures = 0;
    while failures < 5 {
        peer.request_pairing().await.unwrap();
        let code = b.shown_code().await;
        for _ in 0..3 {
            if failures == 5 {
                break;
            }
            rejected(peer.submit_code(&wrong(&code), true).await);
            failures += 1;
        }
    }

    match peer.request_pairing().await {
        Err(PairError::Rejected(PairFailure::Cooldown { retry_after_secs })) => {
            assert!((1..=60).contains(&retry_after_secs));
        }
        other => panic!("seharusnya ditutup sementara, hasilnya {other:?}"),
    }
}

#[tokio::test]
async fn device_not_accepting_connections_rejects_pairing() {
    let a = TestNode::start("MacBook");
    let b = TestNode::start("PC");
    b.node.set_accept_incoming(false);

    let peer = a.node.connect(b.address, None).await.unwrap();

    match peer.request_pairing().await {
        Err(PairError::Rejected(PairFailure::Disabled)) => {}
        other => panic!("seharusnya ditolak, hasilnya {other:?}"),
    }
}

#[tokio::test]
async fn forgotten_device_must_pair_again() {
    let a = TestNode::start("MacBook");
    let mut b = TestNode::start("PC");
    drop(pair(&a, &mut b).await);

    assert!(b.node.forget(&a.node.info().fingerprint).unwrap());

    let peer = a.node.connect(b.address, Some(b.node.info().fingerprint)).await.unwrap();
    assert!(!peer.is_paired());
    assert!(!peer.ping().await.unwrap());
}

#[tokio::test]
async fn closing_the_connection_dismisses_the_code_dialog() {
    let a = TestNode::start("MacBook");
    let mut b = TestNode::start("PC");
    let peer = a.node.connect(b.address, None).await.unwrap();
    peer.request_pairing().await.unwrap();
    b.shown_code().await;

    drop(peer);

    assert!(!b.dialog_result().await);
}

#[tokio::test]
async fn changed_identity_is_detected_on_reconnect() {
    let a = TestNode::start("MacBook");
    let mut b = TestNode::start("PC");
    drop(pair(&a, &mut b).await);
    let impostor = TestNode::start("PC");

    // Alamat yang dituju ternyata dijawab perangkat dengan sertifikat lain.
    let result = a.node.connect(impostor.address, Some(b.node.info().fingerprint)).await;

    assert!(matches!(result, Err(ConnectError::IdentityChanged)));
}

#[tokio::test]
async fn connecting_to_itself_is_refused() {
    let a = TestNode::start("MacBook");
    let result = a.node.connect(a.address, None).await;
    assert!(matches!(result, Err(ConnectError::SelfConnection)));
}

#[tokio::test]
async fn unpaired_device_opening_another_stream_is_disconnected() {
    let b = TestNode::start("PC");
    let identity = Identity::generate().unwrap();
    let endpoint = net::bind(&identity, LOCALHOST.parse().unwrap()).unwrap();
    let link = Link::connect(&endpoint, b.address).await.unwrap();

    // Kanal kontrol dibuka dengan benar...
    let (mut send, mut recv) = link.connection.open_bi().await.unwrap();
    send.write_u8(StreamKind::Control as u8).await.unwrap();
    let hello = ControlMessage::Hello {
        protocol: PROTOCOL_VERSION,
        name: "Tamu".to_owned(),
        os: Os::current(),
        trusts_peer: false,
    };
    write_message(&mut send, &hello).await.unwrap();
    read_message(&mut recv).await.unwrap().unwrap();

    // ...lalu perangkat yang belum di-pair mencoba membuka stream file.
    let mut file = link.connection.open_uni().await.unwrap();
    file.write_u8(StreamKind::File as u8).await.unwrap();

    let closed = tokio::time::timeout(WAIT, link.connection.closed()).await.unwrap();
    match closed {
        quinn::ConnectionError::ApplicationClosed(close) => assert_eq!(close.error_code, 1u32.into()),
        other => panic!("seharusnya diputus oleh tujuan, hasilnya {other:?}"),
    }
}

/// Pihak ketiga yang duduk di antara A dan B, meneruskan kanal kontrol apa
/// adanya lewat dua sesi TLS miliknya sendiri.
async fn relay(endpoint: quinn::Endpoint, target: SocketAddr) -> anyhow::Result<()> {
    let from_a = endpoint.accept().await.unwrap().await?;
    let to_b = Link::connect(&endpoint, target).await?;
    let (mut a_send, mut a_recv) = from_a.accept_bi().await?;
    let (mut b_send, mut b_recv) = to_b.connection.open_bi().await?;
    let _ = tokio::join!(
        tokio::io::copy(&mut a_recv, &mut b_send),
        tokio::io::copy(&mut b_recv, &mut a_send),
    );
    Ok(())
}

#[tokio::test]
async fn relay_in_the_middle_cannot_complete_pairing() {
    let a = TestNode::start("MacBook");
    let mut b = TestNode::start("PC");
    let attacker = Identity::generate().unwrap();
    let endpoint = net::bind(&attacker, LOCALHOST.parse().unwrap()).unwrap();
    let relay_address = endpoint.local_addr().unwrap();
    tokio::spawn(relay(endpoint, b.address));

    // A mengira alamat relay adalah B. Pesan diteruskan utuh, jadi B tetap
    // menampilkan kode dan pengguna mengetiknya dengan benar di A.
    let peer = a.node.connect(relay_address, None).await.unwrap();
    assert_eq!(peer.info.name, "PC");
    peer.request_pairing().await.unwrap();
    let code = b.shown_code().await;
    let result = peer.submit_code(&code, true).await;

    assert!(result.is_err(), "pairing lewat relay harus gagal, hasilnya {result:?}");
    assert!(a.node.trusted_devices().is_empty());
    assert!(b.node.trusted_devices().is_empty());
}
