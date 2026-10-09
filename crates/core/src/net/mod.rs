//! Transport QUIC antar-perangkat.

mod tls;

use std::net::SocketAddr;

use anyhow::Context;
use rustls::pki_types::CertificateDer;

use crate::identity::{Fingerprint, Identity};

/// Nama server di handshake TLS. Tidak dipakai untuk verifikasi.
const SERVER_NAME: &str = "sambung.local";

/// Label TLS exporter yang mengikat pairing ke sesi TLS ini.
const PAIRING_EXPORTER_LABEL: &[u8] = b"sambung pairing v1";

/// Membuka endpoint QUIC yang bisa menerima dan memulai koneksi.
pub fn bind(identity: &Identity, address: SocketAddr) -> anyhow::Result<quinn::Endpoint> {
    let mut endpoint = quinn::Endpoint::server(tls::server_config(identity)?, address)
        .with_context(|| format!("gagal membuka port {address}"))?;
    endpoint.set_default_client_config(tls::client_config(identity)?);
    Ok(endpoint)
}

/// Koneksi QUIC yang handshake-nya sudah selesai, beserta identitas lawannya.
#[derive(Clone)]
pub struct Link {
    pub connection: quinn::Connection,
    pub peer_fingerprint: Fingerprint,
}

impl Link {
    pub async fn connect(endpoint: &quinn::Endpoint, address: SocketAddr) -> anyhow::Result<Self> {
        let connection = endpoint
            .connect(address, SERVER_NAME)
            .context("alamat tujuan tidak valid")?
            .await
            .with_context(|| format!("tidak bisa terhubung ke {address}"))?;
        Self::new(connection)
    }

    pub fn new(connection: quinn::Connection) -> anyhow::Result<Self> {
        let certificates = connection
            .peer_identity()
            .and_then(|identity| identity.downcast::<Vec<CertificateDer<'static>>>().ok())
            .context("lawan tidak menyajikan sertifikat")?;
        let certificate = certificates.first().context("lawan tidak menyajikan sertifikat")?;
        Ok(Self {
            peer_fingerprint: Fingerprint::of_certificate(certificate),
            connection,
        })
    }

    /// Nilai rahasia yang sama di kedua ujung sesi TLS ini, dan berbeda untuk
    /// sesi lain. Pihak ketiga yang menengahi dua sesi mendapat dua nilai berbeda.
    pub fn pairing_binding(&self) -> anyhow::Result<[u8; 32]> {
        let mut binding = [0u8; 32];
        self.connection
            .export_keying_material(&mut binding, PAIRING_EXPORTER_LABEL, b"")
            .map_err(|_| anyhow::anyhow!("TLS exporter tidak tersedia"))?;
        Ok(binding)
    }
}
