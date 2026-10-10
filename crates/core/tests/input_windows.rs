//! Injeksi input sungguhan di Windows: event disuntikkan lewat [`SystemInput`],
//! lalu hasilnya dibaca kembali dari OS.
//!
//! Test ini menggerakkan mouse dan menekan tombol sungguhan, jadi tidak ikut
//! `cargo test` biasa. Jangan sentuh mouse dan keyboard selama berjalan:
//!
//! ```text
//! cargo test -p sambung-core --test input_windows -- --ignored
//! ```
//!
//! Tombol dikirim ke jendela kecil milik test ini, supaya tidak ada aplikasi
//! lain yang menerimanya. Kalau Windows menolak memindahkan fokus ke jendela
//! itu, test menunggu sampai jendelanya diklik.
#![cfg(target_os = "windows")]

use std::ptr::{null, null_mut};
use std::sync::{mpsc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use sambung_core::capture::{self, DisplayInfo};
use sambung_core::input::{to_global, InputBackend, SystemInput};
use sambung_core::protocol::{InputEvent, MouseButton};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{EnumDisplaySettingsW, COLOR_WINDOW, DEVMODEW, ENUM_CURRENT_SETTINGS, HBRUSH};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_APPS, VK_BACK, VK_DELETE, VK_DIVIDE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_HOME, VK_INSERT,
    VK_LBUTTON, VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_NEXT, VK_PRIOR, VK_RCONTROL, VK_RETURN,
    VK_RIGHT, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SPACE, VK_TAB, VK_UP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetCursorPos, GetForegroundWindow, GetMessageW,
    GetWindowRect, RegisterClassW, SetForegroundWindow, SetWindowsHookExW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_EXTENDED,
    LLKHF_INJECTED, LLKHF_UP, MSG, WH_KEYBOARD_LL, WM_CLOSE, WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_SYSCOMMAND, WNDCLASSW,
    WS_CAPTION, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
};

/// Waktu tunggu sampai OS mencerminkan event yang baru disuntikkan.
const SETTLE: Duration = Duration::from_millis(500);
const FOCUS_TIMEOUT: Duration = Duration::from_secs(30);

/// Satu event keyboard hasil injeksi, seperti yang dilihat Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SeenKey {
    vk: u16,
    extended: bool,
    down: bool,
}

static SEEN_KEYS: Mutex<Vec<SeenKey>> = Mutex::new(Vec::new());
/// `(horizontal, delta)` tiap pesan roda mouse yang sampai ke jendela test.
static SEEN_WHEEL: Mutex<Vec<(bool, i16)>> = Mutex::new(Vec::new());
/// Test di file ini berbagi mouse, keyboard, dan fokus, jadi dijalankan satu per satu.
static SERIAL: Mutex<()> = Mutex::new(());

unsafe extern "system" fn on_key(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: untuk hook keyboard tingkat rendah, `lparam` menunjuk KBDLLHOOKSTRUCT.
        let info = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        if info.flags & LLKHF_INJECTED != 0 {
            SEEN_KEYS.lock().unwrap().push(SeenKey {
                vk: info.vkCode as u16,
                extended: info.flags & LLKHF_EXTENDED != 0,
                down: info.flags & LLKHF_UP == 0,
            });
        }
    }
    // SAFETY: meneruskan argumen hook apa adanya.
    unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn on_message(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        // Jendela tidak boleh tertutup atau masuk mode menu karena tombol yang diuji.
        WM_CLOSE | WM_SYSCOMMAND => 0,
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let delta = (wparam >> 16) as u16 as i16;
            SEEN_WHEEL.lock().unwrap().push((message == WM_MOUSEHWHEEL, delta));
            0
        }
        // SAFETY: meneruskan pesan apa adanya ke penanganan bawaan.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Membuat jendela penerima tombol dan memasang hook keyboard, lalu melayani
