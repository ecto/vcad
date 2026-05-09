//! Platform-specific window dressing.
//!
//! Applies native visual effects after the window is created — currently
//! the macOS sidebar vibrancy that lets our tinted panels pick up the
//! translucent blur users expect from apps like Linear, Things, Finder.
//!
//! Also exposes commands for native macOS title-bar affordances:
//! `setDocumentEdited:` (the dot inside the close traffic light), and
//! `setRepresentedFilename:` (the proxy icon + ⌘-click path popover, even
//! though our title is hidden — once shown it gets the icon for free).

use tauri::{AppHandle, Manager, Runtime, WebviewWindow};

/// Apply any platform-specific window effects (vibrancy, etc.) to `window`.
pub fn apply_window_effects(window: &WebviewWindow) {
    #[cfg(target_os = "macos")]
    mac::apply_vibrancy(window);

    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

/// Toggle the modified-document indicator on the main window. On macOS this
/// renders as a dot inside the close traffic light — the standard signal
/// that a document has unsaved changes.
#[tauri::command]
pub fn set_document_edited<R: Runtime>(app: AppHandle<R>, edited: bool) {
    if let Some(window) = app.get_webview_window("main") {
        #[cfg(target_os = "macos")]
        mac::set_document_edited(&window, edited);
        #[cfg(not(target_os = "macos"))]
        let _ = (window, edited);
    }
}

/// Tell the OS this window represents a real file on disk. Powers the
/// proxy-icon drag and ⌘-click path popover when the title bar is visible;
/// also enables the standard "edited" badge in the Window menu. Pass an
/// empty string to clear.
#[tauri::command]
pub fn set_represented_filename<R: Runtime>(app: AppHandle<R>, path: String) {
    if let Some(window) = app.get_webview_window("main") {
        #[cfg(target_os = "macos")]
        mac::set_represented_filename(&window, &path);
        #[cfg(not(target_os = "macos"))]
        let _ = (window, path);
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use cocoa::appkit::NSWindow;
    use cocoa::base::{id, nil, BOOL, NO, YES};
    use cocoa::foundation::NSString;
    use tauri::WebviewWindow;
    use window_vibrancy::{
        apply_vibrancy as apply_v, NSVisualEffectMaterial, NSVisualEffectState,
    };

    /// Sidebar material — the strong, lively blur used for the leftmost
    /// column in Finder, Mail, Music, Notes. We let it bleed through the
    /// whole window: chrome panels paint translucent over it, the 3D
    /// viewport canvas paints opaque. "Active" state keeps the blur lit
    /// even when the window loses focus, matching system apps.
    pub fn apply_vibrancy(window: &WebviewWindow) {
        if let Err(err) = apply_v(
            window,
            NSVisualEffectMaterial::Sidebar,
            Some(NSVisualEffectState::Active),
            None,
        ) {
            eprintln!("[platform] apply_vibrancy failed: {err}");
        }
    }

    /// `[[NSWindow setDocumentEdited:]]` — the dot inside the close traffic
    /// light. Cheapest possible "this is a real Mac app" signal.
    pub fn set_document_edited(window: &WebviewWindow, edited: bool) {
        if let Ok(ptr) = window.ns_window() {
            unsafe {
                let ns_window = ptr as id;
                let flag: BOOL = if edited { YES } else { NO };
                NSWindow::setDocumentEdited_(ns_window, flag);
            }
        }
    }

    /// `[[NSWindow setRepresentedFilename:]]` — surfaces the proxy icon and
    /// path popover when the title is visible, and unlocks Window menu's
    /// "Recent Documents" automatically. Empty string clears it.
    pub fn set_represented_filename(window: &WebviewWindow, path: &str) {
        if let Ok(ptr) = window.ns_window() {
            unsafe {
                let ns_window = ptr as id;
                let ns_str = NSString::alloc(nil).init_str(path);
                NSWindow::setRepresentedFilename_(ns_window, ns_str);
            }
        }
    }
}
