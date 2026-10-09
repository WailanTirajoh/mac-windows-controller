//! Izin OS yang dibutuhkan supaya perangkat ini bisa dikendalikan.
//!
//! Hanya macOS yang mewajibkan izin eksplisit: Screen Recording untuk
//! menampilkan layar, Accessibility untuk menyuntikkan mouse dan keyboard.
//! Aplikasi hanya memeriksa dan membuka halaman pengaturannya; pengguna yang
//! mengaktifkannya.

use serde::{Deserialize, Serialize};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Permissions {
    screen_recording: bool,
    accessibility: bool,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionKind {
    ScreenRecording,
    Accessibility,
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{PermissionKind, Permissions};

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        /// Mengembalikan `Boolean` (unsigned char).
        fn AXIsProcessTrusted() -> u8;
    }

    pub fn check() -> Permissions {
        // SAFETY: ketiga fungsi tanpa argumen dan tanpa prasyarat.
        unsafe {
            Permissions {
                screen_recording: CGPreflightScreenCaptureAccess(),
                accessibility: AXIsProcessTrusted() != 0,
            }
        }
    }

    pub fn request(kind: PermissionKind) -> std::io::Result<()> {
        let pane = match kind {
            PermissionKind::ScreenRecording => {
                // Mendaftarkan aplikasi di daftar Screen Recording; dialog sistem
                // hanya muncul pada pemanggilan pertama.
                // SAFETY: fungsi tanpa argumen dan tanpa prasyarat.
                unsafe { CGRequestScreenCaptureAccess() };
                "Privacy_ScreenCapture"
            }
            PermissionKind::Accessibility => {
                // Mendaftarkan aplikasi di daftar Accessibility.
                sambung_core::input::request_system_access();
                "Privacy_Accessibility"
            }
        };
        std::process::Command::new("open")
            .arg(format!("x-apple.systempreferences:com.apple.preference.security?{pane}"))
            .status()
            .map(|_| ())
    }
}

#[tauri::command]
pub fn get_permissions() -> Permissions {
    #[cfg(target_os = "macos")]
    {
        macos::check()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Permissions {
            screen_recording: true,
            accessibility: true,
        }
    }
}

/// Membuka halaman pengaturan untuk izin yang diminta.
#[tauri::command]
pub fn request_permission(kind: PermissionKind) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        macos::request(kind).map_err(|error| format!("Gagal membuka System Settings: {error}"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = kind;
        Ok(())
    }
}
