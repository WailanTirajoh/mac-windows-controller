# Tugas: siapkan, uji, dan build Sambung di mesin Windows baru

Kamu bekerja di mesin Windows yang masih kosong (belum ada alat pengembangan). Tugasmu membawa project **Sambung** dari nol sampai bisa dikembangkan dan di-build di mesin ini, lalu memverifikasi dan memperbaiki bagian khusus Windows yang belum pernah dijalankan di Windows sungguhan.

Balas dalam bahasa Indonesia. Komentar kode dan pesan commit di repo ini juga berbahasa Indonesia; ikuti gaya kode di sekitarnya.

## Tentang project

Sambung adalah aplikasi remote desktop antara Mac dan Windows lewat jaringan lokal. Tiap instalasi bisa mengendalikan perangkat lain maupun dikendalikan.

- Repo: `https://github.com/WailanTirajoh/mac-windows-controller` (public, branch `main`).
- Stack: inti Rust (`crates/core`, tanpa dependensi UI), aplikasi Tauri 2 (`src-tauri`), antarmuka React + TypeScript + Tailwind (`ui`).
- Transport: QUIC (`quinn`) dengan mutual TLS; identitas perangkat dicek lewat sidik jari sertifikat.
- Pairing: kode 6 angka sekali pakai yang tampil di perangkat tujuan, dibuktikan dengan SPAKE2 dan diikat ke sesi TLS.
- Video: capture layar → H.264 software (OpenH264) → satu stream QUIC per frame → decode di webview lewat WebCodecs.
- Fitur lain: input mouse/keyboard, pilih monitor, clipboard teks, transfer file, discovery mDNS.

Baca `README.md` lebih dulu, lalu komentar modul di `crates/core/src/capture/mod.rs`, `input/mod.rs`, `session/host.rs`, dan `node/responder.rs`.

## Keadaan saat ini

Project dikembangkan di Mac. Yang sudah terbukti:

- `cargo test --workspace` lolos di macOS, dan di runner `windows-latest` GitHub Actions.
- Installer NSIS (`.exe`) berhasil dibuat oleh GitHub Actions.
- Di Mac, dua uji mandiri aplikasi (lihat di bawah) lolos, juga pada build rilis.

Yang **belum pernah dijalankan di Windows sungguhan**, dan menjadi fokusmu:

| Bagian | Lokasi |
|---|---|
| Capture layar (Windows.Graphics.Capture) | `crates/core/src/capture/windows.rs` |
| Konversi BGRA → I420 dan penskalaan | `crates/core/src/video/convert.rs` (logikanya sudah dites di Mac) |
| Injeksi mouse, keyboard, scroll | `crates/core/src/input/enigo_injector.rs`, cabang `#[cfg(target_os = "windows")]` |
| Tabel scancode | `crates/core/src/input/keymap.rs` |
| Clipboard | `crates/core/src/clipboard.rs` |
| Decode H.264 di WebView2 | `ui/src/viewer/decoder.ts` |
| Discovery mDNS | `crates/core/src/discovery.rs` |
| Ikon tray, tutup ke tray | `src-tauri/src/tray.rs`, `src-tauri/src/main.rs` |
| "Tampilkan" file hasil transfer | `src-tauri/src/transfers.rs` (`explorer /select,`) |
| Installer | `src-tauri/tauri.conf.json` |

Kode capture dan injeksi Windows sejauh ini hanya ter-compile. Anggap belum benar sampai kamu membuktikannya.

## Aturan kerja

- **Jangan mengubah pengaturan keamanan sistem**: Windows Firewall, pengecualian Defender, UAC, kebijakan eksekusi global. Kalau dibutuhkan (misalnya dialog firewall saat aplikasi pertama dijalankan), jelaskan ke pengguna apa yang harus diklik dan tunggu.
- Memasang alat pengembangan lewat `winget` memang bagian dari tugas ini. Dialog UAC dari installer harus disetujui pengguna; beri tahu sebelum memicunya.
- **Git**: bekerja di branch baru (`windows/<topik>`), commit lokal per perbaikan. Jangan push, membuat PR, atau menyentuh `main` kecuali pengguna memintanya.
- Jangan melemahkan keamanan: pairing, pengecekan sidik jari, aturan siapa boleh membuka stream, dan pembersihan nama file harus tetap seketat sekarang.
- `crates/core` tidak boleh bergantung pada Tauri. Kode per OS berada di balik trait (`CaptureBackend`/`ScreenCapturer`, `InputBackend`/`InputInjector`, `ClipboardBackend`) dan dibatasi `cfg`. Kamu tidak bisa mem-build untuk macOS di sini, jadi jangan mengubah kode macOS atau kode bersama tanpa alasan kuat.
- Tiap perbaikan bug disertai test kalau logikanya bisa dipisah (matematika koordinat, pemetaan tombol, konversi gambar).
- Sebelum tiap commit jalankan: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `pnpm typecheck`.
- Kalau kamu tidak bisa melihat atau mengklik GUI, katakan itu. Minta pengguna melakukan langkah GUI dengan instruksi yang persis (apa yang diklik, apa yang harus terlihat), lalu lanjut dari jawabannya. Jangan melaporkan sesuatu "berhasil" kalau kamu tidak mengamatinya sendiri atau lewat pengguna.
- Laporkan apa adanya: yang gagal disebut gagal beserta keluarannya, yang dilewati disebut dilewati.

