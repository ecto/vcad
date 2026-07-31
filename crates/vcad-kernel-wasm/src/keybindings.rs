//! WASM bindings for the shared keybinding registry.
//!
//! Exposes [`WasmKeybindings`] — a thin wrapper around
//! [`vcad_app::KeybindingRegistry`] so the web app can look up command IDs
//! from a normalized [`vcad_app::Chord`] without reimplementing resolution.
//!
//! The TS side normalizes each `KeyboardEvent` into a `Chord`, serializes it
//! to JSON, calls [`WasmKeybindings::resolve`], and dispatches the returned
//! command id through its existing action map (`useAppCommands`).

use serde::Serialize;
use vcad_app::{AppMode, Chord, KeybindingRegistry, WhenContext};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct WasmKeybindings {
    inner: KeybindingRegistry,
}

impl Default for WasmKeybindings {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl WasmKeybindings {
    /// Construct a fresh registry with all default bindings.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            inner: KeybindingRegistry::new(),
        }
    }

    /// Resolve a chord to a command id.
    ///
    /// - `chord_json` is the JSON-serialized [`Chord`] produced by the TS
    ///   adapter (`chord.ts` normalizes `KeyboardEvent` → `Chord`).
    /// - `mode_name` is one of `"Normal" | "Sketch" | "Assembly" | ...`
    ///   (see [`AppMode`]).
    /// - `ctx_bits` is the packed u32 from [`WhenContext::bits`].
    ///
    /// Returns the command id on match, or `None` — the TS side checks for
    /// `null` and falls through if nothing binds.
    pub fn resolve(&self, chord_json: &str, mode_name: &str, ctx_bits: u32) -> Option<String> {
        let chord: Chord = serde_json::from_str(chord_json).ok()?;
        let mode = AppMode::parse(mode_name).unwrap_or_default();
        let ctx = WhenContext::from_bits(ctx_bits);
        self.inner.resolve(&chord, mode, ctx).map(|s| s.to_string())
    }

    /// Returns a JSON array describing every registered command. The TS UI
    /// (command palette, keyboard prefs) reads this once at startup.
    ///
    /// Each entry is a `CommandView` — a flattened, owned projection of
    /// `Command` that serde can serialize (the source struct uses `&'static
    /// str` and a non-serializable `ModeScope` enum).
    #[wasm_bindgen(js_name = commandsJson)]
    pub fn commands_json(&self) -> String {
        let out: Vec<CommandView> = self
            .inner
            .commands()
            .iter()
            .map(|cmd| {
                let effective = self.inner.chord_for(cmd.id);
                CommandView {
                    id: cmd.id,
                    label: cmd.label(),
                    keywords: cmd.keywords,
                    icon: cmd.icon,
                    category: cmd.category.as_ref().map(|c| match c {
                        vcad_app::CommandCategory::File => "file",
                        vcad_app::CommandCategory::Edit => "edit",
                        vcad_app::CommandCategory::View => "view",
                        vcad_app::CommandCategory::Create => "create",
                        vcad_app::CommandCategory::Modify => "modify",
                        vcad_app::CommandCategory::Assembly => "assembly",
                        vcad_app::CommandCategory::Tools => "tools",
                        vcad_app::CommandCategory::Help => "help",
                    }),
                    default_chord: cmd.default_chord,
                    effective_chord: effective,
                    when: cmd.when,
                    mode_scope: mode_scope_view(cmd.mode_scope),
                    target: match cmd.target {
                        vcad_app::Target::Kernel => "kernel",
                        vcad_app::Target::Host => "host",
                    },
                }
            })
            .collect();
        serde_json::to_string(&out).unwrap_or_else(|_| "[]".to_string())
    }

    /// Rebind a command. Pass a JSON-encoded chord to set, or `None` to
    /// clear (disabling the binding).
    #[wasm_bindgen(js_name = setBinding)]
    pub fn set_binding(&mut self, id: &str, chord_json: Option<String>) {
        let chord = chord_json.and_then(|s| serde_json::from_str::<Chord>(&s).ok());
        // Registry's set_binding uses None to mean "clear", Some(c) to set.
        // We match JSON parse failures to a clear, which keeps the
        // invariant that malformed input never crashes the app.
        self.inner.set_binding(id, chord);
    }

    /// Return the effective chord (user override or default) for a command
    /// id, or `None` if disabled / unbound.
    #[wasm_bindgen(js_name = chordFor)]
    pub fn chord_for(&self, id: &str) -> Option<String> {
        self.inner
            .chord_for(id)
            .and_then(|c| serde_json::to_string(&c).ok())
    }

    /// Clear all user overrides, restoring default bindings.
    #[wasm_bindgen(js_name = resetAll)]
    pub fn reset_all(&mut self) {
        self.inner.reset_all();
    }

    /// Serialize user overrides for persistence (e.g. localStorage).
    #[wasm_bindgen(js_name = saveOverrides)]
    pub fn save_overrides(&self) -> String {
        self.inner.save_overrides()
    }

    /// Load overrides previously returned by [`Self::save_overrides`]. Malformed
    /// entries are skipped — the caller never sees a parse failure for
    /// stale config.
    #[wasm_bindgen(js_name = loadOverrides)]
    pub fn load_overrides(&mut self, json: &str) -> bool {
        self.inner.load_overrides(json).is_ok()
    }

    /// Report binding conflicts in the given mode: pairs of commands that
    /// share the same chord. Returns a JSON array for the prefs UI to
    /// highlight.
    #[wasm_bindgen(js_name = conflictsJson)]
    pub fn conflicts_json(&self, mode_name: &str) -> String {
        let mode = AppMode::parse(mode_name).unwrap_or_default();
        let conflicts: Vec<ConflictView> = self
            .inner
            .conflicts(mode)
            .into_iter()
            .map(|(chord, ids)| ConflictView { chord, ids })
            .collect();
        serde_json::to_string(&conflicts).unwrap_or_else(|_| "[]".to_string())
    }
}

/// JSON projection of [`vcad_app::Command`] — owned strings so serde can
/// serialize, and an `effective_chord` field resolved through any user
/// override so the TS side doesn't have to merge them itself.
#[derive(Serialize)]
struct CommandView {
    id: &'static str,
    label: &'static str,
    keywords: &'static [&'static str],
    icon: &'static str,
    category: Option<&'static str>,
    default_chord: Option<Chord>,
    effective_chord: Option<Chord>,
    when: Option<&'static str>,
    mode_scope: ModeScopeView,
    target: &'static str,
}

#[derive(Serialize)]
#[serde(tag = "kind", content = "modes")]
enum ModeScopeView {
    Global,
    Mode(&'static str),
    Modes(Vec<&'static str>),
}

fn mode_scope_view(scope: vcad_app::ModeScope) -> ModeScopeView {
    match scope {
        vcad_app::ModeScope::Global => ModeScopeView::Global,
        vcad_app::ModeScope::Mode(m) => ModeScopeView::Mode(m.name()),
        vcad_app::ModeScope::Modes(ms) => {
            ModeScopeView::Modes(ms.iter().map(|m| m.name()).collect())
        }
    }
}

#[derive(Serialize)]
struct ConflictView {
    chord: Chord,
    ids: Vec<&'static str>,
}
