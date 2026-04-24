//! Command registry with metadata.
//!
//! The `Command` struct is the static metadata source: id, display strings,
//! category, default keybinding, `when` gate, mode scope. Hosts (web, TUI)
//! each maintain their own action map keyed by command `id` — actions never
//! cross the language boundary.

use serde::{Deserialize, Serialize};

use crate::keybinding::{Chord, Key};
use crate::mode::{ModeScope, Target};

/// Toolbar tab categories — maps each command to one of the TUI's bottom
/// toolbar rows. Display-only; unrelated to `CommandCategory` (which groups
/// by menu bar section).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolbarTab {
    Chat,
    Create,
    Transform,
    Combine,
    Modify,
    Assembly,
    Simulate,
    Export,
}

/// Menu category — matches the Borland-style menu bar sections shared by
/// web and TUI (File / Edit / View / Create / Modify / Assembly / Tools /
/// Help). `None` means the command is palette-only (not shown in menus).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommandCategory {
    File,
    Edit,
    View,
    Create,
    Modify,
    Assembly,
    Tools,
    Help,
}

/// Command metadata — everything the registry needs to render, resolve, and
/// describe a command.
#[derive(Debug, Clone)]
pub struct Command {
    pub id: &'static str,
    pub label_key: &'static str,
    pub keywords: &'static [&'static str],
    pub icon: &'static str,
    /// Legacy display-only shortcut string kept for backward compatibility
    /// with the existing TUI palette rendering. The registry's authoritative
    /// chord lives in [`Command::default_chord`].
    pub shortcut: Option<&'static str>,
    pub tab: ToolbarTab,
    /// Menu grouping (None = palette-only, never shown in menus).
    pub category: Option<CommandCategory>,
    /// Default key binding. User overrides live in the [`crate::registry::KeybindingRegistry`].
    pub default_chord: Option<Chord>,
    /// Optional `when`-expression source; parsed at registry construction.
    /// See [`crate::context::WhenExpr::parse`] for the grammar.
    pub when: Option<&'static str>,
    /// Which modes this command's binding fires in.
    pub mode_scope: ModeScope,
    /// Advisory hint: kernel-owned action or host-owned action.
    pub target: Target,
}

impl Command {
    /// Resolved display label for the current locale.
    pub fn label(&self) -> &str {
        vcad_i18n::t(self.label_key)
    }
}

/// Default values for new metadata fields — spread with `..CMD_DEFAULTS` so
/// individual entries only declare the fields they care about.
const CMD_DEFAULTS: Command = Command {
    id: "",
    label_key: "",
    keywords: &[],
    icon: "",
    shortcut: None,
    tab: ToolbarTab::Create,
    category: None,
    default_chord: None,
    when: None,
    mode_scope: ModeScope::Global,
    target: Target::Host,
};

/// All registered commands.
pub fn all_commands() -> &'static [Command] {
    COMMANDS
}

/// Search commands by query (case-insensitive contains match on id, label,
/// keywords).
pub fn find_commands(query: &str) -> Vec<&'static Command> {
    if query.is_empty() {
        return COMMANDS.iter().collect();
    }
    let q = query.to_lowercase();
    COMMANDS
        .iter()
        .filter(|cmd| {
            cmd.id.to_lowercase().contains(&q)
                || cmd.label().to_lowercase().contains(&q)
                || cmd.keywords.iter().any(|k| k.to_lowercase().contains(&q))
        })
        .collect()
}