## Tahap 1: periksa mesin

Catat versi dan build Windows, arsitektur (x64 atau ARM64), jumlah monitor beserta skala tampilannya, dan apakah `winget` tersedia. Project ini baru dibangun untuk x64; kalau mesinnya ARM64, berhenti dan laporkan dulu.

## Tahap 2: pasang prasyarat

Yang dibutuhkan:

| Alat | Versi | Catatan |
|---|---|---|
| Git | terbaru | |
| Build Tools MSVC + Windows SDK | Visual Studio Build Tools, workload "Desktop development with C++" | Dibutuhkan Rust target MSVC dan untuk meng-compile OpenH264 |
| Rust | **1.92.0** (dikunci di `rust-toolchain.toml`), komponen `clippy` | Lewat `rustup` |
| Node.js | **22** | Versi yang dipakai CI |
| pnpm | **10.33.0** (field `packageManager`) | |
| NASM | terbaru, harus ada di `PATH` | Tanpa NASM, OpenH264 di-build tanpa assembly dan encoder jauh lebih lambat. Build tetap berhasil, jadi ketiadaannya mudah terlewat |
| WebView2 Runtime | terbaru | Sudah ada di Windows 11; periksa di Windows 10 |
| FFmpeg (opsional) | terbaru | `ffprobe`/`ffmpeg` untuk memeriksa hasil capture |

Perintah yang lazim (periksa dengan `winget search` kalau ada ID yang berubah):

```powershell
winget install --id Git.Git -e
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--passive --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
winget install --id Rustlang.Rustup -e
winget install --id OpenJS.NodeJS.LTS -e
winget install --id NASM.NASM -e
```

Setelah itu buka terminal baru, lalu:

```powershell
rustup toolchain install 1.92.0 --component clippy
corepack enable
corepack prepare pnpm@10.33.0 --activate
```

Catatan:

- Kalau `OpenJS.NodeJS.LTS` memasang versi selain 22, pasang 22 secara eksplisit (lihat `winget show --id OpenJS.NodeJS.LTS --versions`).
- Installer NASM sering tidak menambahkan dirinya ke `PATH`. Tambahkan foldernya ke `PATH` pengguna, lalu pastikan `nasm -v` jalan di terminal baru.
- Verifikasi semuanya sebelum lanjut: `git --version`, `rustc --version` (harus 1.92.0 di dalam folder repo), `cargo clippy --version`, `node --version`, `pnpm --version`, `nasm -v`, dan `cl.exe` ditemukan oleh `cargo` (terbukti saat build pertama).

## Tahap 3: ambil kode dan build dasar

Clone ke path pendek untuk menghindari batas panjang path Windows:

```powershell
git config --global core.longpaths true
git clone https://github.com/WailanTirajoh/mac-windows-controller.git C:\dev\sambung
cd C:\dev\sambung
git switch -c windows/verifikasi
pnpm install
pnpm build
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
pnpm typecheck
```

`pnpm build` harus dijalankan sebelum meng-compile aplikasi Tauri, karena aplikasi menyematkan hasil build UI. Build Rust pertama bisa memakan belasan menit.

Kalau ada test yang gagal di sini padahal lolos di CI, selidiki dulu apakah penyebabnya lingkungan (firewall, antivirus, jaringan) sebelum mengubah kode.

Satu test sengaja tidak ikut jalan karena butuh jaringan yang meneruskan multicast. Jalankan terpisah dan catat hasilnya:

```powershell
cargo test -p sambung-core --test discovery -- --ignored
```

## Tahap 4: verifikasi bagian khusus Windows

Kerjakan berurutan. Tiap butir: jalankan, catat keluarannya, perbaiki kalau salah, tambahkan test kalau bisa.

