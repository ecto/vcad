//! Platform-agnostic keyboard chord representation.
//!
//! A [`Chord`] models the abstract key event (modifiers + key) that both the
//! web (`KeyboardEvent`) and TUI (`crossterm::KeyEvent`) hosts normalize into.
//! The registry resolves chords to command IDs — hosts handle native event
//! translation and action dispatch.
//!
//! Only a single "primary" modifier is exposed (Cmd on macOS, Ctrl elsewhere).
//! Each host's adapter folds its native modifier into `primary` at event time,
//! so bindings declared once behave correctly on both platforms.

use serde::{Deserialize, Serialize};

/// A single keypress — a character or named key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum Key {
    Char(char),
    F(u8),
    Enter,
    Esc,
    Tab,
    Backspace,
    Delete,
    Space,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    Home,
    End,
    PageUp,
    PageDown,
    Backtick,
}

impl Key {
    /// Parse a key name. Single characters become `Char`, `F1`..`F24` become
    /// `F(n)`, and named keys are matched case-insensitively.
    pub fn parse(s: &str) -> Result<Self, ChordParseError> {
        let s = s.trim();
        if s.is_empty() {
            return Err(ChordParseError::NoKey(String::new()));
        }
        // Single character — normalize to lowercase so "S" and "s" round-trip.
        if s.chars().count() == 1 {
            let c = s.chars().next().unwrap();
            return Ok(Key::Char(c.to_ascii_lowercase()));
        }
        let lower = s.to_ascii_lowercase();
        let named = match lower.as_str() {
            "enter" | "return" => Key::Enter,
            "esc" | "escape" => Key::Esc,
            "tab" => Key::Tab,
            "backspace" | "bksp" => Key::Backspace,
            "delete" | "del" => Key::Delete,
            "space" => Key::Space,
            "left" | "arrowleft" => Key::ArrowLeft,
            "right" | "arrowright" => Key::ArrowRight,
            "up" | "arrowup" => Key::ArrowUp,
            "down" | "arrowdown" => Key::ArrowDown,
            "home" => Key::Home,
            "end" => Key::End,
            "pageup" | "pgup" => Key::PageUp,
            "pagedown" | "pgdn" => Key::PageDown,
            "backtick" | "grave" => Key::Backtick,
            _ => {
                if let Some(rest) = lower.strip_prefix('f') {
                    if let Ok(n) = rest.parse::<u8>() {
                        if (1..=24).contains(&n) {
                            return Ok(Key::F(n));
                        }
                    }
                }
                return Err(ChordParseError::UnknownKey(s.to_string()));
            }
        };
        Ok(named)
    }

    /// Canonical string form (ASCII, round-trippable through `parse`).
    pub fn to_canonical(&self) -> String {
        match self {
            Key::Char(c) => c.to_ascii_uppercase().to_string(),
            Key::F(n) => format!("F{n}"),
            Key::Enter => "Enter".into(),
            Key::Esc => "Esc".into(),
            Key::Tab => "Tab".into(),
            Key::Backspace => "Backspace".into(),
            Key::Delete => "Delete".into(),
            Key::Space => "Space".into(),
            Key::ArrowLeft => "Left".into(),
            Key::ArrowRight => "Right".into(),
            Key::ArrowUp => "Up".into(),
            Key::ArrowDown => "Down".into(),
            Key::Home => "Home".into(),
            Key::End => "End".into(),
            Key::PageUp => "PageUp".into(),
            Key::PageDown => "PageDown".into(),
            Key::Backtick => "`".into(),
        }
    }

    /// Pretty glyph form used in menus and tooltips.
    pub fn display_pretty(&self) -> String {
        match self {
            Key::Char(c) => c.to_ascii_uppercase().to_string(),
            Key::F(n) => format!("F{n}"),
            Key::Enter => "↵".into(),
            Key::Esc => "Esc".into(),
            Key::Tab => "⇥".into(),
            Key::Backspace => "⌫".into(),
            Key::Delete => "⌦".into(),
            Key::Space => "Space".into(),
            Key::ArrowLeft => "←".into(),
            Key::ArrowRight => "→".into(),
            Key::ArrowUp => "↑".into(),
            Key::ArrowDown => "↓".into(),
            Key::Home => "Home".into(),
            Key::End => "End".into(),
            Key::PageUp => "PgUp".into(),
            Key::PageDown => "PgDn".into(),
            Key::Backtick => "`".into(),
        }
    }
}

