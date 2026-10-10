//! Identitas perangkat: sepasang kunci dan sertifikat self-signed yang dibuat
//! sekali lalu disimpan. Perangkat lain mengenali perangkat ini lewat sidik
//! jari (fingerprint) sertifikatnya.

use std::fmt;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::str::FromStr;

use anyhow::Context;
use rcgen::{CertificateParams, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

const CERTIFICATE_FILE: &str = "identity.der";
const KEY_FILE: &str = "identity.key";

/// SHA-256 dari sertifikat (DER) sebuah perangkat.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
    pub fn of_certificate(certificate_der: &[u8]) -> Self {
        Self(Sha256::digest(certificate_der).into())
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// ID pendek untuk ditampilkan dan dipakai di discovery: 16 digit hex pertama.
    pub fn device_id(&self) -> String {
        self.to_string()[..16].to_owned()
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({self})")
    }
}

impl FromStr for Fingerprint {
    type Err = anyhow::Error;

    fn from_str(hex: &str) -> Result<Self, Self::Err> {
        anyhow::ensure!(hex.len() == 64 && hex.is_ascii(), "fingerprint harus 64 digit hex");
        let mut bytes = [0u8; 32];
        let (pairs, _) = hex.as_bytes().as_chunks::<2>();
        for (byte, pair) in bytes.iter_mut().zip(pairs) {
            let pair = std::str::from_utf8(pair)?;
            *byte = u8::from_str_radix(pair, 16).context("fingerprint bukan hex")?;
        }
        Ok(Self(bytes))
    }
}

impl Serialize for Fingerprint {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Fingerprint {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// Kunci privat dan sertifikat perangkat ini.
pub struct Identity {
    certificate: CertificateDer<'static>,
    key_pkcs8: Vec<u8>,
    fingerprint: Fingerprint,
}

impl Identity {
    /// Membuat identitas baru tanpa menyimpannya.
    pub fn generate() -> anyhow::Result<Self> {
        let key = KeyPair::generate().context("gagal membuat kunci")?;
        // Nama di sertifikat tidak dipakai untuk verifikasi; yang dicek adalah fingerprint.
        let certificate = CertificateParams::new(vec!["sambung.local".to_owned()])
            .and_then(|params| params.self_signed(&key))
            .context("gagal membuat sertifikat")?;
        Ok(Self::from_parts(certificate.der().clone(), key.serialize_der()))
    }

    /// Memuat identitas dari `dir`, atau membuat dan menyimpannya kalau belum ada.
    pub fn load_or_create(dir: &Path) -> anyhow::Result<Self> {
        let certificate_path = dir.join(CERTIFICATE_FILE);
        let key_path = dir.join(KEY_FILE);

        if certificate_path.exists() && key_path.exists() {
            let certificate = fs::read(&certificate_path).context("gagal membaca sertifikat")?;
            let key = fs::read(&key_path).context("gagal membaca kunci")?;
            return Ok(Self::from_parts(CertificateDer::from(certificate), key));
        }

        let identity = Self::generate()?;
        fs::create_dir_all(dir).context("gagal membuat direktori data")?;
        write_private(&key_path, &identity.key_pkcs8).context("gagal menyimpan kunci")?;
        fs::write(&certificate_path, &identity.certificate).context("gagal menyimpan sertifikat")?;
        Ok(identity)
    }

    fn from_parts(certificate: CertificateDer<'static>, key_pkcs8: Vec<u8>) -> Self {
        let fingerprint = Fingerprint::of_certificate(&certificate);
        Self {
            certificate,
            key_pkcs8,
            fingerprint,
        }
    }

    pub fn fingerprint(&self) -> Fingerprint {
        self.fingerprint
    }

    pub(crate) fn certificate(&self) -> CertificateDer<'static> {
        self.certificate.clone()
    }

    pub(crate) fn private_key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key_pkcs8.clone()))
    }
}

/// Menulis berkas yang hanya bisa dibaca pemiliknya.
fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // Di Windows berkas mewarisi ACL direktori data milik pengguna.
    options.open(path)?.write_all(contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_stable_across_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let first = Identity::load_or_create(dir.path()).unwrap();
        let second = Identity::load_or_create(dir.path()).unwrap();
        assert_eq!(first.fingerprint(), second.fingerprint());
    }

    #[test]
    fn each_generated_identity_is_unique() {
        let a = Identity::generate().unwrap();
        let b = Identity::generate().unwrap();
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn fingerprint_round_trips_through_hex() {
        let fingerprint = Identity::generate().unwrap().fingerprint();
        let parsed: Fingerprint = fingerprint.to_string().parse().unwrap();
        assert_eq!(parsed, fingerprint);
        assert_eq!(fingerprint.device_id().len(), 16);
    }

    #[test]
    fn malformed_fingerprint_is_rejected() {
        assert!("abc".parse::<Fingerprint>().is_err());
        assert!("zz".repeat(32).parse::<Fingerprint>().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn private_key_is_readable_only_by_owner() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        Identity::load_or_create(dir.path()).unwrap();
        let mode = fs::metadata(dir.path().join(KEY_FILE)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
