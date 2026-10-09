//! Kode sekali pakai (OTP) yang ditampilkan di perangkat tujuan saat pairing.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use rand::rngs::OsRng;
use rand::Rng;

use crate::identity::Fingerprint;
use crate::protocol::PairFailure;

/// Jumlah tebakan yang diizinkan untuk satu kode.
pub const MAX_ATTEMPTS: u8 = 3;

#[derive(Debug, Clone, Copy)]
pub struct OtpPolicy {
    /// Masa berlaku satu kode.
    pub ttl: Duration,
    /// Pairing ditutup sementara kalau dalam `failure_window` terjadi
    /// `max_failures` percobaan gagal, dari perangkat mana pun.
    pub failure_window: Duration,
    pub max_failures: usize,
}

impl Default for OtpPolicy {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(120),
            failure_window: Duration::from_secs(60),
            max_failures: 5,
        }
    }
}

struct ActiveCode {
    code: String,
    expires_at: Instant,
    attempts_left: u8,
}

/// Menerbitkan dan memeriksa masa pakai OTP, satu kode aktif per perangkat peminta.
///
/// Batas kegagalan berlaku global, bukan per perangkat: peminta bisa berganti
/// sertifikat di tiap koneksi, jadi batas per perangkat mudah dihindari.
pub struct OtpManager {
    policy: OtpPolicy,
    active: HashMap<Fingerprint, ActiveCode>,
    failures: VecDeque<Instant>,
}

impl OtpManager {
    pub fn new(policy: OtpPolicy) -> Self {
        Self {
            policy,
            active: HashMap::new(),
            failures: VecDeque::new(),
        }
    }

    /// `Some` kalau pairing sedang ditutup karena terlalu banyak kegagalan.
    fn cooldown(&mut self, now: Instant) -> Option<PairFailure> {
        let window = self.policy.failure_window;
        while self.failures.front().is_some_and(|failed| now.duration_since(*failed) >= window) {
            self.failures.pop_front();
        }
        if self.failures.len() < self.policy.max_failures {
            return None;
        }
        let oldest = *self.failures.front()?;
        let remaining = (oldest + window).saturating_duration_since(now);
        Some(PairFailure::Cooldown {
            retry_after_secs: remaining.as_secs().max(1),
        })
    }

    /// Menerbitkan kode baru untuk `peer`, menggantikan kode lamanya.
    pub fn issue(&mut self, peer: Fingerprint, now: Instant) -> Result<String, PairFailure> {
        if let Some(failure) = self.cooldown(now) {
            return Err(failure);
        }
        let code = format!("{:06}", OsRng.gen_range(0..1_000_000u32));
        self.active.insert(
            peer,
            ActiveCode {
                code: code.clone(),
                expires_at: now + self.policy.ttl,
                attempts_left: MAX_ATTEMPTS,
            },
        );
        Ok(code)
    }

    /// Mengambil kode aktif untuk satu percobaan pairing. Pemanggil wajib
    /// menutup percobaan dengan [`succeed`](Self::succeed) atau
    /// [`fail`](Self::fail), termasuk kalau koneksi putus di tengah jalan.
    pub fn begin_attempt(&mut self, peer: Fingerprint, now: Instant) -> Result<String, PairFailure> {
        if let Some(failure) = self.cooldown(now) {
            return Err(failure);
        }
        let Some(active) = self.active.get_mut(&peer) else {
            return Err(PairFailure::Expired);
        };
        if now >= active.expires_at || active.attempts_left == 0 {
            self.active.remove(&peer);
            return Err(PairFailure::Expired);
        }
        active.attempts_left -= 1;
        Ok(active.code.clone())
    }

    /// Percobaan berhasil: kode tidak bisa dipakai lagi.
    pub fn succeed(&mut self, peer: Fingerprint) {
        self.active.remove(&peer);
    }

    /// Percobaan gagal. Kalau jatah kode habis, kode dicabut.
    pub fn fail(&mut self, peer: Fingerprint, now: Instant) -> PairFailure {
        self.failures.push_back(now);
        let attempts_left = self.active.get(&peer).map_or(0, |active| active.attempts_left);
        if attempts_left == 0 {
            self.active.remove(&peer);
        }
        PairFailure::WrongCode { attempts_left }
    }

