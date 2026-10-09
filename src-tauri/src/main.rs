// Tanpa jendela konsol di build rilis Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod selftest;
mod viewer_frame;

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            selftest::selftest_stream,
            selftest::selftest_report
        ])
        .setup(|app| {
            if selftest::requested() {
                selftest::open_window(app.handle())?;
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("gagal menjalankan Sambung");
}
