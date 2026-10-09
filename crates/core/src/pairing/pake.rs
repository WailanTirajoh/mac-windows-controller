//! Pertukaran kunci berbasis OTP (SPAKE2) dan konfirmasi kuncinya.
//!
//! SPAKE2 membuat pihak yang tidak tahu OTP hanya bisa menguji satu tebakan
//! per percobaan, tanpa bisa menebak offline. Konfirmasi kunci mengikat hasil
//! pertukaran ke sesi TLS dan ke sertifikat kedua perangkat, sehingga pihak
//! ketiga yang meneruskan pesan di antara dua sesi TLS berbeda tetap gagal.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use spake2::{Ed25519Group, Identity as PakeIdentity, Password, Spake2};

use crate::identity::Fingerprint;

type HmacSha256 = Hmac<Sha256>;

const LABEL_INITIATOR: &[u8] = b"sambung pairing confirm A";
const LABEL_RESPONDER: &[u8] = b"sambung pairing confirm B";

/// Hal-hal yang harus sama di kedua sisi agar pairing dianggap sah.
#[derive(Clone, Copy)]
pub struct Binding {
    /// Nilai TLS exporter sesi ini, lihat `Link::pairing_binding`.
    pub tls_exporter: [u8; 32],
    /// Sertifikat pengendali (yang mengetik OTP).
    pub initiator: Fingerprint,
    /// Sertifikat tujuan (yang menampilkan OTP).
    pub responder: Fingerprint,
}

pub struct PakeStart {
    state: Spake2<Ed25519Group>,
    is_initiator: bool,
    /// Pesan SPAKE2 yang harus dikirim ke lawan.
    pub message: Vec<u8>,
}

/// Bukti konfirmasi dari kedua sisi, diturunkan dari kunci hasil SPAKE2.
pub struct Confirmations {
    pub initiator: [u8; 32],
    pub responder: [u8; 32],
}

fn identities(binding: &Binding) -> (PakeIdentity, PakeIdentity) {
    (
        PakeIdentity::new(binding.initiator.as_bytes()),
        PakeIdentity::new(binding.responder.as_bytes()),
    )
}

pub fn start_initiator(otp: &str, binding: &Binding) -> PakeStart {
    let (initiator, responder) = identities(binding);
    let (state, message) = Spake2::<Ed25519Group>::start_a(&Password::new(otp), &initiator, &responder);
    PakeStart {
        state,
        is_initiator: true,
        message,
    }
}

pub fn start_responder(otp: &str, binding: &Binding) -> PakeStart {
    let (initiator, responder) = identities(binding);
    let (state, message) = Spake2::<Ed25519Group>::start_b(&Password::new(otp), &initiator, &responder);
    PakeStart {
        state,
        is_initiator: false,
        message,
    }
}

impl PakeStart {
    /// Menyelesaikan pertukaran dengan pesan SPAKE2 dari lawan. Gagal hanya
    /// kalau pesannya cacat; OTP yang berbeda menghasilkan konfirmasi yang
    /// tidak cocok, bukan error di sini.
    pub fn finish(self, peer_message: &[u8], binding: &Binding) -> Option<Confirmations> {
        let Self {
            state,
            is_initiator,
            message,
        } = self;
        let (initiator_message, responder_message) = if is_initiator {
            (message.as_slice(), peer_message)
        } else {
            (peer_message, message.as_slice())
        };
        let key = state.finish(peer_message).ok()?;
        let confirm = |label: &[u8]| -> [u8; 32] {
            let mut mac = HmacSha256::new_from_slice(&key).expect("HMAC menerima kunci sepanjang apa pun");
            for part in [
                label,
                &binding.tls_exporter,
                binding.initiator.as_bytes(),
                binding.responder.as_bytes(),
                initiator_message,
                responder_message,
            ] {
                // Panjang tiap bagian ikut di-MAC supaya batas antarbagian tidak ambigu.
                mac.update(&(part.len() as u64).to_be_bytes());
                mac.update(part);
            }
            mac.finalize().into_bytes().into()
        };
        Some(Confirmations {
            initiator: confirm(LABEL_INITIATOR),
            responder: confirm(LABEL_RESPONDER),
        })
    }
}

/// Membandingkan dua konfirmasi dalam waktu konstan.
pub fn confirmations_match(expected: &[u8; 32], received: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    expected.ct_eq(received).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Identity;

    fn binding() -> Binding {
        Binding {
            tls_exporter: [7; 32],
            initiator: Identity::generate().unwrap().fingerprint(),
            responder: Identity::generate().unwrap().fingerprint(),
        }
    }

    /// Menjalankan pertukaran lengkap dan mengembalikan konfirmasi tiap sisi.
    fn exchange(otp_a: &str, binding_a: &Binding, otp_b: &str, binding_b: &Binding) -> (Confirmations, Confirmations) {
        let a = start_initiator(otp_a, binding_a);
        let b = start_responder(otp_b, binding_b);
        let (message_a, message_b) = (a.message.clone(), b.message.clone());
        (
            a.finish(&message_b, binding_a).unwrap(),
            b.finish(&message_a, binding_b).unwrap(),
        )
    }

    #[test]
    fn same_otp_and_binding_yield_matching_confirmations() {
        let binding = binding();
        let (a, b) = exchange("482913", &binding, "482913", &binding);
        assert!(confirmations_match(&a.initiator, &b.initiator));
        assert!(confirmations_match(&a.responder, &b.responder));
        assert_ne!(a.initiator, a.responder, "konfirmasi tiap arah harus berbeda");
    }

    #[test]
    fn different_otp_yields_mismatched_confirmations() {
        let binding = binding();
        let (a, b) = exchange("482913", &binding, "482914", &binding);
        assert!(!confirmations_match(&a.initiator, &b.initiator));
        assert!(!confirmations_match(&a.responder, &b.responder));
    }

    #[test]
    fn different_tls_session_yields_mismatched_confirmations() {
        let binding_a = binding();
        let binding_b = Binding {
            tls_exporter: [8; 32],
            ..binding_a
        };
        let (a, b) = exchange("482913", &binding_a, "482913", &binding_b);
        assert!(!confirmations_match(&a.initiator, &b.initiator));
    }

    #[test]
    fn malformed_peer_message_is_rejected() {
        let binding = binding();
        let a = start_initiator("482913", &binding);
        assert!(a.finish(b"x", &binding).is_none());
    }

    #[test]
    fn truncated_confirmation_does_not_match() {
        let binding = binding();
        let (a, _) = exchange("482913", &binding, "482913", &binding);
        assert!(!confirmations_match(&a.initiator, &a.initiator[..31]));
    }
}
