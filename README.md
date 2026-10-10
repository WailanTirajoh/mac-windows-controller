# Sambung

Remote desktop antara Mac dan Windows lewat jaringan lokal. Tiap instalasi bisa mengendalikan perangkat lain maupun dikendalikan, dan bisa saling berkirim file.

- **Tanpa akun dan tanpa server.** Perangkat di WiFi/LAN yang sama saling terdeteksi lewat mDNS.
- **Pairing dengan kode sekali pakai.** Koneksi pertama meminta 6 angka yang tampil di perangkat tujuan. Setelah itu perangkat diingat.
- **Remote desktop**: video layar, mouse, keyboard, pilih monitor.
- **Clipboard teks** dua arah selama sesi, dan **transfer file** dua arah.

Status: versi awal. Lihat [Batasan](#batasan).

## Menjalankan dari sumber

Butuh Rust (stable), Node 22, dan pnpm 10. Di macOS cukup Command Line Tools; di Windows butuh Build Tools MSVC dan WebView2.

```bash
pnpm install
pnpm tauri dev
```

Untuk mencoba dua instance di satu mesin, jalankan instance kedua dengan profil sendiri (data dan port terpisah):

```bash
SAMBUNG_PROFILE=b pnpm tauri dev
```

Installer dibuat dengan `pnpm tauri build`. GitHub Actions membuat `.dmg` dan `.exe` di tiap push ke `main`; hasilnya ada di artifact workflow **Build**.

## Izin di macOS

Supaya Mac bisa dikendalikan, Sambung perlu dua izin di System Settings → Privacy & Security:

- **Screen Recording**, agar layarnya bisa ditampilkan ke perangkat lain.
- **Accessibility**, agar mouse dan keyboard bisa digerakkan dari perangkat lain.

Aplikasi menampilkan pengingat dan tombol ke halaman pengaturannya selama izin itu belum ada. Tanpa Accessibility, sesi tetap jalan tetapi hanya untuk melihat.

Di Windows, izinkan Sambung di Windows Firewall untuk jaringan Private saat diminta.

## Pengujian

```bash
cargo test --workspace     # inti jaringan, pairing, sesi, transfer
pnpm typecheck
```

Test sesi dan transfer menjalankan dua node sungguhan di localhost, dengan capture layar dan input tiruan. Test discovery butuh jaringan yang meneruskan multicast, jadi dijalankan terpisah:

```bash
cargo test -p sambung-core --test discovery -- --ignored
```

Aplikasi punya dua uji mandiri yang mencetak hasilnya ke terminal lalu keluar. Jalankan `pnpm dev` di terminal lain lebih dulu.

```bash
SAMBUNG_SELFTEST=video ./target/debug/sambung     # webview bisa men-decode H.264?
SAMBUNG_SELFTEST=session ./target/debug/sambung   # jalur viewer lengkap, dengan layar sintetis
```

## Susunan kode

| Lokasi | Isi |
|---|---|
| `crates/core` | Inti tanpa UI: discovery, identitas, pairing, QUIC, capture, encoder, input, clipboard, transfer |
| `src-tauri` | Aplikasi Tauri: state, perintah untuk UI, jendela viewer |
| `ui` | Antarmuka React: daftar perangkat, dialog kode, viewer |

Keamanan: tiap perangkat punya sertifikat self-signed. Koneksi memakai QUIC dengan mutual TLS, dan identitas lawan dicek lewat sidik jari sertifikatnya. Pairing memakai SPAKE2 dengan kode 6 angka sebagai password, diikat ke sesi TLS, sehingga kode tidak bisa ditebak offline dan pihak yang menengahi koneksi tidak bisa menyelesaikan pairing.

## Batasan

- Video memakai encoder H.264 software (OpenH264). Sisi panjang gambar dibatasi 1920 piksel.
- Cmd+Tab, tombol Windows, dan Ctrl+Alt+Del tidak diteruskan ke perangkat tujuan.
- Di Windows, jendela yang berjalan sebagai Administrator dan layar UAC tidak bisa dikendalikan.
- Installer belum ditandatangani, jadi SmartScreen dan Gatekeeper menampilkan peringatan.
- Clipboard hanya teks; transfer hanya file, bukan folder; belum ada audio.
- Jaringan dengan client isolation memblokir discovery dan koneksi langsung.