/// A keyboard chord: modifiers + key.
///
/// `primary` is the platform's leader modifier (Cmd on macOS, Ctrl elsewhere).
/// Host adapters fold native modifier state into this field at event time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Chord {
    pub primary: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: Key,
}

impl Chord {
    /// A bare key, no modifiers.
    pub const fn bare(key: Key) -> Self {
        Self {
            primary: false,
            shift: false,
            alt: false,
            key,
        }
    }

    /// `primary + key` (Cmd/Ctrl + key).
    pub const fn primary(key: Key) -> Self {
        Self {
            primary: true,
            shift: false,
            alt: false,
            key,
        }
    }

    /// `primary + shift + key`.
    pub const fn primary_shift(key: Key) -> Self {
        Self {
            primary: true,
            shift: true,
            alt: false,
            key,
        }
    }

    /// `shift + key`.
    pub const fn shift(key: Key) -> Self {
        Self {
            primary: false,
            shift: true,
            alt: false,
            key,
        }
    }

    /// `alt + key`.
    pub const fn alt(key: Key) -> Self {
        Self {
            primary: false,
            shift: false,
            alt: true,
            key,
        }
    }

    /// Parse a chord string. Accepts `+`-separated tokens. Modifier aliases:
    /// `Cmd` / `Ctrl` / `Control` / `Mod` / `Primary` / `Meta` / `Super` all
    /// set `primary`; `Shift`, `Alt` / `Option` / `Opt` are self-explanatory.
    pub fn parse(s: &str) -> Result<Self, ChordParseError> {
        let mut primary = false;
        let mut shift = false;
        let mut alt = false;
        let mut key: Option<Key> = None;
        for raw in s.split('+').map(str::trim) {
            if raw.is_empty() {
                return Err(ChordParseError::EmptyToken(s.to_string()));
            }
            let lower = raw.to_ascii_lowercase();
            match lower.as_str() {
                "cmd" | "ctrl" | "control" | "mod" | "primary" | "meta" | "super" => {
                    primary = true;
                }
                "shift" => shift = true,
                "alt" | "option" | "opt" => alt = true,
                _ => {
                    if key.is_some() {
                        return Err(ChordParseError::MultipleKeys(s.to_string()));
                    }
                    key = Some(Key::parse(raw)?);
                }
            }
        }
        let key = key.ok_or_else(|| ChordParseError::NoKey(s.to_string()))?;
        Ok(Chord {
            primary,
            shift,
            alt,
            key,
        })
    }

    /// Canonical round-trippable string: `"Cmd+Shift+F"`, `"Esc"`, `"F10"`.
    pub fn to_canonical(&self) -> String {
        let mut out = String::new();
        if self.primary {
            out.push_str("Cmd+");
        }
        if self.shift {
            out.push_str("Shift+");
        }
        if self.alt {
            out.push_str("Alt+");
        }
        out.push_str(&self.key.to_canonical());
        out
    }

    /// macOS display form with glyphs: `"⌥⇧⌘F"`.
    pub fn display_mac(&self) -> String {
        let mut out = String::new();
        if self.alt {
            out.push('⌥');
        }
        if self.shift {
            out.push('⇧');
        }
        if self.primary {
            out.push('⌘');
        }
        out.push_str(&self.key.display_pretty());
        out
    }

    /// Windows/Linux display form: `"Ctrl+Shift+Alt+F"`.
    pub fn display_pc(&self) -> String {
        let mut out = String::new();
        if self.primary {
            out.push_str("Ctrl+");
        }
        if self.shift {
            out.push_str("Shift+");
        }
        if self.alt {
            out.push_str("Alt+");
        }
        out.push_str(&self.key.display_pretty());
        out
    }
}

