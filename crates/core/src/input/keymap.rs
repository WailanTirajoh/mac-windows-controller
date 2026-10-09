//! Pemetaan `KeyboardEvent.code` (posisi fisik tombol) ke kode tombol native.
//!
//! macOS memakai virtual keycode (`kVK_*`), Windows memakai scancode set 1.
//! Keduanya menyatakan posisi fisik, jadi huruf yang muncul mengikuti layout
//! keyboard perangkat tujuan, sama seperti keyboard yang dicolokkan langsung.

/// Penanda tombol yang tidak ada padanannya di sebuah OS.
const NONE: u16 = u16::MAX;

/// Scancode Windows dengan awalan `E0` (tombol "extended").
const fn ext(scancode: u16) -> u16 {
    0xE000 | scancode
}

/// `(code, keycode macOS, scancode Windows)`.
#[rustfmt::skip]
const KEYS: &[(&str, u16, u16)] = &[
    // Huruf
    ("KeyA", 0x00, 0x1E), ("KeyB", 0x0B, 0x30), ("KeyC", 0x08, 0x2E), ("KeyD", 0x02, 0x20),
    ("KeyE", 0x0E, 0x12), ("KeyF", 0x03, 0x21), ("KeyG", 0x05, 0x22), ("KeyH", 0x04, 0x23),
    ("KeyI", 0x22, 0x17), ("KeyJ", 0x26, 0x24), ("KeyK", 0x28, 0x25), ("KeyL", 0x25, 0x26),
    ("KeyM", 0x2E, 0x32), ("KeyN", 0x2D, 0x31), ("KeyO", 0x1F, 0x18), ("KeyP", 0x23, 0x19),
    ("KeyQ", 0x0C, 0x10), ("KeyR", 0x0F, 0x13), ("KeyS", 0x01, 0x1F), ("KeyT", 0x11, 0x14),
    ("KeyU", 0x20, 0x16), ("KeyV", 0x09, 0x2F), ("KeyW", 0x0D, 0x11), ("KeyX", 0x07, 0x2D),
    ("KeyY", 0x10, 0x15), ("KeyZ", 0x06, 0x2C),
    // Angka baris atas
    ("Digit1", 0x12, 0x02), ("Digit2", 0x13, 0x03), ("Digit3", 0x14, 0x04), ("Digit4", 0x15, 0x05),
    ("Digit5", 0x17, 0x06), ("Digit6", 0x16, 0x07), ("Digit7", 0x1A, 0x08), ("Digit8", 0x1C, 0x09),
    ("Digit9", 0x19, 0x0A), ("Digit0", 0x1D, 0x0B),
    // Tanda baca
    ("Minus", 0x1B, 0x0C), ("Equal", 0x18, 0x0D), ("BracketLeft", 0x21, 0x1A), ("BracketRight", 0x1E, 0x1B),
    ("Backslash", 0x2A, 0x2B), ("Semicolon", 0x29, 0x27), ("Quote", 0x27, 0x28), ("Backquote", 0x32, 0x29),
    ("Comma", 0x2B, 0x33), ("Period", 0x2F, 0x34), ("Slash", 0x2C, 0x35), ("IntlBackslash", 0x0A, 0x56),
    // Spasi dan penyuntingan
    ("Space", 0x31, 0x39), ("Enter", 0x24, 0x1C), ("Tab", 0x30, 0x0F), ("Backspace", 0x33, 0x0E),
    ("Escape", 0x35, 0x01), ("Delete", 0x75, ext(0x53)), ("Insert", 0x72, ext(0x52)),
    // Navigasi
    ("ArrowLeft", 0x7B, ext(0x4B)), ("ArrowRight", 0x7C, ext(0x4D)),
    ("ArrowUp", 0x7E, ext(0x48)), ("ArrowDown", 0x7D, ext(0x50)),
    ("Home", 0x73, ext(0x47)), ("End", 0x77, ext(0x4F)),
    ("PageUp", 0x74, ext(0x49)), ("PageDown", 0x79, ext(0x51)),
    // Modifier. `Meta` adalah Command di macOS dan tombol Windows di Windows.
    ("ShiftLeft", 0x38, 0x2A), ("ShiftRight", 0x3C, 0x36),
    ("ControlLeft", 0x3B, 0x1D), ("ControlRight", 0x3E, ext(0x1D)),
    ("AltLeft", 0x3A, 0x38), ("AltRight", 0x3D, ext(0x38)),
    ("MetaLeft", 0x37, ext(0x5B)), ("MetaRight", 0x36, ext(0x5C)),
    ("CapsLock", 0x39, 0x3A), ("ContextMenu", 0x6E, ext(0x5D)),
    // Tombol fungsi
    ("F1", 0x7A, 0x3B), ("F2", 0x78, 0x3C), ("F3", 0x63, 0x3D), ("F4", 0x76, 0x3E),
    ("F5", 0x60, 0x3F), ("F6", 0x61, 0x40), ("F7", 0x62, 0x41), ("F8", 0x64, 0x42),
    ("F9", 0x65, 0x43), ("F10", 0x6D, 0x44), ("F11", 0x67, 0x57), ("F12", 0x6F, 0x58),
    // Tidak ada di keyboard Mac; F13/F14 adalah padanan yang lazim.
    ("PrintScreen", 0x69, ext(0x37)), ("ScrollLock", 0x6B, 0x46),
    // Numpad
    ("Numpad0", 0x52, 0x52), ("Numpad1", 0x53, 0x4F), ("Numpad2", 0x54, 0x50), ("Numpad3", 0x55, 0x51),
    ("Numpad4", 0x56, 0x4B), ("Numpad5", 0x57, 0x4C), ("Numpad6", 0x58, 0x4D), ("Numpad7", 0x59, 0x47),
    ("Numpad8", 0x5B, 0x48), ("Numpad9", 0x5C, 0x49),
    ("NumpadDecimal", 0x41, 0x53), ("NumpadAdd", 0x45, 0x4E), ("NumpadSubtract", 0x4E, 0x4A),
    ("NumpadMultiply", 0x43, 0x37), ("NumpadDivide", 0x4B, ext(0x35)), ("NumpadEnter", 0x4C, ext(0x1C)),
    // Hanya ada di keyboard Mac.
    ("NumpadEqual", 0x51, NONE),
];

