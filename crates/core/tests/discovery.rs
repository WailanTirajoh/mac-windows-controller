//! Dua perangkat saling menemukan lewat mDNS.
//!
//! Butuh jaringan yang meneruskan multicast, jadi tidak ikut `cargo test`
//! biasa. Jalankan dengan `cargo test -p sambung-core --test discovery -- --ignored`.

use std::time::Duration;

use sambung_core::discovery::{Discovery, DiscoveryEvent};
use sambung_core::identity::Identity;
use sambung_core::node::DeviceInfo;
use sambung_core::trust::Os;
use tokio::sync::mpsc::UnboundedReceiver;

fn device(name: &str) -> DeviceInfo {
    DeviceInfo {
        fingerprint: Identity::generate().unwrap().fingerprint(),
        name: name.to_owned(),
        os: Os::current(),
    }
}

/// Menunggu sampai `events` melaporkan perangkat dengan fingerprint milik `wanted`.
async fn wait_found(events: &mut UnboundedReceiver<DiscoveryEvent>, wanted: &DeviceInfo) -> Vec<std::net::SocketAddr> {
    let search = async {
        loop {
            match events.recv().await.expect("discovery berhenti") {
                DiscoveryEvent::Found(found) if found.info.fingerprint == wanted.fingerprint => {
                    assert_eq!(found.info, *wanted);
                    return found.addresses;
                }
                _ => {}
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), search)
        .await
        .expect("perangkat tidak ditemukan dalam 10 detik")
}

#[tokio::test]
#[ignore = "butuh jaringan yang meneruskan multicast"]
async fn devices_find_each_other_and_notice_when_one_leaves() {
    let (mac, pc) = (device("MacBook"), device("PC kantor"));
    let (mac_discovery, mut mac_events) = Discovery::start(&mac, 47001).unwrap();
    let (pc_discovery, mut pc_events) = Discovery::start(&pc, 47002).unwrap();

    let pc_addresses = wait_found(&mut mac_events, &pc).await;
    let mac_addresses = wait_found(&mut pc_events, &mac).await;
    assert!(pc_addresses.iter().all(|address| address.port() == 47002));
    assert!(mac_addresses.iter().all(|address| address.port() == 47001));

    drop(pc_discovery);
    let left = async {
        loop {
            if mac_events.recv().await.expect("discovery berhenti") == DiscoveryEvent::Lost(pc.fingerprint) {
                return;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), left)
        .await
        .expect("kepergian perangkat tidak terdeteksi");
    drop(mac_discovery);
}