    /// Membatalkan kode aktif, misalnya saat peminta memutus koneksi.
    /// Mengembalikan `true` kalau memang ada kode yang dibatalkan.
    pub fn cancel(&mut self, peer: Fingerprint) -> bool {
        self.active.remove(&peer).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Identity;

    fn peer() -> Fingerprint {
        Identity::generate().unwrap().fingerprint()
    }

    fn manager() -> OtpManager {
        OtpManager::new(OtpPolicy::default())
    }

    /// Menghabiskan satu percobaan gagal untuk `peer`.
    fn fail_once(otp: &mut OtpManager, peer: Fingerprint, now: Instant) -> PairFailure {
        otp.begin_attempt(peer, now).unwrap();
        otp.fail(peer, now)
    }

    #[test]
    fn code_is_six_digits() {
        let code = manager().issue(peer(), Instant::now()).unwrap();
        assert_eq!(code.len(), 6);
        assert!(code.bytes().all(|byte| byte.is_ascii_digit()));
    }

    #[test]
    fn code_cannot_be_used_after_success() {
        let (mut otp, peer, now) = (manager(), peer(), Instant::now());
        otp.issue(peer, now).unwrap();
        otp.begin_attempt(peer, now).unwrap();
        otp.succeed(peer);
        assert_eq!(otp.begin_attempt(peer, now), Err(PairFailure::Expired));
    }

    #[test]
    fn code_expires_after_ttl() {
        let (mut otp, peer, now) = (manager(), peer(), Instant::now());
        otp.issue(peer, now).unwrap();
        let late = now + Duration::from_secs(121);
        assert_eq!(otp.begin_attempt(peer, late), Err(PairFailure::Expired));
    }

    #[test]
    fn third_failure_revokes_the_code() {
        let (mut otp, peer, now) = (manager(), peer(), Instant::now());
        otp.issue(peer, now).unwrap();

        for attempts_left in [2, 1, 0] {
            assert_eq!(fail_once(&mut otp, peer, now), PairFailure::WrongCode { attempts_left });
        }

        assert_eq!(otp.begin_attempt(peer, now), Err(PairFailure::Expired));
    }

    #[test]
    fn too_many_failures_close_pairing_for_every_peer() {
        let (mut otp, now) = (manager(), Instant::now());

        // Lima kegagalan dari lima "perangkat" berbeda, seperti penyerang yang
        // berganti sertifikat di tiap koneksi.
        for _ in 0..5 {
            let attacker = peer();
            otp.issue(attacker, now).unwrap();
            fail_once(&mut otp, attacker, now);
        }

        let innocent = peer();
        assert_eq!(otp.issue(innocent, now), Err(PairFailure::Cooldown { retry_after_secs: 60 }));
        assert!(otp.issue(innocent, now + Duration::from_secs(60)).is_ok());
    }

    #[test]
    fn existing_code_cannot_be_guessed_during_cooldown() {
        let (mut otp, victim, now) = (manager(), peer(), Instant::now());
        otp.issue(victim, now).unwrap();
        for _ in 0..5 {
            let attacker = peer();
            otp.issue(attacker, now).unwrap();
            fail_once(&mut otp, attacker, now);
        }

        assert!(matches!(
            otp.begin_attempt(victim, now),
            Err(PairFailure::Cooldown { .. })
        ));
    }

    #[test]
    fn new_request_replaces_previous_code() {
        let (mut otp, peer, now) = (manager(), peer(), Instant::now());
        otp.issue(peer, now).unwrap();
        let second = otp.issue(peer, now).unwrap();
        assert_eq!(otp.begin_attempt(peer, now).unwrap(), second);
    }

    #[test]
    fn cancelled_code_cannot_be_used() {
        let (mut otp, peer, now) = (manager(), peer(), Instant::now());
        otp.issue(peer, now).unwrap();
        assert!(otp.cancel(peer));
        assert!(!otp.cancel(peer));
        assert_eq!(otp.begin_attempt(peer, now), Err(PairFailure::Expired));
    }
}
