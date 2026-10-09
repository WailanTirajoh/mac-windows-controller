//! Discovery perangkat di jaringan lokal lewat mDNS/DNS-SD.
//!
//! Isi record discovery tidak diautentikasi: siapa pun di jaringan bisa
//! mengiklankan fingerprint mana pun. Record hanya dipakai sebagai petunjuk
//! alamat; identitas tetap dibuktikan saat koneksi (lihat `node`).

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};

use anyhow::Context;
use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent, ServiceInfo};
use tokio::sync::mpsc;

use crate::identity::Fingerprint;
use crate::node::DeviceInfo;
use crate::protocol::PROTOCOL_VERSION;
use crate::trust::Os;

const SERVICE_TYPE: &str = "_sambung._udp.local.";

const TXT_FINGERPRINT: &str = "fp";
const TXT_NAME: &str = "name";
const TXT_OS: &str = "os";
const TXT_VERSION: &str = "v";

/// Satu properti TXT (kunci + nilai) paling panjang 255 byte.
const MAX_NAME_BYTES: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredDevice {
    pub info: DeviceInfo,
    /// Alamat IPv4 yang diiklankan, sudah digabung dengan port-nya.
    pub addresses: Vec<SocketAddr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryEvent {
    Found(DiscoveredDevice),
    Lost(Fingerprint),
}

/// Iklan dan pencarian yang sedang berjalan. Berhenti saat di-drop.
pub struct Discovery {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Discovery {
    /// Mengiklankan perangkat ini di `port` dan mulai mencari perangkat lain.
    /// Harus dipanggil dari dalam runtime tokio.
    pub fn start(me: &DeviceInfo, port: u16) -> anyhow::Result<(Self, mpsc::UnboundedReceiver<DiscoveryEvent>)> {
        let daemon = ServiceDaemon::new().context("gagal memulai mDNS")?;

        let device_id = me.fingerprint.device_id();
        let properties = [
            (TXT_FINGERPRINT, me.fingerprint.to_string()),
            (TXT_NAME, truncate(&me.name, MAX_NAME_BYTES).to_owned()),
            (TXT_OS, me.os.as_str().to_owned()),
            (TXT_VERSION, PROTOCOL_VERSION.to_string()),
        ];
        // Alamat dikosongkan; `enable_addr_auto` mengisinya dari semua antarmuka
        // dan memperbaruinya saat jaringan berubah.
        let service = ServiceInfo::new(
            SERVICE_TYPE,
            &device_id,
            &format!("{device_id}.local."),
            "",
            port,
            &properties[..],
        )
        .context("record discovery tidak valid")?
        .enable_addr_auto();
        let fullname = service.get_fullname().to_owned();
        daemon.register(service).context("gagal mengiklankan perangkat")?;

        let found = daemon.browse(SERVICE_TYPE).context("gagal mencari perangkat")?;
        let (events, receiver) = mpsc::unbounded_channel();
        let own = me.fingerprint;
        tokio::spawn(async move {
            // Event `ServiceRemoved` hanya membawa nama lengkap service.
            let mut known: HashMap<String, Fingerprint> = HashMap::new();
            while let Ok(event) = found.recv_async().await {
                let event = match event {
                    ServiceEvent::ServiceResolved(service) => {
                        let Some(device) = parse(&service).filter(|device| device.info.fingerprint != own) else {
                            continue;
                        };
                        known.insert(service.get_fullname().to_owned(), device.info.fingerprint);
                        DiscoveryEvent::Found(device)
                    }
                    ServiceEvent::ServiceRemoved(_, fullname) => match known.remove(&fullname) {
                        Some(fingerprint) => DiscoveryEvent::Lost(fingerprint),
                        None => continue,
                    },
                    _ => continue,
                };
                if events.send(event).is_err() {
                    break;
                }
            }
        });

        Ok((Self { daemon, fullname }, receiver))
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        // Mengirim "goodbye" supaya perangkat lain langsung menghapus kita dari daftarnya.
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

fn parse(service: &ResolvedService) -> Option<DiscoveredDevice> {
    let fingerprint = service.get_property_val_str(TXT_FINGERPRINT)?.parse().ok()?;
    let name = service.get_property_val_str(TXT_NAME)?.to_owned();
    let os = Os::from_name(service.get_property_val_str(TXT_OS).unwrap_or_default());

    let port = service.get_port();
    let mut addresses: Vec<SocketAddr> = service
        .get_addresses_v4()
        .into_iter()
        .map(|ip| SocketAddr::new(IpAddr::V4(ip), port))
        .collect();
    addresses.sort();
    if addresses.is_empty() {
        return None;
    }

    Some(DiscoveredDevice {
        info: DeviceInfo { fingerprint, name, os },
        addresses,
    })
}

/// Memotong `text` ke paling banyak `max_bytes` byte tanpa membelah karakter.
fn truncate(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_keeps_short_text() {
        assert_eq!(truncate("MacBook", 200), "MacBook");
    }

    #[test]
    fn truncate_never_splits_a_character() {
        // "é" memakai dua byte; batas 3 jatuh di tengah "é" kedua.
        assert_eq!(truncate("ééé", 3), "é");
    }
}
