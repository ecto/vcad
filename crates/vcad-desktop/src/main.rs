#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

use tauri::Manager;

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            // macOS: activate the app so the window actually appears
            // (raw binaries launched from terminal aren't auto-activated)
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(tauri::ActivationPolicy::Regular);
            }
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::printer::discover_printers,
            commands::printer::send_to_printer,
            commands::files::open_native_file_dialog,
            commands::files::read_file_bytes,
            commands::files::write_file_bytes,
            commands::files::get_recent_files,
            commands::files::launch_external_slicer,
            commands::system::get_platform_info,
            commands::system::is_desktop,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
