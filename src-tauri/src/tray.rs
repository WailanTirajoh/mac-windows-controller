//! Ikon tray: tetap terlihat saat jendela utama ditutup, dan menunjukkan
//! kalau ada perangkat yang sedang mengendalikan perangkat ini.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

use crate::state::{show_main_window, AppState};

const TRAY_ID: &str = "sambung";
const MENU_OPEN: &str = "open";
const MENU_DISCONNECT: &str = "disconnect";
const MENU_QUIT: &str = "quit";

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, MENU_OPEN, "Buka Sambung", true, None::<&str>)?;
    let disconnect = MenuItem::with_id(app, MENU_DISCONNECT, "Putuskan semua pengendali", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "Keluar", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &PredefinedMenuItem::separator(app)?,
            &disconnect,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let mut tray = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Sambung")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_OPEN => show_main_window(app),
            MENU_DISCONNECT => disconnect_all(app),
            MENU_QUIT => app.exit(0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

fn disconnect_all(app: &AppHandle) {
    let state = app.state::<AppState>();
    let controllers: Vec<_> = state.controllers.lock().unwrap().keys().copied().collect();
    for fingerprint in controllers {
        state.node.end_incoming_session(&fingerprint);
    }
}

/// Menyesuaikan tray dengan daftar perangkat yang sedang mengendalikan.
pub fn refresh(app: &AppHandle) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let mut names: Vec<String> = app
        .state::<AppState>()
        .controllers
        .lock()
        .unwrap()
        .values()
        .map(|controller| controller.name.clone())
        .collect();
    names.sort();

    let tooltip = if names.is_empty() {
        "Sambung".to_owned()
    } else {
        format!("Sambung: sedang dikendalikan oleh {}", names.join(", "))
    };
    let _ = tray.set_tooltip(Some(tooltip));
    // Di macOS teks ini tampil di menu bar, di samping ikonnya.
    #[cfg(target_os = "macos")]
    let _ = tray.set_title(if names.is_empty() { None } else { Some("Dikendalikan") });
}