/// Kode native untuk `code` di OS `windows` atau macOS.
fn lookup(code: &str, windows: bool) -> Option<u16> {
    let (_, mac, win) = KEYS.iter().find(|(name, _, _)| *name == code)?;
    Some(if windows { *win } else { *mac }).filter(|native| *native != NONE)
}

/// Kode native untuk `code` di OS tempat program ini berjalan. `None` kalau
/// tombol itu tidak dikenal atau tidak ada padanannya di OS ini.
pub fn native_code(code: &str) -> Option<u16> {
    lookup(code, cfg!(target_os = "windows"))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn every_code_is_listed_once() {
        let names: HashSet<&str> = KEYS.iter().map(|(name, _, _)| *name).collect();
        assert_eq!(names.len(), KEYS.len());
    }

    #[test]
    fn no_two_keys_share_a_native_code() {
        for windows in [false, true] {
            let natives: Vec<u16> = KEYS.iter().filter_map(|(name, _, _)| lookup(name, windows)).collect();
            let unique: HashSet<u16> = natives.iter().copied().collect();
            assert_eq!(unique.len(), natives.len(), "kode native ganda (windows = {windows})");
        }
    }

    #[test]
    fn every_key_maps_on_both_systems_except_mac_only_keys() {
        for (name, _, _) in KEYS {
            assert!(lookup(name, false).is_some(), "{name} tidak punya keycode macOS");
            assert_eq!(lookup(name, true).is_some(), *name != "NumpadEqual", "{name}");
        }
    }

    #[test]
    fn known_positions_map_to_documented_codes() {
        // kVK_ANSI_A = 0x00, scancode A = 0x1E; panah kiri adalah tombol extended di Windows.
        assert_eq!((lookup("KeyA", false), lookup("KeyA", true)), (Some(0x00), Some(0x1E)));
        assert_eq!((lookup("ArrowLeft", false), lookup("ArrowLeft", true)), (Some(0x7B), Some(0xE04B)));
        assert_eq!((lookup("MetaLeft", false), lookup("MetaLeft", true)), (Some(0x37), Some(0xE05B)));
    }

    #[test]
    fn unknown_code_is_ignored() {
        assert_eq!(native_code("TombolAneh"), None);
        assert_eq!(native_code(""), None);
    }
}
