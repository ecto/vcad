//! The keybinding registry — the single source of truth for "which chord
//! triggers which command, in which mode, under which context".
//!
//! Hosts call [`KeybindingRegistry::resolve`] with a normalized chord, the
//! current [`AppMode`], and a [`WhenContext`] flag set; the registry returns
//! `Option<&'static str>` — the command id to dispatch — or `None` if no
//! binding applies. The host then looks that id up in its own action map
//! (`useAppCommands` on web, `App::process_command` on the TUI).
//!
//! User overrides live in a `HashMap<command_id, Chord>` that shadows the
//! defaults. Clearing a binding is represented by inserting `None` via
//! [`KeybindingRegistry::set_binding`], which puts the id into the disabled
//! set.

use std::collections::{HashMap, HashSet};

use crate::commands::{all_commands, Command};
use crate::context::{WhenContext, WhenExpr, WhenParseError};
use crate::keybinding::{Chord, ChordParseError};
use crate::mode::AppMode;

/// Runtime registry combining static command metadata, parsed when-clauses,
/// and per-user overrides.
pub struct KeybindingRegistry {
    commands: &'static [Command],
    /// Pre-parsed when-expressions by command id. Present only for commands
    /// that declared a `when` string.
    when_exprs: HashMap<&'static str, WhenExpr>,
    /// User's reassigned chords, keyed by command id. An entry here wins
    /// over `Command.default_chord`.
    user_overrides: HashMap<String, Chord>,
    /// Commands whose binding has been explicitly cleared by the user.
    disabled: HashSet<String>,
}

impl KeybindingRegistry {
    /// Build a registry from the static command table. Panics if any
    /// command's `when` string fails to parse — that's a static invariant
    /// so it should be caught at build time, not at runtime.
    pub fn new() -> Self {
        let commands = all_commands();
        let mut when_exprs: HashMap<&'static str, WhenExpr> = HashMap::new();
        for cmd in commands {
            if let Some(src) = cmd.when {
                match WhenExpr::parse(src) {
                    Ok(e) => {
                        when_exprs.insert(cmd.id, e);
                    }
                    Err(err) => {
                        panic!("invalid when clause on command {}: {}", cmd.id, err)
                    }
                }
            }
        }
        Self {
            commands,
            when_exprs,
            user_overrides: HashMap::new(),
            disabled: HashSet::new(),
        }
    }

    /// The full static command list.
    pub fn commands(&self) -> &'static [Command] {
        self.commands
    }

    /// The effective chord for a command: the user override, or the default.
    /// `None` if the user has disabled the binding or no default exists.
    pub fn chord_for(&self, id: &str) -> Option<Chord> {
        if self.disabled.contains(id) {
            return None;
        }
        if let Some(c) = self.user_overrides.get(id) {
            return Some(*c);
        }
        self.commands
            .iter()
            .find(|c| c.id == id)
            .and_then(|c| c.default_chord)
    }

    /// Set a user override. `Some(chord)` rebinds, `None` clears the binding.
    pub fn set_binding(&mut self, id: &str, chord: Option<Chord>) {
        match chord {
            Some(c) => {
                self.user_overrides.insert(id.to_string(), c);
                self.disabled.remove(id);
            }
            None => {
                self.user_overrides.remove(id);
                self.disabled.insert(id.to_string());
            }
        }
    }

    /// Clear all overrides, restoring defaults.
    pub fn reset_all(&mut self) {
        self.user_overrides.clear();
        self.disabled.clear();
    }

    /// Resolve a chord to a command id. Returns `None` if no command's
    /// effective binding matches, or if all matching commands are gated out
    /// by their `when` clause / mode scope.
    ///
    /// Precedence when multiple commands share a chord:
    /// 1. Most specific mode scope wins (`Mode(X)` > `Modes(&[..])` > `Global`).
    /// 2. A `when`-gated command that evaluates true beats a `when`-less one
    ///    with the same specificity, so ungated commands act as fallbacks.
    /// 3. Declaration order (first in `COMMANDS`) tie-breaks within a rank.
    pub fn resolve(&self, chord: &Chord, mode: AppMode, ctx: WhenContext) -> Option<&'static str> {
        let mut best: Option<(&'static str, i32)> = None;
        for cmd in self.commands {
            if !cmd.mode_scope.matches(mode) {
                continue;
            }
            let Some(effective) = self.chord_for(cmd.id) else {
                continue;
            };
            if effective != *chord {
                continue;
            }
            if let Some(expr) = self.when_exprs.get(cmd.id) {
                if !expr.eval(ctx) {
                    continue;
                }
            }
            // Rank = specificity×2 + has_when (gated beats fallback).
            let rank = cmd.mode_scope.specificity() as i32 * 2
                + self.when_exprs.contains_key(cmd.id) as i32;
            match best {
                None => best = Some((cmd.id, rank)),
                Some((_, br)) if rank > br => best = Some((cmd.id, rank)),
                _ => {}
            }
        }
        best.map(|(id, _)| id)
    }

    /// Find pairs of commands that bind to the same chord in the same mode.
    /// Used by the prefs UI to surface conflicts.
    pub fn conflicts(&self, mode: AppMode) -> Vec<(Chord, Vec<&'static str>)> {
        let mut by_chord: HashMap<Chord, Vec<&'static str>> = HashMap::new();
        for cmd in self.commands {
            if !cmd.mode_scope.matches(mode) {
                continue;
            }
            if let Some(chord) = self.chord_for(cmd.id) {
                by_chord.entry(chord).or_default().push(cmd.id);
            }
        }
        by_chord.into_iter().filter(|(_, v)| v.len() > 1).collect()
    }

    /// Serialize user overrides to a JSON string suitable for localStorage /
    /// a config file. Format: `{"overrides": {"save": "Cmd+S"}, "disabled": ["redo"]}`.
    pub fn save_overrides(&self) -> String {
        let overrides: HashMap<&String, String> = self
            .user_overrides
            .iter()
            .map(|(k, v)| (k, v.to_canonical()))
            .collect();
        let disabled: Vec<&String> = self.disabled.iter().collect();
        serde_json::json!({
            "overrides": overrides,
            "disabled": disabled,
        })
        .to_string()
    }

    /// Load overrides previously saved with [`save_overrides`]. Unknown
    /// command ids and malformed chords are skipped — the caller shouldn't
    /// crash on a stale config file.
    pub fn load_overrides(&mut self, json: &str) -> Result<(), LoadError> {
        let v: serde_json::Value = serde_json::from_str(json).map_err(LoadError::Json)?;
        let Some(obj) = v.as_object() else {
            return Err(LoadError::Shape("expected top-level object"));
        };
        if let Some(overrides) = obj.get("overrides").and_then(|v| v.as_object()) {
            for (id, chord) in overrides {
                let Some(s) = chord.as_str() else { continue };
                let Ok(chord) = Chord::parse(s) else { continue };
                if self.commands.iter().any(|c| c.id == id) {
                    self.user_overrides.insert(id.clone(), chord);
                }
            }
        }
        if let Some(disabled) = obj.get("disabled").and_then(|v| v.as_array()) {
            for id in disabled {
                let Some(s) = id.as_str() else { continue };
                if self.commands.iter().any(|c| c.id == s) {
                    self.disabled.insert(s.to_string());
                }
            }
        }
        Ok(())
    }
}

