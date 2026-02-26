use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
pub struct RecentFile {
    pub path: String,
    pub name: String,
    pub modified: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct FileFilter {
    pub name: String,
    pub extensions: Vec<String>,
}

#[tauri::command]
pub fn open_native_file_dialog(_filters: Vec<FileFilter>) -> Option<PathBuf> {
    // TODO: add tauri-plugin-dialog for native file picker
    // For now the frontend falls back to <input type="file">
    None
}

#[tauri::command]
pub fn read_file_bytes(path: PathBuf) -> Result<Vec<u8>, String> {
    std::fs::read(&path).map_err(|e| format!("failed to read {}: {}", path.display(), e))
}

#[tauri::command]
pub fn write_file_bytes(path: PathBuf, data: Vec<u8>) -> Result<(), String> {
    std::fs::write(&path, &data).map_err(|e| format!("failed to write {}: {}", path.display(), e))
}

#[tauri::command]
pub fn get_recent_files() -> Vec<RecentFile> {
    // TODO: persist recent files list to app data dir
    Vec::new()
}

#[tauri::command]
pub fn launch_external_slicer(stl_path: PathBuf) -> Result<(), String> {
    // TODO: detect installed slicer (PrusaSlicer, Cura, etc.) and open file
    Err(format!(
        "external slicer launch not yet implemented (path={})",
        stl_path.display()
    ))
}
