// Tanpa jendela konsol di build rilis Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod permissions;
mod selftest;
mod state;
mod transfers;
mod tray;
mod viewer;
mod viewer_frame;

use tauri::WindowEvent;

fn main() {
    // Log ke stderr; hanya terlihat kalau aplikasi dijalankan dari terminal.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(tracing::Level::INFO)
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_self,
            commands::list_devices,
            commands::connect_device,
            commands::connect_address,
            commands::submit_pairing_code,
            commands::disconnect_device,
            commands::forget_device,
            commands::reject_pairing,
            commands::set_accept_incoming,
            commands::list_controllers,
            commands::end_incoming_session,
            permissions::get_permissions,
            permissions::request_permission,
            transfers::list_transfers,
            transfers::send_files,
            transfers::cancel_transfer,
            transfers::clear_transfers,
            transfers::reveal_transfer,
            viewer::open_viewer,
            viewer::viewer_start,
            viewer::viewer_input,
            viewer::viewer_select_display,
            viewer::viewer_request_keyframe,
            viewer::viewer_set_clipboard_sync,
            viewer::viewer_set_quality,
            selftest::selftest_stream,
            selftest::selftest_report
        ])
        .setup(|app| {
            match selftest::requested() {
                // Uji mandiri memakai node sendiri dan tidak mengiklankan perangkat.
                Some(mode) => selftest::start(app.handle(), mode)?,
                None => {
                    state::start(app.handle())?;
                    tray::create(app.handle())?;
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Menutup jendela utama hanya menyembunyikannya: aplikasi tetap jalan
            // di tray supaya perangkat ini tetap bisa dihubungi. Keluar lewat menu tray.
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" && selftest::requested().is_none() {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("gagal menjalankan Sambung")
        .run(|app, event| {
            // Klik ikon Dock di macOS menampilkan lagi jendela yang disembunyikan.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                state::show_main_window(app);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}
