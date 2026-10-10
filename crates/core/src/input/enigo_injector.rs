//! Injeksi input lewat `enigo` (CGEvent di macOS, SendInput di Windows).

use std::collections::HashSet;

use anyhow::anyhow;
use enigo::{Axis, Button, Direction, Enigo, Mouse, Settings};
#[cfg(target_os = "macos")]
use enigo::{Coordinate, Keyboard};

use super::{keymap, to_global, InputInjector};
use crate::capture::DisplayInfo;
use crate::protocol::{InputEvent, MouseButton};

/// Jarak scroll (piksel) yang dianggap satu "klik" roda mouse di Windows.
#[cfg(target_os = "windows")]
const PIXELS_PER_NOTCH: f64 = 100.0;

pub struct EnigoInjector {
    enigo: Enigo,
    held_keys: HashSet<u16>,
    held_buttons: HashSet<MouseButton>,
    /// Sisa scroll pecahan yang belum cukup untuk satu langkah.
    scroll_remainder: (f64, f64),
}

impl EnigoInjector {
    pub fn new() -> anyhow::Result<Self> {
        let settings = Settings {
            // Izin diminta lewat layar onboarding aplikasi, bukan dialog dari sini.
            open_prompt_to_get_permissions: false,
            ..Settings::default()
        };
        let enigo = Enigo::new(&settings).map_err(|error| {
            anyhow!("injeksi input tidak tersedia ({error}); di macOS, berikan izin Accessibility")
        })?;
        Ok(Self {
            enigo,
            held_keys: HashSet::new(),
            held_buttons: HashSet::new(),
            scroll_remainder: (0.0, 0.0),
        })
    }

    #[cfg(target_os = "macos")]
    fn move_to(&mut self, x: f64, y: f64, display: &DisplayInfo) -> anyhow::Result<()> {
        let (x, y) = to_global(x, y, display);
        self.enigo.move_mouse(x, y, Coordinate::Abs)?;
        Ok(())
    }

    /// Gerakan absolut `enigo` di Windows hanya menjangkau monitor utama,
    /// jadi di sini dikirim sendiri relatif terhadap seluruh desktop virtual.
    #[cfg(target_os = "windows")]
    fn move_to(&mut self, x: f64, y: f64, display: &DisplayInfo) -> anyhow::Result<()> {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
            SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK,
            MOUSEINPUT,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
        };

        let (x, y) = to_global(x, y, display);
        // SAFETY: `GetSystemMetrics` tanpa prasyarat; `SendInput` menerima satu
        // struktur INPUT yang valid beserta ukurannya.
        let sent = unsafe {
            let dx = super::to_absolute_units(x, GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_CXVIRTUALSCREEN));
            let dy = super::to_absolute_units(y, GetSystemMetrics(SM_YVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
            let input = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx,
                        dy,
                        mouseData: 0,
                        dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            SendInput(1, &input, size_of::<INPUT>() as i32)
        };
        anyhow::ensure!(sent == 1, "Windows menolak gerakan mouse");
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn key(&mut self, native: u16, down: bool) -> anyhow::Result<()> {
        self.enigo.raw(native, direction(down))?;
        Ok(())
    }

    /// `enigo.raw` di Windows hanya menandai sebagian tombol sebagai extended:
    /// tombol Windows dan Menu tidak sampai, dan NumpadEnter terbaca sebagai
    /// Enter biasa. Jadi scancode dikirim sendiri.
    #[cfg(target_os = "windows")]
    fn key(&mut self, native: u16, down: bool) -> anyhow::Result<()> {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT};

        let (scan, flags) = keymap::windows_key_input(native, down);
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: 0,
                    wScan: scan,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        // SAFETY: `SendInput` menerima satu struktur INPUT yang valid beserta ukurannya.
        let sent = unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) };
        anyhow::ensure!(sent == 1, "Windows menolak event keyboard");
        Ok(())
    }

    /// Mengubah jarak scroll menjadi langkah utuh, menyimpan sisanya.
    fn take_steps(remainder: &mut f64, delta: f64, step: f64) -> i32 {
        *remainder += delta;
        let steps = (*remainder / step).trunc();
        *remainder -= steps * step;
        steps as i32
    }

    #[cfg(target_os = "macos")]
    fn scroll(&mut self, dx: f64, dy: f64) -> anyhow::Result<()> {
        let x = Self::take_steps(&mut self.scroll_remainder.0, dx, 1.0);
        let y = Self::take_steps(&mut self.scroll_remainder.1, dy, 1.0);
        if x != 0 {
            self.enigo.smooth_scroll(x, Axis::Horizontal)?;
        }
        if y != 0 {
            self.enigo.smooth_scroll(y, Axis::Vertical)?;
        }
        Ok(())
    }

    #[cfg(target_os = "windows")]
    fn scroll(&mut self, dx: f64, dy: f64) -> anyhow::Result<()> {
        let x = Self::take_steps(&mut self.scroll_remainder.0, dx, PIXELS_PER_NOTCH);
        let y = Self::take_steps(&mut self.scroll_remainder.1, dy, PIXELS_PER_NOTCH);
        if x != 0 {
            self.enigo.scroll(x, Axis::Horizontal)?;
        }
        if y != 0 {
            self.enigo.scroll(y, Axis::Vertical)?;
        }
        Ok(())
    }
}

/// Membuat koneksi input dengan permintaan izin menyala, lalu membuangnya.
#[cfg(target_os = "macos")]
pub fn prompt_for_access() {
    let settings = Settings {
        open_prompt_to_get_permissions: true,
        ..Settings::default()
    };
    drop(Enigo::new(&settings));
}

const fn button(button: MouseButton) -> Button {
    match button {
        MouseButton::Left => Button::Left,
        MouseButton::Right => Button::Right,
        MouseButton::Middle => Button::Middle,
    }
}

const fn direction(down: bool) -> Direction {
    if down {
        Direction::Press
    } else {
        Direction::Release
    }
}

impl InputInjector for EnigoInjector {
    fn inject(&mut self, event: &InputEvent, display: &DisplayInfo) -> anyhow::Result<()> {
        match event {
            InputEvent::Move { x, y } => self.move_to(*x, *y, display)?,
            InputEvent::Button {
                button: pressed,
                down,
                x,
                y,
            } => {
                self.move_to(*x, *y, display)?;
                self.enigo.button(button(*pressed), direction(*down))?;
                if *down {
                    self.held_buttons.insert(*pressed);
                } else {
                    self.held_buttons.remove(pressed);
                }
            }
            InputEvent::Scroll { dx, dy } => self.scroll(*dx, *dy)?,
            InputEvent::Key { code, down } => {
                // Tombol yang tidak dikenal diabaikan, bukan dianggap error.
                let Some(native) = keymap::native_code(code) else {
                    return Ok(());
                };
                self.key(native, *down)?;
                if *down {
                    self.held_keys.insert(native);
                } else {
                    self.held_keys.remove(&native);
                }
            }
        }
        Ok(())
    }

    fn release_all(&mut self) {
        for native in std::mem::take(&mut self.held_keys) {
            let _ = self.key(native, false);
        }
        for pressed in self.held_buttons.drain() {
            let _ = self.enigo.button(button(pressed), Direction::Release);
        }
    }
}
