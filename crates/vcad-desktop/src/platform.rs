//! Platform-specific window dressing.
//!
//! Applies native visual effects after the window is created — currently
//! the macOS "Under Window" vibrancy that lets our tinted panels pick up the
//! translucent blur users expect from apps like Linear, Things, or Notion.

use tauri::WebviewWindow;

/// Apply any platform-specific window effects (vibrancy, etc.) to `window`.
pub fn apply_window_effects(window: &WebviewWindow) {
    #[cfg(target_os = "macos")]
    mac::apply(window);

    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

#[cfg(target_os = "macos")]
mod mac {
    use tauri::WebviewWindow;
    use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};

    pub fn apply(window: &WebviewWindow) {
        // UnderWindowBackground — the subtle material used in native macOS
        // sidebars. "Active" state keeps the blur visible even when the
        // window loses focus, which matches how Finder/Music behave.
        if let Err(err) = apply_vibrancy(
            window,
            NSVisualEffectMaterial::UnderWindowBackground,
            Some(NSVisualEffectState::Active),
            None,
        ) {
            eprintln!("[platform] apply_vibrancy failed: {err}");
        }
    }
}