impl std::fmt::Display for Chord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_canonical())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ChordParseError {
    #[error("no key in chord: {0:?}")]
    NoKey(String),
    #[error("multiple keys in chord: {0:?}")]
    MultipleKeys(String),
    #[error("unknown key: {0:?}")]
    UnknownKey(String),
    #[error("empty token in chord: {0:?}")]
    EmptyToken(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple() {
        assert_eq!(
            Chord::parse("Cmd+S").unwrap(),
            Chord::primary(Key::Char('s'))
        );
        assert_eq!(Chord::parse("Esc").unwrap(), Chord::bare(Key::Esc));
        assert_eq!(Chord::parse("F10").unwrap(), Chord::bare(Key::F(10)));
    }

    #[test]
    fn parse_mod_aliases() {
        let target = Chord::primary(Key::Char('k'));
        for alias in ["Cmd+K", "Ctrl+K", "Mod+K", "Primary+K", "Meta+K", "Super+K"] {
            assert_eq!(Chord::parse(alias).unwrap(), target, "alias {alias}");
        }
    }

    #[test]
    fn parse_multi_mod() {
        assert_eq!(
            Chord::parse("Cmd+Shift+Z").unwrap(),
            Chord::primary_shift(Key::Char('z')),
        );
        assert_eq!(
            Chord::parse("Shift+Alt+Cmd+P").unwrap(),
            Chord {
                primary: true,
                shift: true,
                alt: true,
                key: Key::Char('p'),
            },
        );
    }

    #[test]
    fn parse_case_insensitive_key() {
        // Shift is encoded in the modifier, not by uppercase — so "S" and "s"
        // both produce the lowercase char.
        assert_eq!(
            Chord::parse("Cmd+S").unwrap().key,
            Chord::parse("Cmd+s").unwrap().key,
        );
    }

    #[test]
    fn round_trip_canonical() {
        for src in &[
            "Cmd+S",
            "Cmd+Shift+Z",
            "Shift+Alt+Cmd+P",
            "Esc",
            "F10",
            "Ctrl+K",
            "Alt+Enter",
            "Shift+Tab",
            "`",
        ] {
            let parsed = Chord::parse(src).unwrap();
            let canonical = parsed.to_canonical();
            let reparsed = Chord::parse(&canonical).unwrap();
            assert_eq!(parsed, reparsed, "round-trip for {src} via {canonical}");
        }
    }

    #[test]
    fn display_mac_and_pc() {
        let c = Chord::primary_shift(Key::Char('f'));
        assert_eq!(c.display_mac(), "⇧⌘F");
        assert_eq!(c.display_pc(), "Ctrl+Shift+F");
    }

    #[test]
    fn display_named_keys() {
        assert_eq!(Chord::bare(Key::Esc).display_mac(), "Esc");
        assert_eq!(Chord::bare(Key::Enter).display_mac(), "↵");
        assert_eq!(Chord::primary(Key::ArrowLeft).display_mac(), "⌘←");
    }

    #[test]
    fn parse_errors() {
        assert!(matches!(
            Chord::parse("").unwrap_err(),
            ChordParseError::EmptyToken(_) | ChordParseError::NoKey(_)
        ));
        assert!(matches!(
            Chord::parse("Cmd").unwrap_err(),
            ChordParseError::NoKey(_)
        ));
        assert!(matches!(
            Chord::parse("A+B").unwrap_err(),
            ChordParseError::MultipleKeys(_)
        ));
        assert!(matches!(
            Chord::parse("Cmd+Nope").unwrap_err(),
            ChordParseError::UnknownKey(_)
        ));
    }

    #[test]
    fn serde_round_trip() {
        let c = Chord::primary_shift(Key::Char('f'));
        let json = serde_json::to_string(&c).unwrap();
        let decoded: Chord = serde_json::from_str(&json).unwrap();
        assert_eq!(c, decoded);
    }

    #[test]
    fn f_key_range() {
        assert_eq!(Chord::parse("F1").unwrap().key, Key::F(1));
        assert_eq!(Chord::parse("F24").unwrap().key, Key::F(24));
        assert!(Chord::parse("F25").is_err());
        assert!(Chord::parse("F0").is_err());
    }
}