### 4.1 Encoder

```powershell
cargo run -p sambung-core --release --example encode_bench -- 1920 1080 180 10
```

Catat `delta rata-rata` (ms per frame). Sebagai pembanding, Apple M3 mencatat sekitar 10 ms di 1920×1248. Kalau angkanya jauh lebih buruk, periksa apakah NASM benar-benar terpakai saat build `openh264-sys2`.

### 4.2 Capture layar

```powershell
cargo run -p sambung-core --release --example capture_dump -- 5 1920 10 60
```

Selama 5 detik itu layar harus berubah (gerakkan jendela), karena layar yang diam tidak menghasilkan frame. Periksa:

- Daftar layar: jumlah, ukuran piksel, posisi, dan mana yang utama cocok dengan pengaturan Windows.
- Ada frame yang ter-capture dan ter-encode, dan `out.h264` tidak kosong.
- Dengan FFmpeg: `ffprobe -v error -count_frames -show_entries stream=width,height,nb_read_frames out.h264`, lalu `ffmpeg -y -i out.h264 -frames:v 1 frame.png` dan lihat gambarnya: warnanya benar (bukan biru dan merah tertukar), tidak miring, tidak terpotong.

Hal yang patut dicurigai:

- Contoh ini program konsol biasa yang tidak sadar DPI, jadi ukuran dari `GetMonitorInfoW` bisa berupa ukuran yang sudah diskala. Ukuran frame sebenarnya diambil per frame dari capture. Di aplikasi Tauri prosesnya sadar DPI; di situlah koordinat harus benar (lihat 4.4).
- Bingkai kuning di sekeliling layar pada Windows versi lama, dan pengaturan capture yang tidak didukung (kode sudah memeriksa dukungan sebelum memakainya; pastikan memang begitu perilakunya).
- Capture monitor kedua, kalau ada.

### 4.3 Decode video dan jalur viewer

Uji mandiri mencetak satu baris `SELFTEST {...}` lalu keluar. Pakai build debug, karena build rilis Windows tidak punya konsol. Biarkan `pnpm dev` berjalan di terminal lain.

```powershell
cargo build -p sambung
$env:SAMBUNG_SELFTEST = "video"; .\target\debug\sambung.exe
$env:SAMBUNG_SELFTEST = "session"; .\target\debug\sambung.exe
Remove-Item Env:SAMBUNG_SELFTEST
```

Yang diharapkan:

- `video`: `hasVideoDecoder` true, `decoded` sama dengan `received`, `errors` kosong. Ini membuktikan WebView2 bisa men-decode H.264 dari encoder kita.
- `session`: `moving` true, `movingAfterQuality` true, `sizeAfterDisplayChange` bernilai `"800x600"`, `errors` kosong, dan baris `SELFTEST_INPUT` memuat event `Move`, `KeyA`, dan `KeyB`. Uji ini menjalankan dua node dalam satu proses dengan layar sintetis, jadi menguji viewer, QUIC, pairing, ganti kualitas, dan pindah layar tanpa menyentuh capture atau input sungguhan.

Kalau `video` gagal, laporkan `userAgent` dan pesan error-nya; edisi Windows N tidak membawa codec media.

### 4.4 Injeksi input

Ini bagian paling berisiko. Bahan pemeriksaan:

- Gerakan mouse absolut dikirim sendiri lewat `SendInput` dengan `MOUSEEVENTF_VIRTUALDESK`, karena gerakan absolut `enigo` di Windows hanya menjangkau monitor utama.
- Tombol dikirim dengan `enigo.raw(scancode)`. Tombol extended ditulis sebagai `0xE0xx` di `keymap.rs`.
- Scroll: `PIXELS_PER_NOTCH = 100` piksel dianggap satu klik roda.

Cara memverifikasi tanpa perangkat kedua: tulis test integrasi khusus Windows yang ditandai `#[ignore]` (karena menggerakkan mouse sungguhan), yang membuka `SystemInput`, menyuntikkan event, lalu membaca hasilnya dari OS:

- Setelah `InputEvent::Move` ke beberapa titik (pojok, tengah) di tiap monitor, `GetCursorPos` harus mengembalikan piksel yang diharapkan, dengan toleransi 1 piksel.
- Setelah `Key` ditekan, `GetAsyncKeyState` untuk virtual key yang sesuai harus menunjukkan tertekan, dan lepas setelah dilepas. Uji setidaknya: huruf, angka, Enter, panah, Home/End, Delete, Ctrl/Alt/Shift kiri dan kanan, tombol Windows, NumpadEnter, NumpadDivide, F1 sampai F12.
- `release_all` melepas semua yang masih tertekan.

