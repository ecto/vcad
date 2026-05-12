#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod menu;
mod platform;

use tauri::Manager;

use commands::{bambu, context_menu, local_ai};

/// Closes the native splashscreen window and reveals the main window.
///
/// Called by the frontend once React has mounted, so the user transitions
/// from the static splash HTML directly into the in-app `<Splash>` (which
/// owns the rest of bootstrap progress).
#[tauri::command]
fn close_splashscreen(app: tauri::AppHandle) {
    if let Some(splash) = app.get_webview_window("splashscreen") {
        let _ = splash.close();
    }
    if let Some(main) = app.get_webview_window("main") {
        platform::apply_window_effects(&main);
        let _ = main.show();
        let _ = main.set_focus();
    }
}

fn main() {
    vcad_i18n::init(&vcad_i18n::Locale::from_env());
    tauri::Builder::default()
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_opener::init())
        .manage(bambu::BambuState::new())
        .setup(|app| {
            // macOS: activate the app so the window actually appears
            // (raw binaries launched from terminal aren't auto-activated)
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(tauri::ActivationPolicy::Regular);
            }
            menu::install(&app.handle())?;
            // Holds the live popup menu's items so they outlive the click
            // closures — Tauri's popup is fire-and-forget and the OS keeps
            // a weak ref to the menu object.
            app.manage(context_menu::ContextMenuState::<tauri::Wry>::new());
            // Main window stays hidden until the frontend invokes
            // `close_splashscreen` (after React mounts). The splashscreen
            // window declared in tauri.conf.json is the only thing visible
            // during the cold-start gap.
            Ok(())
        })
        .on_menu_event(|app, event| {
            let id = event.id().as_ref();
            // Top-level menu and popup menus share Tauri's single event
            // stream. We dispatch both: top-level ids land on the
            // `menu-command` channel, popup ids on `context-menu-select`.
            // The webview only listens to the relevant one for each
            // surface, so harmless overlap if an id collides.
            menu::handle_event(app, id);
            context_menu::handle_event(app, id);
        })
        .invoke_handler(tauri::generate_handler![
            close_splashscreen,
            bambu::bambu_discover,
            bambu::bambu_connect,
            bambu::bambu_status,
            bambu::bambu_send_print,
            bambu::bambu_control,
            local_ai::local_ai_probe,
            local_ai::local_ai_chat_stream,
            menu::set_menu_enabled,
            context_menu::show_context_menu,
            platform::set_document_edited,
            platform::set_represented_filename,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
