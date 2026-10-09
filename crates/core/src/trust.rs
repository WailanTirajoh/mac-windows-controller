//! Daftar perangkat yang sudah di-pair, disimpan sebagai JSON.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::identity::Fingerprint;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Os {
    Macos,
    Windows,
    Linux,
    Unknown,
}

impl Os {
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Macos
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Unknown
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedDevice {
    pub fingerprint: Fingerprint,
    pub name: String,
    pub os: Os,
    /// Apakah perangkat itu boleh mengendalikan perangkat ini.
    pub allow_incoming: bool,
    /// Waktu pairing, detik sejak epoch Unix.
    pub paired_at: u64,
}

pub struct TrustStore {
    path: PathBuf,
    devices: Mutex<Vec<TrustedDevice>>,
}

impl TrustStore {
    /// Memuat daftar dari `path`. Berkas yang belum ada berarti daftar kosong.
    pub fn load(path: PathBuf) -> anyhow::Result<Self> {
        let devices = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("daftar perangkat tersimpan rusak")?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error).context("gagal membaca daftar perangkat tersimpan"),
        };
        Ok(Self {
            path,
            devices: Mutex::new(devices),
        })
    }

    pub fn get(&self, fingerprint: &Fingerprint) -> Option<TrustedDevice> {
        self.devices
            .lock()
            .unwrap()
            .iter()
            .find(|device| device.fingerprint == *fingerprint)
            .cloned()
    }

    pub fn list(&self) -> Vec<TrustedDevice> {
        self.devices.lock().unwrap().clone()
    }

    /// Menambah perangkat, atau mengganti entri lama dengan fingerprint yang sama.
    pub fn upsert(&self, device: TrustedDevice) -> anyhow::Result<()> {
        let mut devices = self.devices.lock().unwrap();
        devices.retain(|existing| existing.fingerprint != device.fingerprint);
        devices.push(device);
        self.save(&devices)
    }

    /// Menghapus perangkat. Mengembalikan `true` kalau memang ada.
    pub fn remove(&self, fingerprint: &Fingerprint) -> anyhow::Result<bool> {
        let mut devices = self.devices.lock().unwrap();
        let before = devices.len();
        devices.retain(|existing| existing.fingerprint != *fingerprint);
        let removed = devices.len() != before;
        if removed {
            self.save(&devices)?;
        }
        Ok(removed)
    }

    /// Menulis ke berkas sementara lalu rename, supaya berkas lama tidak rusak
    /// kalau proses berhenti di tengah penulisan.
    fn save(&self, devices: &[TrustedDevice]) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).context("gagal membuat direktori data")?;
        }
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, serde_json::to_vec_pretty(devices)?).context("gagal menulis daftar perangkat")?;
        fs::rename(&temporary, &self.path).context("gagal menyimpan daftar perangkat")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Identity;

    fn device(name: &str) -> TrustedDevice {
        TrustedDevice {
            fingerprint: Identity::generate().unwrap().fingerprint(),
            name: name.to_owned(),
            os: Os::Windows,
            allow_incoming: true,
            paired_at: 1_700_000_000,
        }
    }

    #[test]
    fn devices_survive_a_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trusted.json");
        let pc = device("PC");

        TrustStore::load(path.clone()).unwrap().upsert(pc.clone()).unwrap();

        let reloaded = TrustStore::load(path).unwrap();
        assert_eq!(reloaded.list(), vec![pc.clone()]);
        assert_eq!(reloaded.get(&pc.fingerprint), Some(pc));
    }

    #[test]
    fn upsert_replaces_entry_with_same_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let store = TrustStore::load(dir.path().join("trusted.json")).unwrap();
        let mut pc = device("PC");
        store.upsert(pc.clone()).unwrap();

        pc.name = "PC kantor".to_owned();
        pc.allow_incoming = false;
        store.upsert(pc.clone()).unwrap();

        assert_eq!(store.list(), vec![pc]);
    }

    #[test]
    fn removed_device_is_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let store = TrustStore::load(dir.path().join("trusted.json")).unwrap();
        let pc = device("PC");
        store.upsert(pc.clone()).unwrap();

        assert!(store.remove(&pc.fingerprint).unwrap());
        assert!(!store.remove(&pc.fingerprint).unwrap());
        assert_eq!(store.get(&pc.fingerprint), None);
    }

    #[test]
    fn corrupt_file_is_an_error_not_an_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trusted.json");
        fs::write(&path, b"{ bukan json").unwrap();
        assert!(TrustStore::load(path).is_err());
    }
}
