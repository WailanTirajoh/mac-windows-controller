// Tanpa jendela konsol di build rilis Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod permissions;
mod selftest;
mod state;
mod viewer;
mod viewer_frame;

fn main() {
    // Log ke stderr; hanya terlihat kalau aplikasi dijalankan dari terminal.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(tracing::Level::INFO)
        .init();

    tauri::Builder::default()
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
            viewer::open_viewer,
            viewer::viewer_start,
            viewer::viewer_input,
            viewer::viewer_select_display,
            viewer::viewer_request_keyframe,
            selftest::selftest_stream,
            selftest::selftest_report
        ])
        .setup(|app| {
            match selftest::requested() {
                // Uji mandiri memakai node sendiri dan tidak mengiklankan perangkat.
                Some(mode) => selftest::start(app.handle(), mode)?,
                None => state::start(app.handle())?,
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("gagal menjalankan Sambung");
}
