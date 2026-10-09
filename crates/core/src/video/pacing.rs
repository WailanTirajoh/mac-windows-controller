//! Slot "frame terbaru" antara capture dan encoder.

use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// Slot berisi paling banyak satu nilai. Nilai baru menimpa nilai lama, jadi
/// kalau konsumen tertinggal, frame lama dibuang dan tidak pernah diantrekan.
pub struct LatestSlot<T> {
    value: Mutex<Option<T>>,
    ready: Condvar,
}

impl<T> Default for LatestSlot<T> {
    fn default() -> Self {
        Self {
            value: Mutex::new(None),
            ready: Condvar::new(),
        }
    }
}

impl<T> LatestSlot<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Menaruh nilai baru. Mengembalikan `true` kalau ada nilai lama yang
    /// tertimpa (frame dibuang).
    pub fn put(&self, value: T) -> bool {
        let mut slot = self.value.lock().unwrap();
        let replaced = slot.replace(value).is_some();
        self.ready.notify_one();
        replaced
    }

    /// Mengambil nilai, menunggu paling lama `timeout` kalau slot kosong.
    pub fn take(&self, timeout: Duration) -> Option<T> {
        let slot = self.value.lock().unwrap();
        let (mut slot, _) = self
            .ready
            .wait_timeout_while(slot, timeout, |value| value.is_none())
            .unwrap();
        slot.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn newer_value_replaces_older_one() {
        let slot = LatestSlot::new();
        assert!(!slot.put(1));
        assert!(slot.put(2));
        assert_eq!(slot.take(Duration::ZERO), Some(2));
        assert_eq!(slot.take(Duration::ZERO), None);
    }

    #[test]
    fn take_wakes_up_when_value_arrives() {
        let slot = Arc::new(LatestSlot::new());
        let producer = Arc::clone(&slot);
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            producer.put(7);
        });
        assert_eq!(slot.take(Duration::from_secs(5)), Some(7));
        handle.join().unwrap();
    }
}
