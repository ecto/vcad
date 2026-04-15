//! Application-level modes that scope keybindings and command availability.
//!
//! Shared by every host. Keybindings can declare a `ModeScope` of `Global`,
//! a single `Mode`, or several `Modes`, and the registry filters them during
//! resolve. The TUI already tracks a richer `TuiMode` (with per-mode state
//! structs) — it should map into `AppMode` at the dispatch boundary.

use serde::{Deserialize, Serialize};

/// Top-level application mode.
///
/// Hosts may keep additional local state (e.g. which sketch tool is active),
/// but the registry only cares about this coarse enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum AppMode {
    #[default]
    Normal,
    Sketch,
    Assembly,
    Physics,
    Cam,
    Print,
    Electronics,
    Drawing,
}

impl AppMode {
    pub const ALL: &'static [AppMode] = &[
        AppMode::Normal,
        AppMode::Sketch,
        AppMode::Assembly,
        AppMode::Physics,
        AppMode::Cam,
        AppMode::Print,
        AppMode::Electronics,
        AppMode::Drawing,
    ];

    pub fn name(self) -> &'static str {
        match self {
            AppMode::Normal => "Normal",
            AppMode::Sketch => "Sketch",
            AppMode::Assembly => "Assembly",
            AppMode::Physics => "Physics",
            AppMode::Cam => "Cam",
            AppMode::Print => "Print",
            AppMode::Electronics => "Electronics",
            AppMode::Drawing => "Drawing",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "Normal" => AppMode::Normal,
            "Sketch" => AppMode::Sketch,
            "Assembly" => AppMode::Assembly,
            "Physics" => AppMode::Physics,
            "Cam" => AppMode::Cam,
            "Print" => AppMode::Print,
            "Electronics" => AppMode::Electronics,
            "Drawing" => AppMode::Drawing,
            _ => return None,
        })
    }
}

/// Where a command's binding is active.
///
/// `Global` fires in any mode (subject to `when`). `Mode` restricts the
/// binding to a single mode. `Modes` allows a static list — useful for
/// commands that apply to several related modes (e.g. Normal + Assembly
/// both want Esc→deselect).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeScope {
    Global,
    Mode(AppMode),
    Modes(&'static [AppMode]),
}

impl ModeScope {
    pub fn matches(&self, mode: AppMode) -> bool {
        match self {
            ModeScope::Global => true,
            ModeScope::Mode(m) => *m == mode,
            ModeScope::Modes(ms) => ms.contains(&mode),
        }
    }

    /// `Global` beats `Modes` beats `Mode` — used when two bindings overlap
    /// at resolve time and we need a tie-breaker. More specific wins.
    pub fn specificity(&self) -> u8 {
        match self {
            ModeScope::Mode(_) => 2,
            ModeScope::Modes(_) => 1,
            ModeScope::Global => 0,
        }
    }
}

/// Advisory label for which side of the host boundary runs a command's
/// action. Not enforced — hosts still own their action dispatch — but useful
/// for UI grouping (e.g. "these commands work in TUI too").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    /// Action mutates kernel state (undo, boolean, etc.).
    Kernel,
    /// Action is host-specific UI (toggle sidebar, cycle theme).
    Host,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_names() {
        for m in AppMode::ALL {
            assert_eq!(AppMode::parse(m.name()), Some(*m));
        }
    }

    #[test]
    fn scope_specificity() {
        assert!(ModeScope::Mode(AppMode::Sketch).specificity() > ModeScope::Global.specificity());
        assert!(
            ModeScope::Mode(AppMode::Sketch).specificity()
                > ModeScope::Modes(&[AppMode::Sketch]).specificity()
        );
    }

    #[test]
    fn scope_matches() {
        assert!(ModeScope::Global.matches(AppMode::Sketch));
        assert!(ModeScope::Mode(AppMode::Sketch).matches(AppMode::Sketch));
        assert!(!ModeScope::Mode(AppMode::Sketch).matches(AppMode::Normal));
        assert!(
            ModeScope::Modes(&[AppMode::Normal, AppMode::Assembly]).matches(AppMode::Assembly)
        );
    }
}
