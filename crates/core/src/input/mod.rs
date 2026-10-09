//! Injeksi input di perangkat tujuan.

pub mod keymap;
pub mod recording;

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod enigo_injector;

use crate::capture::DisplayInfo;
use crate::protocol::InputEvent;

/// Menyuntikkan event mouse dan keyboard ke OS.
pub trait InputInjector: Send {
    /// `display` adalah monitor yang sedang ditampilkan ke pengendali; posisi
    /// 0..1 di dalam event dipetakan ke monitor itu.
    fn inject(&mut self, event: &InputEvent, display: &DisplayInfo) -> anyhow::Result<()>;

    /// Melepas semua tombol yang masih tertekan, supaya tidak ada tombol
    /// "nyangkut" saat sesi berakhir di tengah penekanan.
    fn release_all(&mut self);
}

/// Pembuat injector, satu per sesi.
pub trait InputBackend: Send + Sync {
    fn open(&self) -> anyhow::Result<Box<dyn InputInjector>>;
}

/// Injeksi input sungguhan lewat API OS.
pub struct SystemInput;

impl InputBackend for SystemInput {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn open(&self) -> anyhow::Result<Box<dyn InputInjector>> {
        Ok(Box::new(enigo_injector::EnigoInjector::new()?))
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn open(&self) -> anyhow::Result<Box<dyn InputInjector>> {
        anyhow::bail!("injeksi input belum didukung di OS ini")
    }
}

/// Meminta OS mendaftarkan aplikasi ini sebagai calon pengendali input.
///
/// Di macOS ini memunculkan dialog Accessibility (sekali) dan menambahkan
/// aplikasi ke daftar di System Settings; izinnya tetap diaktifkan pengguna.
/// Di OS lain tidak melakukan apa-apa.
pub fn request_system_access() {
    #[cfg(target_os = "macos")]
    enigo_injector::prompt_for_access();
}

/// Posisi `value` dalam rentang 0..=65535 yang dipakai `SendInput` Windows
/// untuk gerakan mouse absolut, relatif terhadap rentang piksel yang dimulai
/// di `origin` sepanjang `length` piksel.
pub fn to_absolute_units(value: i32, origin: i32, length: i32) -> i32 {
    let last = i64::from(length.max(2)) - 1;
    let offset = (i64::from(value) - i64::from(origin)).clamp(0, last);
    ((offset * 65535 + last / 2) / last) as i32
}

/// Posisi piksel/point di koordinat global OS untuk posisi 0..1 di `display`.
/// Hasilnya selalu di dalam monitor, juga untuk masukan di luar 0..1.
pub fn to_global(x: f64, y: f64, display: &DisplayInfo) -> (i32, i32) {
    let along = |fraction: f64, origin: f64, length: f64| {
        let offset = (fraction.clamp(0.0, 1.0) * length).floor().min(length - 1.0).max(0.0);
        (origin + offset) as i32
    };
    (
        along(x, display.x, display.width),
        along(y, display.y, display.height),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(x: f64, y: f64, width: f64, height: f64) -> DisplayInfo {
        DisplayInfo {
            id: 1,
            x,
            y,
            width,
            height,
            pixel_width: width as u32,
            pixel_height: height as u32,
            is_primary: true,
        }
    }

    #[test]
    fn corners_map_to_first_and_last_pixel() {
        let main = display(0.0, 0.0, 1920.0, 1080.0);
        assert_eq!(to_global(0.0, 0.0, &main), (0, 0));
        assert_eq!(to_global(1.0, 1.0, &main), (1919, 1079));
        assert_eq!(to_global(0.5, 0.5, &main), (960, 540));
    }

    #[test]
    fn secondary_display_offsets_are_applied() {
        // Monitor kedua di kiri atas monitor utama punya koordinat negatif.
        let left = display(-1280.0, -200.0, 1280.0, 720.0);
        assert_eq!(to_global(0.0, 0.0, &left), (-1280, -200));
        assert_eq!(to_global(1.0, 1.0, &left), (-1, 519));
    }

    #[test]
    fn absolute_units_span_the_whole_virtual_desktop() {
        // Desktop virtual 3200 piksel yang dimulai di -1280 (monitor kedua di kiri).
        assert_eq!(to_absolute_units(-1280, -1280, 3200), 0);
        assert_eq!(to_absolute_units(1919, -1280, 3200), 65535);
        assert_eq!(to_absolute_units(320, -1280, 3200), 32778);
        // Di luar desktop dijepit ke tepinya.
        assert_eq!(to_absolute_units(-5000, -1280, 3200), 0);
        assert_eq!(to_absolute_units(9000, -1280, 3200), 65535);
    }

    #[test]
    fn out_of_range_positions_stay_inside_the_display() {
        let main = display(0.0, 0.0, 1920.0, 1080.0);
        assert_eq!(to_global(-0.5, 2.0, &main), (0, 1079));
        assert_eq!(to_global(f64::NAN, 0.0, &main).1, 0);
    }
}
