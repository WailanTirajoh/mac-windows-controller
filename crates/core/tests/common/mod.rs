//! Node sungguhan di localhost untuk test integrasi.

// Tiap berkas test memakai sebagian isi modul ini saja.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::time::Duration;

use sambung_core::node::{Node, NodeConfig, NodeEvent, Peer};
use sambung_core::pairing::otp::OtpPolicy;
use sambung_core::session::SessionServices;
use tempfile::TempDir;
use tokio::sync::mpsc::UnboundedReceiver;

pub const LOCALHOST: &str = "127.0.0.1:0";
pub const WAIT: Duration = Duration::from_secs(5);

pub struct TestNode {
    pub node: Node,
    pub events: UnboundedReceiver<NodeEvent>,
    pub address: SocketAddr,
    _data: TempDir,
}

impl TestNode {
    pub fn start(name: &str) -> Self {
        Self::start_with(name, OtpPolicy::default(), SessionServices::default())
    }

    pub fn start_with(name: &str, otp_policy: OtpPolicy, services: SessionServices) -> Self {
        let data = TempDir::new().unwrap();
        let (node, events) = Node::start(NodeConfig {
            data_dir: data.path().to_owned(),
            device_name: name.to_owned(),
            bind_address: LOCALHOST.parse().unwrap(),
            otp_policy,
            services,
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

    pub async fn next_event(&mut self) -> NodeEvent {
        tokio::time::timeout(WAIT, self.events.recv())
            .await
            .expect("event tidak datang")
            .expect("node sudah berhenti")
    }

    /// Menunggu event pertama yang cocok dengan `pick` dan mengembalikan hasilnya.
    pub async fn wait_for<T>(&mut self, mut pick: impl FnMut(NodeEvent) -> Option<T>) -> T {
        loop {
            if let Some(found) = pick(self.next_event().await) {
                return found;
            }
        }
    }

    /// Menunggu dialog OTP muncul dan mengembalikan kodenya.
    pub async fn shown_code(&mut self) -> String {
        self.wait_for(|event| match event {
            NodeEvent::PairingRequested { code, .. } => Some(code),
            _ => None,
        })
        .await
    }

    /// Menunggu dialog OTP ditutup dan mengembalikan apakah pairing berhasil.
    pub async fn dialog_result(&mut self) -> bool {
        self.wait_for(|event| match event {
            NodeEvent::PairingFinished { paired, .. } => Some(paired),
            _ => None,
        })
        .await
    }
}

/// `a` terhubung ke `b` dan menyelesaikan pairing dengan kode yang benar.
pub async fn pair(a: &TestNode, b: &mut TestNode) -> Peer {
    let peer = a.node.connect(b.address, None).await.unwrap();
    peer.request_pairing().await.unwrap();
    let code = b.shown_code().await;
    peer.submit_code(&code, true).await.unwrap();
    peer
}