impl Default for KeybindingRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("json error: {0}")]
    Json(serde_json::Error),
    #[error("bad shape: {0}")]
    Shape(&'static str),
    #[error("chord parse error: {0}")]
    Chord(#[from] ChordParseError),
    #[error("when-expr error: {0}")]
    When(#[from] WhenParseError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keybinding::Key;

    #[test]
    fn resolves_from_defaults() {
        let reg = KeybindingRegistry::new();
        // `save` has default Cmd+S.
        let id = reg.resolve(
            &Chord::primary(Key::Char('s')),
            AppMode::Normal,
            WhenContext::NONE,
        );
        assert_eq!(id, Some("save"));
    }

    #[test]
    fn user_override_wins_over_default() {
        let mut reg = KeybindingRegistry::new();
        reg.set_binding("save", Some(Chord::primary(Key::Char('w'))));

        // Old chord no longer resolves (to save).
        let old = reg.resolve(
            &Chord::primary(Key::Char('s')),
            AppMode::Normal,
            WhenContext::NONE,
        );
        assert_ne!(old, Some("save"));

        // New chord does.
        let new = reg.resolve(
            &Chord::primary(Key::Char('w')),
            AppMode::Normal,
            WhenContext::NONE,
        );
        assert_eq!(new, Some("save"));
    }

    #[test]
    fn disabled_binding_does_not_resolve() {
        let mut reg = KeybindingRegistry::new();
        reg.set_binding("save", None);
        let id = reg.resolve(
            &Chord::primary(Key::Char('s')),
            AppMode::Normal,
            WhenContext::NONE,
        );
        assert_ne!(id, Some("save"));
    }

    #[test]
    fn save_and_load_overrides_round_trip() {
        let mut reg = KeybindingRegistry::new();
        reg.set_binding("save", Some(Chord::primary(Key::Char('w'))));
        reg.set_binding("redo", None);
        let json = reg.save_overrides();

        let mut fresh = KeybindingRegistry::new();
        fresh.load_overrides(&json).unwrap();
        assert_eq!(
            fresh.chord_for("save"),
            Some(Chord::primary(Key::Char('w'))),
        );
        assert_eq!(fresh.chord_for("redo"), None);
    }

    #[test]
    fn load_ignores_unknown_commands() {
        let mut reg = KeybindingRegistry::new();
        let json = r#"{"overrides":{"not_a_real_cmd":"Cmd+Z"},"disabled":["also_fake"]}"#;
        reg.load_overrides(json).unwrap();
        assert!(!reg.user_overrides.contains_key("not_a_real_cmd"));
        assert!(!reg.disabled.contains("also_fake"));
    }

    #[test]
    fn load_ignores_malformed_chords() {
        let mut reg = KeybindingRegistry::new();
        let json = r#"{"overrides":{"save":"NotAChord+???"}}"#;
        reg.load_overrides(json).unwrap();
        // `save` still has its default.
        assert_eq!(reg.chord_for("save"), Some(Chord::primary(Key::Char('s'))),);
    }

    #[test]
    fn every_command_when_clause_parses() {
        // Catches a malformed `when` string at CI time instead of at app
        // startup (where KeybindingRegistry::new panics).
        for cmd in all_commands() {
            if let Some(src) = cmd.when {
                WhenExpr::parse(src)
                    .unwrap_or_else(|e| panic!("when clause on {} failed to parse: {e}", cmd.id));
            }
        }
    }
}
