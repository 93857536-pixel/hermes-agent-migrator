#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::scan,
            commands::pack,
            commands::check_environment,
            commands::restore,
            commands::verify_package,
            commands::backup_current_hermes
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
