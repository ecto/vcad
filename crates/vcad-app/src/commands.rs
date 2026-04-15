//! Command registry with metadata.

/// Toolbar tab categories.
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

/// Command metadata.
#[derive(Debug, Clone)]
pub struct Command {
    pub id: &'static str,
    pub label: &'static str,
    pub keywords: &'static [&'static str],
    pub shortcut: Option<&'static str>,
    pub icon: &'static str,
    pub tab: ToolbarTab,
}

/// All registered commands.
pub fn all_commands() -> &'static [Command] {
    COMMANDS
}

/// Search commands by query (case-insensitive prefix match on id, label, keywords).
pub fn find_commands(query: &str) -> Vec<&'static Command> {
    if query.is_empty() {
        return COMMANDS.iter().collect();
    }
    let q = query.to_lowercase();
    COMMANDS
        .iter()
        .filter(|cmd| {
            cmd.id.to_lowercase().contains(&q)
                || cmd.label.to_lowercase().contains(&q)
                || cmd.keywords.iter().any(|k| k.to_lowercase().contains(&q))
        })
        .collect()
}

static COMMANDS: &[Command] = &[
    // Create
    Command {
        id: "cube",
        label: "Add Cube",
        keywords: &["box", "rectangular", "prism"],
        shortcut: None,
        icon: "\u{25A0}",
        tab: ToolbarTab::Create,
    },
    Command {
        id: "cylinder",
        label: "Add Cylinder",
        keywords: &["cyl", "tube", "pipe"],
        shortcut: None,
        icon: "\u{25CB}",
        tab: ToolbarTab::Create,
    },
    Command {
        id: "sphere",
        label: "Add Sphere",
        keywords: &["ball", "globe"],
        shortcut: None,
        icon: "\u{25CF}",
        tab: ToolbarTab::Create,
    },
    Command {
        id: "cone",
        label: "Add Cone",
        keywords: &["conical", "taper"],
        shortcut: None,
        icon: "\u{25B2}",
        tab: ToolbarTab::Create,
    },
    // Transform
    Command {
        id: "translate",
        label: "Move",
        keywords: &["translate", "position", "offset"],
        shortcut: Some("g"),
        icon: "\u{2194}",
        tab: ToolbarTab::Transform,
    },
    Command {
        id: "rotate",
        label: "Rotate",
        keywords: &["spin", "turn", "orientation"],
        shortcut: Some("r"),
        icon: "\u{21BB}",
        tab: ToolbarTab::Transform,
    },
    Command {
        id: "scale",
        label: "Scale",
        keywords: &["resize", "size"],
        shortcut: Some("s"),
        icon: "\u{2922}",
        tab: ToolbarTab::Transform,
    },
    Command {
        id: "mirror",
        label: "Mirror",
        keywords: &["flip", "reflect", "symmetry"],
        shortcut: None,
        icon: "\u{2016}",
        tab: ToolbarTab::Transform,
    },
    // Combine
    Command {
        id: "union",
        label: "Union",
        keywords: &["add", "join", "merge", "combine"],
        shortcut: None,
        icon: "\u{222A}",
        tab: ToolbarTab::Combine,
    },
    Command {
        id: "difference",
        label: "Difference",
        keywords: &["subtract", "cut", "remove"],
        shortcut: None,
        icon: "\u{2216}",
        tab: ToolbarTab::Combine,
    },
    Command {
        id: "intersection",
        label: "Intersection",
        keywords: &["intersect", "common", "overlap"],
        shortcut: None,
        icon: "\u{2229}",
        tab: ToolbarTab::Combine,
    },
    // Modify
    Command {
        id: "fillet",
        label: "Fillet",
        keywords: &["round", "radius", "smooth"],
        shortcut: None,
        icon: "\u{25E0}",
        tab: ToolbarTab::Modify,
    },
    Command {
        id: "chamfer",
        label: "Chamfer",
        keywords: &["bevel", "edge"],
        shortcut: None,
        icon: "\u{25FA}",
        tab: ToolbarTab::Modify,
    },
    Command {
        id: "shell",
        label: "Shell",
        keywords: &["hollow", "thin", "wall"],
        shortcut: None,
        icon: "\u{25A1}",
        tab: ToolbarTab::Modify,
    },
    Command {
        id: "linear_pattern",
        label: "Linear Pattern",
        keywords: &["array", "repeat", "linear"],
        shortcut: None,
        icon: "\u{2026}",
        tab: ToolbarTab::Modify,
    },
    Command {
        id: "circular_pattern",
        label: "Circular Pattern",
        keywords: &["radial", "polar", "array"],
        shortcut: None,
        icon: "\u{25CE}",
        tab: ToolbarTab::Modify,
    },
    // General
    Command {
        id: "delete",
        label: "Delete",
        keywords: &["remove", "erase", "del"],
        shortcut: Some("x"),
        icon: "\u{2715}",
        tab: ToolbarTab::Create,
    },
    Command {
        id: "undo",
        label: "Undo",
        keywords: &["back", "revert"],
        shortcut: Some("u"),
        icon: "\u{21B6}",
        tab: ToolbarTab::Create,
    },
    Command {
        id: "redo",
        label: "Redo",
        keywords: &["forward"],
        shortcut: Some("Ctrl+r"),
        icon: "\u{21B7}",
        tab: ToolbarTab::Create,
    },
    // Export / File I/O
    Command {
        id: "export_stl",
        label: "Export STL",
        keywords: &["stl", "mesh", "3d print"],
        shortcut: None,
        icon: "\u{2197}",
        tab: ToolbarTab::Export,
    },
    Command {
        id: "export_glb",
        label: "Export GLB",
        keywords: &["glb", "gltf", "mesh", "web"],
        shortcut: None,
        icon: "\u{2197}",
        tab: ToolbarTab::Export,
    },
    Command {
        id: "export_step",
        label: "Export STEP",
        keywords: &["step", "stp", "cad"],
        shortcut: None,
        icon: "\u{2197}",
        tab: ToolbarTab::Export,
    },
    Command {
        id: "save",
        label: "Save",
        keywords: &["write", "store"],
        shortcut: Some("Ctrl+s"),
        icon: "S",
        tab: ToolbarTab::Export,
    },
    Command {
        id: "new",
        label: "New Document",
        keywords: &["empty", "clear", "start"],
        shortcut: Some("Ctrl+n"),
        icon: "+",
        tab: ToolbarTab::Export,
    },
    Command {
        id: "open",
        label: "Open…",
        keywords: &["load", "file", "import"],
        shortcut: Some("Ctrl+o"),
        icon: "\u{2198}",
        tab: ToolbarTab::Export,
    },
    Command {
        id: "quit",
        label: "Quit",
        keywords: &["exit", "close", "q"],
        shortcut: Some("Ctrl+q"),
        icon: "\u{2715}",
        tab: ToolbarTab::Export,
    },
    // Edit clipboard
    Command {
        id: "copy",
        label: "Copy",
        keywords: &["clipboard", "yank"],
        shortcut: Some("Ctrl+c"),
        icon: "\u{29C9}",
        tab: ToolbarTab::Create,
    },
    Command {
        id: "paste",
        label: "Paste",
        keywords: &["clipboard"],
        shortcut: Some("Ctrl+v"),
        icon: "\u{2398}",
        tab: ToolbarTab::Create,
    },
    Command {
        id: "duplicate",
        label: "Duplicate",
        keywords: &["clone", "copy"],
        shortcut: Some("Ctrl+d"),
        icon: "\u{29C9}",
        tab: ToolbarTab::Create,
    },
    Command {
        id: "select_all",
        label: "Select All",
        keywords: &["all", "everything"],
        shortcut: Some("Ctrl+a"),
        icon: "\u{25A3}",
        tab: ToolbarTab::Create,
    },
    Command {
        id: "deselect",
        label: "Deselect",
        keywords: &["clear", "none"],
        shortcut: Some("Esc"),
        icon: "\u{25A1}",
        tab: ToolbarTab::Create,
    },
    // View toggles
    Command {
        id: "toggle_sidebar",
        label: "Toggle Sidebar",
        keywords: &["tree", "panel", "parts", "hide"],
        shortcut: Some("\\"),
        icon: "\u{2630}",
        tab: ToolbarTab::Create,
    },
    Command {
        id: "toggle_chat",
        label: "Toggle Chat",
        keywords: &["ai", "assistant", "panel"],
        shortcut: Some("`"),
        icon: "\u{2726}",
        tab: ToolbarTab::Chat,
    },
    Command {
        id: "toggle_wireframe",
        label: "Toggle Wireframe",
        keywords: &["edges", "mesh", "view"],
        shortcut: None,
        icon: "\u{25C7}",
        tab: ToolbarTab::Create,
    },
    Command {
        id: "cycle_theme",
        label: "Cycle Theme",
        keywords: &["dark", "light", "appearance"],
        shortcut: None,
        icon: "\u{25D0}",
        tab: ToolbarTab::Create,
    },
    // Camera presets
    Command {
        id: "camera_iso",
        label: "Isometric View",
        keywords: &["camera", "iso", "3d", "default"],
        shortcut: Some("7"),
        icon: "\u{25C6}",
        tab: ToolbarTab::Create,
    },
    Command {
        id: "camera_top",
        label: "Top View",
        keywords: &["camera", "plan", "down"],
        shortcut: Some("8"),
        icon: "\u{25AB}",
        tab: ToolbarTab::Create,
    },
    Command {
        id: "camera_front",
        label: "Front View",
        keywords: &["camera", "elevation"],
        shortcut: Some("9"),
        icon: "\u{25A1}",
        tab: ToolbarTab::Create,
    },
    Command {
        id: "camera_right",
        label: "Right View",
        keywords: &["camera", "side", "profile"],
        shortcut: Some("0"),
        icon: "\u{25A1}",
        tab: ToolbarTab::Create,
    },
    Command {
        id: "camera_fit",
        label: "Fit to Screen",
        keywords: &["camera", "zoom", "frame", "home"],
        shortcut: Some("f"),
        icon: "\u{2922}",
        tab: ToolbarTab::Create,
    },
    // Tools
    Command {
        id: "palette",
        label: "Command Palette",
        keywords: &["search", "command", "palette", "jump"],
        shortcut: Some(":"),
        icon: "\u{2318}",
        tab: ToolbarTab::Create,
    },
    Command {
        id: "sketch",
        label: "New Sketch",
        keywords: &["2d", "draw", "profile"],
        shortcut: Some("S"),
        icon: "\u{270E}",
        tab: ToolbarTab::Create,
    },
    // Help
    Command {
        id: "about",
        label: "About vcad",
        keywords: &["info", "version", "credits"],
        shortcut: None,
        icon: "\u{2139}",
        tab: ToolbarTab::Create,
    },
    Command {
        id: "open_docs",
        label: "Open Docs",
        keywords: &["help", "manual", "guide"],
        shortcut: None,
        icon: "\u{1F4D6}",
        tab: ToolbarTab::Create,
    },
    Command {
        id: "open_github",
        label: "GitHub",
        keywords: &["source", "repo", "code"],
        shortcut: None,
        icon: "\u{2756}",
        tab: ToolbarTab::Create,
    },
    Command {
        id: "open_discord",
        label: "Discord",
        keywords: &["chat", "community", "help"],
        shortcut: None,
        icon: "\u{25CD}",
        tab: ToolbarTab::Create,
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
}