/// pesannya selama proses test hidup. Mengembalikan handle jendela.
fn start_window() -> isize {
    let (ready, created) = mpsc::channel();
    std::thread::spawn(move || {
        let class = wide("SambungUjiInput");
        let title = wide("Sambung: uji input (klik di sini)");
        // SAFETY: semua pointer menunjuk data yang hidup sampai panggilan selesai;
        // jendela dan hook dibuat serta dilayani di thread ini.
        unsafe {
            let module = GetModuleHandleW(null());
            RegisterClassW(&WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(on_message),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: module,
                hIcon: null_mut(),
                hCursor: null_mut(),
                hbrBackground: (COLOR_WINDOW + 1) as HBRUSH,
                lpszMenuName: null(),
                lpszClassName: class.as_ptr(),
            });
            let window = CreateWindowExW(
                WS_EX_TOPMOST,
                class.as_ptr(),
                title.as_ptr(),
                WS_POPUP | WS_CAPTION | WS_VISIBLE,
                200,
                200,
                480,
                240,
                null_mut(),
                null_mut(),
                module,
                null(),
            );
            let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(on_key), module, 0);
            ready.send((window as isize, !hook.is_null())).unwrap();

            let mut message: MSG = std::mem::zeroed();
            while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
                DispatchMessageW(&message);
            }
        }
    });
    let (window, hooked) = created.recv().unwrap();
    assert!(window != 0, "jendela uji gagal dibuat");
    assert!(hooked, "hook keyboard gagal dipasang");
    window
}

/// Yang dibutuhkan tiap test: giliran eksklusif, jendela uji yang sedang
/// fokus, dan proses yang sadar DPI seperti aplikasi Tauri.
struct Harness {
    window: isize,
    _turn: MutexGuard<'static, ()>,
}

impl Harness {
    fn acquire() -> Self {
        static WINDOW: OnceLock<isize> = OnceLock::new();
        let turn = SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let window = *WINDOW.get_or_init(|| {
            // Harus sebelum jendela pertama dibuat. Aplikasi Tauri juga berjalan
            // dengan kesadaran DPI ini, jadi koordinatnya piksel fisik.
            // SAFETY: tanpa prasyarat.
            unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
            start_window()
        });
        let harness = Self { window, _turn: turn };
        harness.focus();
        harness
    }

    fn focused(&self) -> bool {
        // SAFETY: tanpa prasyarat.
        unsafe { GetForegroundWindow() as isize == self.window }
    }

    fn focus(&self) {
        // SAFETY: handle jendela milik proses ini.
        unsafe { SetForegroundWindow(self.window as HWND) };
        if !wait_until(SETTLE, || self.focused()) {
            eprintln!("Klik jendela \"Sambung: uji input\" supaya test bisa mengirim tombol ke sana...");
        }
        assert!(
            wait_until(FOCUS_TIMEOUT, || self.focused()),
            "jendela uji tidak mendapat fokus; tombol tidak dikirim"
        );
    }

    /// Titik tengah jendela uji sebagai posisi 0..1 di `display`.
    fn center_in(&self, display: &DisplayInfo) -> (f64, f64) {
        let mut area = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: `area` valid dan handle jendela milik proses ini.
        unsafe { GetWindowRect(self.window as HWND, &mut area) };
        (
            (f64::from(area.left + area.right) / 2.0 - display.x) / display.width,
            (f64::from(area.top + area.bottom) / 2.0 - display.y) / display.height,
        )
    }
}