Jalankan test itu pada skala tampilan 100% dan pada skala lain (125% atau 150%), di monitor utama dan monitor kedua kalau ada. Koordinat di `DisplayInfo` harus piksel fisik.

Kalau `enigo.raw` salah menangani tombol extended, ganti dengan `SendInput` langsung (`KEYEVENTF_SCANCODE`, ditambah `KEYEVENTF_EXTENDEDKEY` untuk kode `0xE0xx`) memakai `windows-sys`, yang sudah menjadi dependensi.

Batasan yang sudah diketahui dan **bukan** bug: jendela yang berjalan sebagai Administrator dan layar UAC tidak bisa dikendalikan. Jangan mengatasinya dengan menjalankan aplikasi sebagai Administrator.

### 4.5 Aplikasi utuh di satu mesin

Jalankan dua instance. Instance kedua memakai server UI milik instance pertama, jadi `pnpm tauri dev` hanya dijalankan sekali.

```powershell
pnpm tauri dev
# di terminal lain:
$env:SAMBUNG_PROFILE = "b"; .\target\debug\sambung.exe
```

Saat pertama jalan, Windows Firewall akan bertanya; pengguna perlu mengizinkannya untuk jaringan Private. Aplikasi mendengarkan di UDP 47823 (instance kedua memakai port acak) dan memakai mDNS di UDP 5353.

Log aplikasi keluar di terminal: `node berjalan`, `perangkat terlihat`, `sesi masuk dimulai`.

Periksa, lewat pengguna kalau kamu tidak bisa mengoperasikan GUI:

1. Kedua instance saling muncul di daftar perangkat.
2. Pairing: klik Hubungkan di satu instance, kode 6 angka muncul di instance lain, ketik kodenya. Kode salah ditolak dengan sisa percobaan.
3. Jendela viewer terbuka dan menampilkan layar (di satu mesin, layar itu memuat dirinya sendiri; itu wajar).
4. Mouse dan keyboard di viewer sampai ke sistem.
5. Banner "sedang mengendalikan layar ini" muncul di instance tujuan, dan tombol Putuskan bekerja.
6. Kirim file: muncul di `%USERPROFILE%\Downloads\Sambung\profile-b\` atau `Downloads\Sambung\`, dan tombol Tampilkan membuka Explorer di file itu.
7. Clipboard: salin teks di satu sisi selama sesi berjalan. Di satu mesin kedua instance berbagi clipboard yang sama, jadi uji ini baru bermakna dengan dua perangkat; cukup pastikan tidak ada error atau loop.
8. Ikon tray muncul. Menutup jendela utama hanya menyembunyikannya; menu tray punya Buka, Putuskan semua pengendali, dan Keluar.
9. Ganti kualitas (Hemat, Seimbang, Tajam) dan pindah layar di toolbar viewer.

Data aplikasi ada di `%APPDATA%\io.github.wailantirajoh.sambung\` (identitas dan daftar perangkat tersimpan). Hapus folder itu untuk mengulang dari keadaan belum pernah pairing.

Untuk melihat UI tanpa backend: buka `http://localhost:1420/?mock` di browser selama `pnpm dev` berjalan.

## Tahap 5: build installer

```powershell
pnpm tauri build --bundles nsis
```

Hasilnya di `target\release\bundle\nsis\`. Pasang, jalankan dari Start Menu, pastikan jendela utama dan tray muncul, lalu uninstall. Installer belum ditandatangani, jadi peringatan SmartScreen itu wajar.

## Tahap 6: laporan

Akhiri dengan laporan yang memuat:

1. Lingkungan: versi Windows, arsitektur, monitor dan skalanya, versi tiap alat yang terpasang.
2. Hasil `cargo test`, `clippy`, `typecheck`, dan test discovery.
3. Angka `encode_bench` dan `capture_dump`, serta keluaran kedua uji mandiri.
4. Tabel butir 4.2 sampai 4.5 dan Tahap 5: lolos, gagal, atau tidak bisa diverifikasi, masing-masing dengan buktinya.
5. Bug yang ditemukan: gejala, penyebab, perbaikan, commit, dan test yang ditambahkan.
6. Yang masih harus diuji manusia dengan dua perangkat (Mac dan Windows): pairing lintas OS, Mac mengendalikan Windows, Windows mengendalikan Mac, file dua arah, clipboard dua arah, pindah monitor.
7. Daftar commit di branch kerjamu, dan apakah ada yang menunggu keputusan pengguna (push, PR, pengaturan sistem).