static COMMANDS: &[Command] = &[
    // ── Create ───────────────────────────────────────────────────────
    Command {
        id: "cube",
        label_key: "cmd.cube.label",
        keywords: &["box", "rectangular", "prism"],
        icon: "\u{25A0}",
        tab: ToolbarTab::Create,
        category: Some(CommandCategory::Create),
        target: Target::Kernel,
        ..CMD_DEFAULTS
    },
    Command {
        id: "cylinder",
        label_key: "cmd.cylinder.label",
        keywords: &["cyl", "tube", "pipe"],
        icon: "\u{25CB}",
        tab: ToolbarTab::Create,
        category: Some(CommandCategory::Create),
        target: Target::Kernel,
        ..CMD_DEFAULTS
    },
    Command {
        id: "sphere",
        label_key: "cmd.sphere.label",
        keywords: &["ball", "globe"],
        icon: "\u{25CF}",
        tab: ToolbarTab::Create,
        category: Some(CommandCategory::Create),
        target: Target::Kernel,
        ..CMD_DEFAULTS
    },
    Command {
        id: "cone",
        label_key: "cmd.cone.label",
        keywords: &["conical", "taper"],
        icon: "\u{25B2}",
        tab: ToolbarTab::Create,
        category: Some(CommandCategory::Create),
        target: Target::Kernel,
        ..CMD_DEFAULTS
    },
    // ── Transform ────────────────────────────────────────────────────
    Command {
        id: "translate",
        label_key: "cmd.translate.label",
        keywords: &["translate", "position", "offset"],
        shortcut: Some("G"),
        icon: "\u{2194}",
        tab: ToolbarTab::Transform,
        category: Some(CommandCategory::Edit),
        default_chord: Some(Chord::bare(Key::Char('g'))),
        when: Some("has_selection && !input_focused"),
        target: Target::Kernel,
        ..CMD_DEFAULTS
    },
    Command {
        id: "rotate",
        label_key: "cmd.rotate.label",
        keywords: &["spin", "turn", "orientation"],
        shortcut: Some("R"),
        icon: "\u{21BB}",
        tab: ToolbarTab::Transform,
        category: Some(CommandCategory::Edit),
        default_chord: Some(Chord::bare(Key::Char('r'))),
        when: Some("has_selection && !input_focused"),
        target: Target::Kernel,
        ..CMD_DEFAULTS
    },
    Command {
        id: "scale",
        label_key: "cmd.scale.label",
        keywords: &["resize", "size"],
        shortcut: Some("Shift+S"),
        icon: "\u{2922}",
        tab: ToolbarTab::Transform,
        category: Some(CommandCategory::Edit),
        default_chord: Some(Chord::shift(Key::Char('s'))),
        when: Some("has_selection && !input_focused"),
        target: Target::Kernel,
        ..CMD_DEFAULTS
    },
    Command {
        id: "mirror",
        label_key: "cmd.mirror.label",
        keywords: &["flip", "reflect", "symmetry"],
        icon: "\u{2016}",
        tab: ToolbarTab::Transform,
        category: Some(CommandCategory::Modify),
        when: Some("one_part"),
        target: Target::Kernel,
        ..CMD_DEFAULTS
    },
    // ── Combine (booleans) ───────────────────────────────────────────
    Command {
        id: "union",
        label_key: "cmd.union.label",
        keywords: &["add", "join", "merge", "combine"],
        shortcut: Some("Cmd+Shift+U"),
        icon: "\u{222A}",
        tab: ToolbarTab::Combine,
        category: Some(CommandCategory::Modify),
        default_chord: Some(Chord::primary_shift(Key::Char('u'))),
        when: Some("two_selected"),
        target: Target::Kernel,
        ..CMD_DEFAULTS
    },
    Command {
        id: "difference",
        label_key: "cmd.difference.label",
        keywords: &["subtract", "cut", "remove"],
        shortcut: Some("Cmd+Shift+D"),
        icon: "\u{2216}",
        tab: ToolbarTab::Combine,
        category: Some(CommandCategory::Modify),
        default_chord: Some(Chord::primary_shift(Key::Char('d'))),
        when: Some("two_selected"),
        target: Target::Kernel,
        ..CMD_DEFAULTS
    },
    Command {
        id: "intersection",
        label_key: "cmd.intersection.label",
        keywords: &["intersect", "common", "overlap"],
        shortcut: Some("Cmd+Shift+I"),
        icon: "\u{2229}",
        tab: ToolbarTab::Combine,
        category: Some(CommandCategory::Modify),
        default_chord: Some(Chord::primary_shift(Key::Char('i'))),
        when: Some("two_selected"),
        target: Target::Kernel,
        ..CMD_DEFAULTS
    },
    // ── Modify ───────────────────────────────────────────────────────
    Command {
        id: "fillet",
        label_key: "cmd.fillet.label",
        keywords: &["round", "radius", "smooth"],
        icon: "\u{25E0}",
        tab: ToolbarTab::Modify,
        category: Some(CommandCategory::Modify),
        when: Some("one_part"),
        target: Target::Kernel,
        ..CMD_DEFAULTS
    },
    Command {
        id: "chamfer",
        label_key: "cmd.chamfer.label",
        keywords: &["bevel", "edge"],
        icon: "\u{25FA}",
        tab: ToolbarTab::Modify,
        category: Some(CommandCategory::Modify),
        when: Some("one_part"),
        target: Target::Kernel,
        ..CMD_DEFAULTS
    },
    Command {
        id: "shell",
        label_key: "cmd.shell.label",
        keywords: &["hollow", "thin", "wall"],
        icon: "\u{25A1}",
        tab: ToolbarTab::Modify,
        category: Some(CommandCategory::Modify),
        when: Some("one_part"),
        target: Target::Kernel,
        ..CMD_DEFAULTS
    },
    Command {
        id: "linear_pattern",
        label_key: "cmd.linear_pattern.label",
        keywords: &["array", "repeat", "linear"],
        icon: "\u{2026}",
        tab: ToolbarTab::Modify,
        category: Some(CommandCategory::Modify),
        when: Some("one_part"),
        target: Target::Kernel,
        ..CMD_DEFAULTS
    },
    Command {
        id: "circular_pattern",
        label_key: "cmd.circular_pattern.label",
        keywords: &["radial", "polar", "array"],
        icon: "\u{25CE}",
        tab: ToolbarTab::Modify,
        category: Some(CommandCategory::Modify),
        when: Some("one_part"),
        target: Target::Kernel,
        ..CMD_DEFAULTS
    },
    // ── Edit ─────────────────────────────────────────────────────────
    Command {
        id: "delete",
        label_key: "cmd.delete.label",
        keywords: &["remove", "erase", "del"],
        shortcut: Some("Delete"),
        icon: "\u{2715}",
        tab: ToolbarTab::Create,
        category: Some(CommandCategory::Edit),
        default_chord: Some(Chord::bare(Key::Delete)),
        when: Some("has_selection && !input_focused"),
        target: Target::Kernel,
        ..CMD_DEFAULTS
    },
    Command {
        id: "undo",
        label_key: "cmd.undo.label",
        keywords: &["back", "revert"],
        shortcut: Some("Cmd+Z"),
        icon: "\u{21B6}",
        tab: ToolbarTab::Create,
        category: Some(CommandCategory::Edit),
        default_chord: Some(Chord::primary(Key::Char('z'))),
        when: Some("can_undo"),
        target: Target::Kernel,
        ..CMD_DEFAULTS
    },
    Command {
        id: "redo",
        label_key: "cmd.redo.label",
        keywords: &["forward"],
        shortcut: Some("Cmd+Shift+Z"),
        icon: "\u{21B7}",
        tab: ToolbarTab::Create,
        category: Some(CommandCategory::Edit),
        default_chord: Some(Chord::primary_shift(Key::Char('z'))),
        when: Some("can_redo"),
        target: Target::Kernel,
        ..CMD_DEFAULTS
    },
    Command {
        id: "copy",
        label_key: "cmd.copy.label",
        keywords: &["clipboard", "yank"],
        shortcut: Some("Cmd+C"),
        icon: "\u{29C9}",
        tab: ToolbarTab::Create,
        category: Some(CommandCategory::Edit),
        default_chord: Some(Chord::primary(Key::Char('c'))),
        when: Some("has_selection && !input_focused"),
        target: Target::Kernel,
        ..CMD_DEFAULTS
    },
    Command {
        id: "paste",
        label_key: "cmd.paste.label",
        keywords: &["clipboard"],
        shortcut: Some("Cmd+V"),
        icon: "\u{2398}",
        tab: ToolbarTab::Create,
        category: Some(CommandCategory::Edit),
        default_chord: Some(Chord::primary(Key::Char('v'))),
        when: Some("!input_focused"),
        target: Target::Kernel,
        ..CMD_DEFAULTS
    },
    Command {
        id: "duplicate",
        label_key: "cmd.duplicate.label",
        keywords: &["clone", "copy"],
        shortcut: Some("Cmd+D"),
        icon: "\u{29C9}",
        tab: ToolbarTab::Create,
        category: Some(CommandCategory::Edit),
        default_chord: Some(Chord::primary(Key::Char('d'))),
        when: Some("has_selection && !input_focused"),
        target: Target::Kernel,
        ..CMD_DEFAULTS
    },
    Command {
        id: "select_all",
        label_key: "cmd.select_all.label",
        keywords: &["all", "everything"],
        shortcut: Some("Cmd+A"),
        icon: "\u{25A3}",
        tab: ToolbarTab::Create,
        category: Some(CommandCategory::Edit),
        default_chord: Some(Chord::primary(Key::Char('a'))),
        when: Some("!input_focused"),
        target: Target::Host,
        ..CMD_DEFAULTS
    },
    Command {
        id: "deselect",
        label_key: "cmd.deselect.label",
        keywords: &["clear", "none"],
        shortcut: Some("Esc"),
        icon: "\u{25A1}",
        tab: ToolbarTab::Create,
        category: Some(CommandCategory::Edit),
        default_chord: Some(Chord::bare(Key::Esc)),
        // Esc is overloaded in other modes (sketch escape, assembly cancel,
        // etc.) — only let the registry steal it in Normal.
        mode_scope: ModeScope::Mode(crate::AppMode::Normal),
        target: Target::Host,
        ..CMD_DEFAULTS
    },
    // ── File ─────────────────────────────────────────────────────────
    Command {
        id: "new",
        label_key: "cmd.new.label",
        keywords: &["empty", "clear", "start"],
        shortcut: Some("Cmd+N"),
        icon: "+",
        tab: ToolbarTab::Export,
        category: Some(CommandCategory::File),
        default_chord: Some(Chord::primary(Key::Char('n'))),
        target: Target::Kernel,
        ..CMD_DEFAULTS
    },
    Command {
        id: "open",
        label_key: "cmd.open.label",
        keywords: &["load", "file", "import"],
        shortcut: Some("Cmd+O"),
        icon: "\u{2198}",
        tab: ToolbarTab::Export,
        category: Some(CommandCategory::File),
        default_chord: Some(Chord::primary(Key::Char('o'))),
        target: Target::Host,
        ..CMD_DEFAULTS
    },
    Command {
        id: "save",
        label_key: "cmd.save.label",
        keywords: &["write", "store"],
        shortcut: Some("Cmd+S"),
        icon: "S",
        tab: ToolbarTab::Export,
        category: Some(CommandCategory::File),
        default_chord: Some(Chord::primary(Key::Char('s'))),
        target: Target::Host,
        ..CMD_DEFAULTS
    },
    Command {
        id: "export_stl",
        label_key: "cmd.export_stl.label",
        keywords: &["stl", "mesh", "3d print"],
        icon: "\u{2197}",
        tab: ToolbarTab::Export,
        category: Some(CommandCategory::File),
        when: Some("has_parts"),
        target: Target::Host,
        ..CMD_DEFAULTS
    },
    Command {
        id: "export_glb",
        label_key: "cmd.export_glb.label",
        keywords: &["glb", "gltf", "mesh", "web"],
        icon: "\u{2197}",
        tab: ToolbarTab::Export,
        category: Some(CommandCategory::File),
        when: Some("has_parts"),
        target: Target::Host,
        ..CMD_DEFAULTS
    },
    Command {
        id: "export_step",
        label_key: "cmd.export_step.label",
        keywords: &["step", "stp", "cad"],
        icon: "\u{2197}",
        tab: ToolbarTab::Export,
        category: Some(CommandCategory::File),
        when: Some("has_parts"),
        target: Target::Host,
        ..CMD_DEFAULTS
    },
    Command {
        id: "quit",
        label_key: "cmd.quit.label",
        keywords: &["exit", "close", "q"],
        shortcut: Some("Cmd+Q"),
        icon: "\u{2715}",
        tab: ToolbarTab::Export,
        category: Some(CommandCategory::File),
        default_chord: Some(Chord::primary(Key::Char('q'))),
        target: Target::Host,
        ..CMD_DEFAULTS
    },
    // ── View ─────────────────────────────────────────────────────────
    Command {
        id: "toggle_sidebar",
        label_key: "cmd.toggle_sidebar.label",
        keywords: &["tree", "panel", "parts", "hide"],
        shortcut: Some("\\"),
        icon: "\u{2630}",
        tab: ToolbarTab::Create,
        category: Some(CommandCategory::View),
        default_chord: Some(Chord::bare(Key::Char('\\'))),
        when: Some("!input_focused"),
        target: Target::Host,
        ..CMD_DEFAULTS
    },
    Command {
        id: "toggle_chat",
        label_key: "cmd.toggle_chat.label",
        keywords: &["ai", "assistant", "panel"],
        shortcut: Some("F6"),
        icon: "\u{2726}",
        tab: ToolbarTab::Chat,
        category: Some(CommandCategory::View),
        default_chord: Some(Chord::bare(Key::F(6))),
        target: Target::Host,
        ..CMD_DEFAULTS
    },
    Command {
        id: "toggle_devtools",
        label_key: "cmd.toggle_devtools.label",
        keywords: &["console", "log", "debug", "devtools"],
        shortcut: Some("`"),
        icon: "\u{25AE}",
        tab: ToolbarTab::Create,
        category: Some(CommandCategory::View),
        default_chord: Some(Chord::bare(Key::Backtick)),
        target: Target::Host,
        ..CMD_DEFAULTS
    },
    Command {
        id: "toggle_wireframe",
        label_key: "cmd.toggle_wireframe.label",
        keywords: &["edges", "mesh", "view"],
        shortcut: Some("X"),
        icon: "\u{25C7}",
        tab: ToolbarTab::Create,
        category: Some(CommandCategory::View),
        default_chord: Some(Chord::bare(Key::Char('x'))),
        when: Some("!input_focused"),
        target: Target::Host,
        ..CMD_DEFAULTS
    },
    Command {
        id: "toggle_grid_snap",
        label_key: "cmd.toggle_grid_snap.label",
        keywords: &["snap", "grid", "align"],
        shortcut: Some("G"),
        icon: "\u{25A6}",
        tab: ToolbarTab::Create,
        category: Some(CommandCategory::View),
        // Note: G collides with the `translate` shortcut above. When a
        // selection exists, translate wins (has `has_selection` gate); when
        // there's no selection, grid snap fires as the fallback.
        default_chord: Some(Chord::bare(Key::Char('g'))),
        when: Some("!has_selection && !input_focused"),
        target: Target::Host,
        ..CMD_DEFAULTS
    },
    Command {
        id: "cycle_theme",
        label_key: "cmd.cycle_theme.label",
        keywords: &["dark", "light", "appearance"],
        icon: "\u{25D0}",
        tab: ToolbarTab::Create,
        category: Some(CommandCategory::View),
        target: Target::Host,
        ..CMD_DEFAULTS
    },
    Command {
        id: "camera_iso",
        label_key: "cmd.camera_iso.label",
        keywords: &["camera", "iso", "3d", "default"],
        shortcut: Some("7"),
        icon: "\u{25C6}",
        tab: ToolbarTab::Create,
        category: Some(CommandCategory::View),
        default_chord: Some(Chord::bare(Key::Char('7'))),
        when: Some("!input_focused"),
        target: Target::Host,
        ..CMD_DEFAULTS
    },
    Command {
        id: "camera_top",
        label_key: "cmd.camera_top.label",
        keywords: &["camera", "plan", "down"],
        shortcut: Some("8"),
        icon: "\u{25AB}",
        tab: ToolbarTab::Create,
        category: Some(CommandCategory::View),
        default_chord: Some(Chord::bare(Key::Char('8'))),
        when: Some("!input_focused"),
        target: Target::Host,
        ..CMD_DEFAULTS
    },
    Command {
        id: "camera_front",
        label_key: "cmd.camera_front.label",
        keywords: &["camera", "elevation"],
        shortcut: Some("9"),
        icon: "\u{25A1}",
        tab: ToolbarTab::Create,
        category: Some(CommandCategory::View),
        default_chord: Some(Chord::bare(Key::Char('9'))),
        when: Some("!input_focused"),
        target: Target::Host,
        ..CMD_DEFAULTS
    },
    Command {
        id: "camera_right",
        label_key: "cmd.camera_right.label",
        keywords: &["camera", "side", "profile"],
        shortcut: Some("0"),
        icon: "\u{25A1}",
        tab: ToolbarTab::Create,
        category: Some(CommandCategory::View),
        default_chord: Some(Chord::bare(Key::Char('0'))),
        when: Some("!input_focused"),
        target: Target::Host,
        ..CMD_DEFAULTS
    },
    Command {
        id: "camera_fit",
        label_key: "cmd.camera_fit.label",
        keywords: &["camera", "zoom", "frame", "home"],
        shortcut: Some("F"),
        icon: "\u{2922}",
        tab: ToolbarTab::Create,
        category: Some(CommandCategory::View),
        default_chord: Some(Chord::bare(Key::Char('f'))),
        when: Some("!input_focused"),
        target: Target::Host,
        ..CMD_DEFAULTS
    },
    // ── Tools ────────────────────────────────────────────────────────
    Command {
        id: "palette",
        label_key: "cmd.palette.label",
        keywords: &["search", "command", "palette", "jump"],
        shortcut: Some("Cmd+K"),
        icon: "\u{2318}",
        tab: ToolbarTab::Create,
        category: Some(CommandCategory::Tools),
        default_chord: Some(Chord::primary(Key::Char('k'))),
        target: Target::Host,
        ..CMD_DEFAULTS
    },
    Command {
        id: "sketch",
        label_key: "cmd.sketch.label",
        keywords: &["2d", "draw", "profile"],
        icon: "\u{270E}",
        tab: ToolbarTab::Create,
        category: Some(CommandCategory::Tools),
        target: Target::Host,
        ..CMD_DEFAULTS
    },
    // ── Help ─────────────────────────────────────────────────────────
    Command {
        id: "about",
        label_key: "cmd.about.label",
        keywords: &["info", "version", "credits"],
        shortcut: Some("F1"),
        icon: "\u{2139}",
        tab: ToolbarTab::Create,
        category: Some(CommandCategory::Help),
        default_chord: Some(Chord::bare(Key::F(1))),
        target: Target::Host,
        ..CMD_DEFAULTS
    },
    Command {
        id: "open_docs",
        label_key: "cmd.open_docs.label",
        keywords: &["help", "manual", "guide"],
        icon: "\u{1F4D6}",
        tab: ToolbarTab::Create,
        category: Some(CommandCategory::Help),
        target: Target::Host,
        ..CMD_DEFAULTS
    },
    Command {
        id: "open_github",
        label_key: "cmd.open_github.label",
        keywords: &["source", "repo", "code"],
        icon: "\u{2756}",
        tab: ToolbarTab::Create,
        category: Some(CommandCategory::Help),
        target: Target::Host,
        ..CMD_DEFAULTS
    },
    Command {
        id: "open_discord",
        label_key: "cmd.open_discord.label",
        keywords: &["chat", "community", "help"],
        icon: "\u{25CD}",
        tab: ToolbarTab::Create,
        category: Some(CommandCategory::Help),
        target: Target::Host,
        ..CMD_DEFAULTS
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_commands() {
        assert!(!all_commands().is_empty());
    }

    #[test]
    fn test_find_commands() {
        let results = find_commands("cube");
        assert!(!results.is_empty());
        assert_eq!(results[0].id, "cube");
    }

    #[test]
    fn test_find_by_keyword() {
        let results = find_commands("hollow");
        assert!(!results.is_empty());
        assert_eq!(results[0].id, "shell");
    }

    #[test]
    fn test_empty_query_returns_all() {
        let results = find_commands("");
        assert_eq!(results.len(), all_commands().len());
    }

    #[test]
    fn every_command_has_unique_id() {
        let mut seen = std::collections::HashSet::new();
        for cmd in all_commands() {
            assert!(seen.insert(cmd.id), "duplicate command id: {}", cmd.id);
        }
    }

    #[test]
    fn all_when_clauses_parse() {
        use crate::context::WhenExpr;
        for cmd in all_commands() {
            if let Some(src) = cmd.when {
                WhenExpr::parse(src)
                    .unwrap_or_else(|e| panic!("command {} has bad when {:?}: {}", cmd.id, src, e));
            }
        }
    }
}