fn wait_until(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn is_down(vk: u16) -> bool {
    // SAFETY: tanpa prasyarat.
    unsafe { GetAsyncKeyState(i32::from(vk)) as u16 & 0x8000 != 0 }
}

fn cursor() -> (i32, i32) {
    let mut point = POINT { x: 0, y: 0 };
    // SAFETY: `point` valid.
    assert!(unsafe { GetCursorPos(&mut point) } != 0, "posisi kursor tidak terbaca");
    (point.x, point.y)
}

fn key(code: &str, down: bool) -> InputEvent {
    InputEvent::Key {
        code: code.to_owned(),
        down,
    }
}

fn primary(displays: &[DisplayInfo]) -> &DisplayInfo {
    displays.iter().find(|display| display.is_primary).expect("tidak ada layar utama")
}

/// `(code, virtual key, extended)` untuk layout US.
fn expected_keys() -> Vec<(String, u16, bool)> {
    let mut keys: Vec<(String, u16, bool)> = [
        ("KeyA", 0x41, false),
        ("KeyM", 0x4D, false),
        ("KeyZ", 0x5A, false),
        ("Digit1", 0x31, false),
        ("Digit0", 0x30, false),
        ("Space", VK_SPACE, false),
        ("Tab", VK_TAB, false),
        ("Backspace", VK_BACK, false),
        ("Escape", VK_ESCAPE, false),
        ("Enter", VK_RETURN, false),
        ("NumpadEnter", VK_RETURN, true),
        ("NumpadDivide", VK_DIVIDE, true),
        ("ArrowLeft", VK_LEFT, true),
        ("ArrowRight", VK_RIGHT, true),
        ("ArrowUp", VK_UP, true),
        ("ArrowDown", VK_DOWN, true),
        ("Home", VK_HOME, true),
        ("End", VK_END, true),
        ("PageUp", VK_PRIOR, true),
        ("PageDown", VK_NEXT, true),
        ("Insert", VK_INSERT, true),
        ("Delete", VK_DELETE, true),
        ("ShiftLeft", VK_LSHIFT, false),
        // Scancode Shift kanan tidak berawalan E0, tetapi Windows tetap menandainya extended.
        ("ShiftRight", VK_RSHIFT, true),
        ("ControlLeft", VK_LCONTROL, false),
        ("ControlRight", VK_RCONTROL, true),
        ("AltLeft", VK_LMENU, false),
        ("AltRight", VK_RMENU, true),
        ("ContextMenu", VK_APPS, true),
    ]
    .into_iter()
    .map(|(code, vk, extended)| (code.to_owned(), vk, extended))
    .collect();
    keys.extend((0..12).map(|index| (format!("F{}", index + 1), VK_F1 + index, false)));
    // Tombol Windows paling akhir: kalau menu Start sampai terbuka, tombol lain sudah teruji.
    keys.push(("MetaLeft".to_owned(), VK_LWIN, true));
    keys.push(("MetaRight".to_owned(), VK_RWIN, true));
    keys
}

#[test]
#[ignore = "menggerakkan mouse sungguhan"]
fn display_list_is_in_physical_pixels() {
    let _harness = Harness::acquire();
    let displays = capture::list_displays().unwrap();
    let main = primary(&displays);

    // SAFETY: DEVMODEW hanya berisi bilangan, jadi nol adalah nilai yang valid.
    let mut mode: DEVMODEW = unsafe { std::mem::zeroed() };
    mode.dmSize = size_of::<DEVMODEW>() as u16;
    // SAFETY: `mode` valid dan `dmSize`-nya sudah diisi; nama perangkat kosong berarti layar utama.
    assert!(unsafe { EnumDisplaySettingsW(null(), ENUM_CURRENT_SETTINGS, &mut mode) } != 0);

    // Resolusi mode layar tidak ikut skala tampilan.
    assert_eq!((main.pixel_width, main.pixel_height), (mode.dmPelsWidth, mode.dmPelsHeight));
    assert_eq!((main.width, main.height), (f64::from(mode.dmPelsWidth), f64::from(mode.dmPelsHeight)));
    assert_eq!((main.x, main.y), (0.0, 0.0));
}

#[test]
#[ignore = "menggerakkan mouse sungguhan"]
fn move_lands_on_the_expected_pixel_of_every_display() {
    let _harness = Harness::acquire();
    let mut injector = SystemInput.open().unwrap();
    let mut failures = Vec::new();

    for display in capture::list_displays().unwrap() {
        for (x, y) in [(0.5, 0.5), (0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0), (0.25, 0.75)] {
            injector.inject(&InputEvent::Move { x, y }, &display).unwrap();
            let wanted = to_global(x, y, &display);
            let near = |at: (i32, i32)| (at.0 - wanted.0).abs() <= 1 && (at.1 - wanted.1).abs() <= 1;
            if !wait_until(SETTLE, || near(cursor())) {
                failures.push(format!("layar {} ({x}, {y}): kursor di {:?}, seharusnya {wanted:?}", display.id, cursor()));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
#[ignore = "menekan tombol sungguhan"]
fn keys_reach_windows_as_the_expected_virtual_keys() {
    let harness = Harness::acquire();
    let display = primary(&capture::list_displays().unwrap()).clone();
    let mut injector = SystemInput.open().unwrap();
    let mut failures = Vec::new();

    for (code, vk, extended) in expected_keys() {
        assert!(harness.focused(), "fokus pindah dari jendela uji sebelum {code}");
        let is_meta = code.starts_with("Meta");
        SEEN_KEYS.lock().unwrap().clear();

        injector.inject(&key(&code, true), &display).unwrap();
        let pressed = wait_until(SETTLE, || is_down(vk));
        if is_meta {
            // Tombol lain di tengah penekanan mencegah menu Start terbuka saat dilepas.
            injector.inject(&key("ControlLeft", true), &display).unwrap();
            injector.inject(&key("ControlLeft", false), &display).unwrap();
        }
        injector.inject(&key(&code, false), &display).unwrap();
        let released = wait_until(SETTLE, || !is_down(vk));

        let seen: Vec<SeenKey> = SEEN_KEYS
            .lock()
            .unwrap()
            .iter()
            .filter(|seen| !is_meta || seen.vk != VK_LCONTROL)
            .copied()
            .collect();
        let wanted = [true, false].map(|down| SeenKey { vk, extended, down });
        if !pressed || !released || seen != wanted {
            failures.push(format!(
                "{code}: tertekan = {pressed}, terlepas = {released}, terlihat {seen:x?}, seharusnya {wanted:x?}"
            ));
        }
    }
    injector.release_all();
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
#[ignore = "menekan tombol sungguhan"]
fn release_all_lets_go_of_keys_and_buttons() {
    let harness = Harness::acquire();
    let display = primary(&capture::list_displays().unwrap()).clone();
    let mut injector = SystemInput.open().unwrap();
    let (x, y) = harness.center_in(&display);

    injector.inject(&key("KeyA", true), &display).unwrap();
    injector.inject(&key("ShiftLeft", true), &display).unwrap();
    injector.inject(&key("ArrowLeft", true), &display).unwrap();
    let button = InputEvent::Button {
        button: MouseButton::Left,
        down: true,
        x,
        y,
    };
    injector.inject(&button, &display).unwrap();
    let held = [0x41, VK_LSHIFT, VK_LEFT, VK_LBUTTON];
    assert!(wait_until(SETTLE, || held.iter().all(|vk| is_down(*vk))), "tombol tidak tertekan");

    injector.release_all();

    assert!(wait_until(SETTLE, || held.iter().all(|vk| !is_down(*vk))), "masih ada yang tertekan");
}

#[test]
#[ignore = "menggerakkan mouse sungguhan"]
fn scroll_turns_pixels_into_wheel_notches() {
    let harness = Harness::acquire();
    let display = primary(&capture::list_displays().unwrap()).clone();
    let mut injector = SystemInput.open().unwrap();
    let (x, y) = harness.center_in(&display);
    injector.inject(&InputEvent::Move { x, y }, &display).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    SEEN_WHEEL.lock().unwrap().clear();

    // Empat kali seperempat klik menjadi satu klik ke bawah; lalu satu klik ke
    // atas, satu ke kanan, satu ke kiri.
    for _ in 0..4 {
        injector.inject(&InputEvent::Scroll { dx: 0.0, dy: 25.0 }, &display).unwrap();
    }
    injector.inject(&InputEvent::Scroll { dx: 0.0, dy: -100.0 }, &display).unwrap();
    injector.inject(&InputEvent::Scroll { dx: 100.0, dy: 0.0 }, &display).unwrap();
    injector.inject(&InputEvent::Scroll { dx: -100.0, dy: 0.0 }, &display).unwrap();

    // Roda ke bawah bernilai negatif di Windows; roda horizontal ke kanan positif.
    let wanted = [(false, -120), (false, 120), (true, 120), (true, -120)];
    wait_until(SETTLE, || SEEN_WHEEL.lock().unwrap().len() >= wanted.len());
    assert_eq!(*SEEN_WHEEL.lock().unwrap(), wanted);
}
