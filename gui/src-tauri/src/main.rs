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
            commands::backup_current_hermes,
            commands::cloud::cloud_test_connection,
            commands::cloud::cloud_status,
            commands::cloud::cloud_upload,
            commands::cloud::cloud_download_restore,
            commands::cloud::cloud_delete,
            commands::cloud::cloud_sweep_orphans,
            commands::cloud::cloud_server_info,
            commands::cloud::cloud_server_set,
            commands::cloud::cloud_server_clear,
            commands::installer::installer_detect_system,
            commands::installer::installer_install,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
