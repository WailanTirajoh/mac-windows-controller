//! Nama file tujuan untuk file yang diterima.
//!
//! Nama dikirim oleh perangkat lain, jadi diperlakukan sebagai masukan tak
//! terpercaya: tidak boleh keluar dari folder unduhan, dan harus valid di
//! macOS maupun Windows.

use std::path::{Path, PathBuf};

/// Panjang maksimum nama file dalam byte, di bawah batas 255 kebanyakan
/// filesystem dengan ruang untuk akhiran " (n)" dan ".part".
const MAX_NAME_BYTES: usize = 200;

/// Nama yang punya arti khusus di Windows, apa pun ekstensinya.
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1",
    "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Mengubah nama kiriman menjadi nama file biasa yang aman: tanpa komponen
/// folder, tanpa karakter terlarang, dan tidak kosong.
pub fn sanitize(name: &str) -> String {
    // Hanya bagian setelah pemisah folder terakhir, untuk gaya Unix dan Windows.
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();

    let cleaned: String = base
        .chars()
        .filter(|character| !character.is_control() && !matches!(character, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
        .collect();
    // Windows membuang titik dan spasi di akhir nama; awalan titik membuat
    // file tersembunyi di macOS, dan "." serta ".." berarti folder.
    let edge = |character: char| character == '.' || character.is_whitespace();
    let cleaned = truncate(cleaned.trim_matches(edge), MAX_NAME_BYTES);
    let mut cleaned = cleaned.trim_end_matches(edge).to_owned();

    let stem = cleaned.split('.').next().unwrap_or_default();
    if RESERVED.iter().any(|reserved| reserved.eq_ignore_ascii_case(stem)) {
        cleaned.insert(0, '_');
    }
    if cleaned.is_empty() {
        "file".to_owned()
    } else {
        cleaned
    }
}

/// Memotong `name` ke `max_bytes` byte, mempertahankan ekstensinya kalau bisa.
fn truncate(name: &str, max_bytes: usize) -> String {
    if name.len() <= max_bytes {
        return name.to_owned();
    }
    let (stem, extension) = match name.rfind('.') {
        // Ekstensi yang sangat panjang bukan ekstensi sungguhan; tidak dipertahankan.
        Some(dot) if dot > 0 && name.len() - dot <= 16 => name.split_at(dot),
        _ => (name, ""),
    };
    let mut end = max_bytes - extension.len();
    while !stem.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{extension}", &stem[..end])
}

/// Kandidat ke-`attempt` untuk `name` di `dir`: nama asli, lalu "nama (1).ext",
/// "nama (2).ext", dan seterusnya.
pub fn candidate(dir: &Path, name: &str, attempt: u32) -> PathBuf {
    if attempt == 0 {
        return dir.join(name);
    }
    match name.rfind('.') {
        Some(dot) if dot > 0 => dir.join(format!("{} ({attempt}){}", &name[..dot], &name[dot..])),
        _ => dir.join(format!("{name} ({attempt})")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_names_are_kept() {
        assert_eq!(sanitize("laporan Q3.pdf"), "laporan Q3.pdf");
        assert_eq!(sanitize("foto-liburan.JPG"), "foto-liburan.JPG");
        assert_eq!(sanitize("日本語.txt"), "日本語.txt");
    }

    #[test]
    fn folder_components_are_dropped() {
        assert_eq!(sanitize("../../etc/passwd"), "passwd");
        assert_eq!(sanitize("/Users/korban/.ssh/authorized_keys"), "authorized_keys");
        assert_eq!(sanitize(r"C:\Windows\System32\drivers\etc\hosts"), "hosts");
        assert_eq!(sanitize(r"..\..\evil.exe"), "evil.exe");
    }

    #[test]
    fn dot_names_cannot_refer_to_folders_or_hide_the_file() {
        assert_eq!(sanitize(".."), "file");
        assert_eq!(sanitize("."), "file");
        assert_eq!(sanitize("foo/.."), "file");
        assert_eq!(sanitize(".bashrc"), "bashrc");
    }

    #[test]
    fn characters_invalid_on_windows_are_removed() {
        assert_eq!(sanitize("a<b>c:d\"e|f?g*h.txt"), "abcdefgh.txt");
        assert_eq!(sanitize("baris\nbaru\t.txt"), "barisbaru.txt");
        assert_eq!(sanitize("akhiran titik. . ."), "akhiran titik");
    }

    #[test]
    fn windows_device_names_are_defused() {
        assert_eq!(sanitize("CON"), "_CON");
        assert_eq!(sanitize("nul.txt"), "_nul.txt");
        assert_eq!(sanitize("COM1.tar.gz"), "_COM1.tar.gz");
        assert_eq!(sanitize("CONTOH.txt"), "CONTOH.txt");
    }

    #[test]
    fn empty_results_fall_back_to_a_default_name() {
        assert_eq!(sanitize(""), "file");
        assert_eq!(sanitize("   "), "file");
        assert_eq!(sanitize("???"), "file");
    }

    #[test]
    fn long_names_are_shortened_but_keep_their_extension() {
        let long = format!("{}.pdf", "a".repeat(400));
        let shortened = sanitize(&long);
        assert_eq!(shortened.len(), MAX_NAME_BYTES);
        assert!(shortened.ends_with(".pdf"));

        // Karakter multibyte tidak boleh terbelah.
        let wide = sanitize(&"é".repeat(300));
        assert!(wide.len() <= MAX_NAME_BYTES);
        assert!(wide.chars().all(|character| character == 'é'));
    }

    #[test]
    fn candidates_add_a_counter_before_the_extension() {
        let dir = Path::new("/unduhan");
        assert_eq!(candidate(dir, "foto.jpg", 0), dir.join("foto.jpg"));
        assert_eq!(candidate(dir, "foto.jpg", 1), dir.join("foto (1).jpg"));
        assert_eq!(candidate(dir, "arsip.tar.gz", 2), dir.join("arsip.tar (2).gz"));
        assert_eq!(candidate(dir, "README", 3), dir.join("README (3)"));
    }
}
