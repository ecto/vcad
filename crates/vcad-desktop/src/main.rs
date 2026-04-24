#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod menu;
mod platform;

use tauri::Manager;

use commands::{bambu, local_ai};

fn main() {
    vcad_i18n::init(&vcad_i18n::Locale::from_env());
    tauri::Builder::default()
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_deep_link::init())
        .manage(bambu::BambuState::new())
        .setup(|app| {
            // macOS: activate the app so the window actually appears
            // (raw binaries launched from terminal aren't auto-activated)
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(tauri::ActivationPolicy::Regular);
            }
            menu::install(&app.handle())?;
            if let Some(window) = app.get_webview_window("main") {
                platform::apply_window_effects(&window);
                let _ = window.show();
                let _ = window.set_focus();
            }
            Ok(())
        })
        .on_menu_event(|app, event| {
            menu::handle_event(app, event.id().as_ref());
        })
        .invoke_handler(tauri::generate_handler![
            bambu::bambu_discover,
            bambu::bambu_connect,
            bambu::bambu_status,
            bambu::bambu_send_print,
            bambu::bambu_control,
            local_ai::local_ai_probe,
            local_ai::local_ai_chat_stream,
            menu::set_menu_enabled,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
