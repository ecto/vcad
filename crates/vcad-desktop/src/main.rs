#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

use tauri::Manager;

use commands::{bambu, local_ai};

fn main() {
    tauri::Builder::default()
        .manage(bambu::BambuState::new())
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
            bambu::bambu_discover,
            bambu::bambu_connect,
            bambu::bambu_status,
            bambu::bambu_send_print,
            bambu::bambu_control,
            local_ai::local_ai_probe,
            local_ai::local_ai_chat_stream,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
