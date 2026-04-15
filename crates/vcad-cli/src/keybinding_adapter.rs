//! Translation layer between crossterm key events and the shared
//! [`vcad_app::Chord`] type, plus the [`WhenContext`] builder for the TUI.
//!
//! Lives in `vcad-cli` (not `vcad-app`) because `crossterm` is a host
//! dependency — the kernel has no business knowing about terminal events.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use vcad_app::{AppMode, Chord, Key, WhenContext};

use crate::app::App;
use crate::tui::TuiMode;

/// Convert a crossterm [`KeyEvent`] into a platform-agnostic [`Chord`].
///
/// Returns `None` for events that can't be expressed as a chord (key
/// release, modifier-only, unmapped function keys, etc.).
///
/// `Ctrl` maps to `primary` here — the registry's "primary" modifier folds
/// Cmd-on-mac and Ctrl-on-PC into one slot, and terminals always send
/// `Ctrl` for the leader regardless of platform.
pub fn chord_from_crossterm(key: KeyEvent) -> Option<Chord> {
    let primary = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    let key = match key.code {
        KeyCode::Char(c) => Key::Char(c.to_ascii_lowercase()),
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Tab => Key::Tab,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Left => Key::ArrowLeft,
        KeyCode::Right => Key::ArrowRight,
        KeyCode::Up => Key::ArrowUp,
        KeyCode::Down => Key::ArrowDown,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::F(n) if (1..=24).contains(&n) => Key::F(n),
        _ => return None,
    };

    Some(Chord {
        primary,
        shift,
        alt,
        key,
    })
}

/// Map the TUI's [`TuiMode`] (rich state per mode) onto the registry's
/// coarse [`AppMode`] enum used for binding scope filtering.
pub fn app_mode_for(mode: &TuiMode) -> AppMode {
    match mode {
        TuiMode::Normal => AppMode::Normal,
        TuiMode::Command => AppMode::Normal,
        TuiMode::Sketch(_) => AppMode::Sketch,
        TuiMode::Assembly(_) => AppMode::Assembly,
        TuiMode::Physics(_) => AppMode::Physics,
        TuiMode::Cam(_) => AppMode::Cam,
        TuiMode::Print(_) => AppMode::Print,
    }
}

/// Build the current [`WhenContext`] flag set from `App` state. Called
/// each dispatch so the registry sees fresh values.
pub fn when_context_for(app: &App) -> WhenContext {
    let mut ctx = WhenContext::NONE;

    let selection_size = app.selected.len();
    if selection_size > 0 {
        ctx.insert(WhenContext::HAS_SELECTION);
    }
    if selection_size == 2 {
        ctx.insert(WhenContext::TWO_SELECTED);
    }
    if selection_size == 1 {
        ctx.insert(WhenContext::ONE_PART);
    }

    if !app.document.roots.is_empty() {
        ctx.insert(WhenContext::HAS_PARTS);
    }

    if app.menu_state.is_open() {
        ctx.insert(WhenContext::MENU_OPEN);
    }

    if matches!(app.mode, TuiMode::Command) {
        ctx.insert(WhenContext::COMMAND_MODE);
    }

    if app.chat.focused {
        ctx.insert(WhenContext::INPUT_FOCUSED);
    }

    // can_undo / can_redo would need access to the undo stack; the App
    // struct keeps them as private fields so we'd need accessors. Leave
    // off for now — affected commands fall through to the legacy match.

    ctx
}
