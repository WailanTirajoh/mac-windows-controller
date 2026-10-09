//! Sinkronisasi clipboard teks selama sesi berjalan.
//!
//! Hanya perubahan yang terjadi **selama sesi** yang dikirim. Isi clipboard
//! sebelum sesi dimulai tidak dikirim, supaya sesuatu yang disalin sebelumnya
//! (misalnya kata sandi) tidak ikut berpindah perangkat.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::Duration;

/// Teks yang lebih besar dari ini tidak disinkronkan.
pub const MAX_TEXT_BYTES: usize = 1024 * 1024;

const POLL_INTERVAL: Duration = Duration::from_millis(500);

pub trait ClipboardBackend: Send + Sync {
    /// Isi clipboard kalau berupa teks.
    fn read_text(&self) -> Option<String>;
    fn write_text(&self, text: &str);
}

/// Clipboard OS.
pub struct SystemClipboard;

impl ClipboardBackend for SystemClipboard {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn read_text(&self) -> Option<String> {
        arboard::Clipboard::new().ok()?.get_text().ok()
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn write_text(&self, text: &str) {
        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            let _ = clipboard.set_text(text);
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn read_text(&self) -> Option<String> {
        None
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn write_text(&self, _text: &str) {}
}

/// Clipboard yang selalu kosong dan mengabaikan tulisan. Dipakai uji mandiri
/// dan test, yang tidak boleh menyentuh clipboard pengguna.
pub struct NullClipboard;

impl ClipboardBackend for NullClipboard {
    fn read_text(&self) -> Option<String> {
        None
    }

    fn write_text(&self, _text: &str) {}
}

/// Mendeteksi perubahan clipboard lokal dan menerapkan teks dari lawan tanpa
/// memantulkannya kembali.
struct Watcher {
    backend: Arc<dyn ClipboardBackend>,
    /// Isi terakhir yang sudah diketahui kedua sisi.
    last: Option<String>,
}

impl Watcher {
    fn new(backend: Arc<dyn ClipboardBackend>) -> Self {
        let last = backend.read_text();
        Self { backend, last }
    }

    /// Teks baru untuk dikirim, kalau clipboard lokal berubah sejak pemeriksaan terakhir.
    fn poll(&mut self) -> Option<String> {
        let current = self.backend.read_text()?;
        if self.last.as_deref() == Some(current.as_str()) {
            return None;
        }
        self.last = Some(current.clone());
        (!current.is_empty() && current.len() <= MAX_TEXT_BYTES).then_some(current)
    }

    /// Menulis teks dari lawan ke clipboard lokal.
    fn apply(&mut self, text: String) {
        if text.len() > MAX_TEXT_BYTES {
            return;
        }
        self.backend.write_text(&text);
        self.last = Some(text);
    }
}

/// Sinkronisasi yang sedang berjalan. Berhenti saat di-drop.
pub struct ClipboardSync {
    incoming: mpsc::Sender<String>,
    enabled: Arc<AtomicBool>,
}

impl ClipboardSync {
    /// Mulai memantau clipboard. `send` dipanggil dengan teks yang harus
    /// dikirim ke lawan.
    pub fn start(backend: Arc<dyn ClipboardBackend>, send: impl Fn(String) + Send + 'static) -> Self {
        let (incoming, received) = mpsc::channel::<String>();
        let enabled = Arc::new(AtomicBool::new(true));
        // Isi awal dicatat di sini, bukan di thread, supaya yang disalin tepat
        // setelah sesi dimulai tidak terhitung sebagai isi lama.
        let mut watcher = Watcher::new(backend);

        let active = Arc::clone(&enabled);
        std::thread::spawn(move || {
            loop {
                match received.recv_timeout(POLL_INTERVAL) {
                    Ok(text) => {
                        if active.load(Ordering::Relaxed) {
                            watcher.apply(text);
                        }
                    }
                    // Tetap dipantau saat nonaktif, supaya isi lama tidak
                    // terkirim begitu sinkronisasi dinyalakan lagi.
                    Err(RecvTimeoutError::Timeout) => {
                        if let Some(text) = watcher.poll().filter(|_| active.load(Ordering::Relaxed)) {
                            send(text);
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        });

        Self { incoming, enabled }
    }

    /// Menerapkan teks yang diterima dari lawan.
    pub fn receive(&self, text: String) {
        let _ = self.incoming.send(text);
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct FakeClipboard {
        text: Mutex<Option<String>>,
        writes: Mutex<Vec<String>>,
    }

    impl FakeClipboard {
        fn copy(&self, text: &str) {
            *self.text.lock().unwrap() = Some(text.to_owned());
        }
    }

    impl ClipboardBackend for FakeClipboard {
        fn read_text(&self) -> Option<String> {
            self.text.lock().unwrap().clone()
        }

        fn write_text(&self, text: &str) {
            self.copy(text);
            self.writes.lock().unwrap().push(text.to_owned());
        }
    }

    fn watcher_with(initial: Option<&str>) -> (Arc<FakeClipboard>, Watcher) {
        let clipboard = Arc::new(FakeClipboard::default());
        if let Some(initial) = initial {
            clipboard.copy(initial);
        }
        let watcher = Watcher::new(Arc::clone(&clipboard) as Arc<dyn ClipboardBackend>);
        (clipboard, watcher)
    }

    #[test]
    fn content_from_before_the_session_is_never_sent() {
        let (_, mut watcher) = watcher_with(Some("kata sandi lama"));
        assert_eq!(watcher.poll(), None);
    }

    #[test]
    fn new_copy_is_sent_once() {
        let (clipboard, mut watcher) = watcher_with(Some("lama"));
        clipboard.copy("baru");
        assert_eq!(watcher.poll().as_deref(), Some("baru"));
        assert_eq!(watcher.poll(), None);
    }

    #[test]
    fn text_from_the_peer_is_written_but_not_sent_back() {
        let (clipboard, mut watcher) = watcher_with(None);
        watcher.apply("dari lawan".to_owned());
        assert_eq!(clipboard.read_text().as_deref(), Some("dari lawan"));
        assert_eq!(watcher.poll(), None, "teks dari lawan tidak boleh dipantulkan");
    }

    #[test]
    fn oversized_text_is_neither_sent_nor_applied() {
        let (clipboard, mut watcher) = watcher_with(None);
        let huge = "x".repeat(MAX_TEXT_BYTES + 1);

        clipboard.copy(&huge);
        assert_eq!(watcher.poll(), None);

        watcher.apply(huge);
        assert!(clipboard.writes.lock().unwrap().is_empty());
    }

    #[test]
    fn non_text_clipboard_is_ignored() {
        let (clipboard, mut watcher) = watcher_with(Some("teks"));
        // Pengguna menyalin gambar: tidak ada teks yang bisa dibaca.
        *clipboard.text.lock().unwrap() = None;
        assert_eq!(watcher.poll(), None);
    }
}
