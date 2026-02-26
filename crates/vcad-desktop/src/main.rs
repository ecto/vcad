#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

fn main() {
    eprintln!("vcad-desktop: starting tauri app...");

    let result = tauri::Builder::default()
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
        .run(tauri::generate_context!());

    match result {
        Ok(()) => eprintln!("vcad-desktop: exited cleanly"),
        Err(e) => eprintln!("vcad-desktop: error: {e}"),
    }
}
