//! Rule pack loader and the per-process check modules.
//!
//! A [`RulePack`] is a process-specific TOML file in `lib/dfm/` that
//! lists rule thresholds and severity. Default packs are bundled into
//! the binary via `include_str!` so the WASM build doesn't need to fetch
//! them at runtime.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use vcad_kernel_cost::Process;

use crate::DfmError;

pub mod casting;
pub mod cnc;
pub mod fdm;
pub mod mold;
pub mod sheet;

/// A loaded DFM rule pack.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RulePack {
    /// Schema version string (e.g. `"vcad.dfm/1"`). Reserved for future
    /// breaking-change migrations.
    #[serde(default = "default_schema")]
    pub schema: String,
    /// Pack version (defaults to `"1"`).
    #[serde(default = "default_version")]
    pub version: String,
    /// Which process this pack applies to.
    pub process: Process,
    /// Human-readable name.
    pub name: String,
    /// Optional notes shown in the UI ("assumes 6 mm end mill", etc.).
    #[serde(default)]
    pub notes: String,
    /// Rule table — keyed by rule id (`"thin_wall"`, `"draft"`, …).
    #[serde(default)]
    pub rules: HashMap<String, Rule>,
}

fn default_schema() -> String {
    "vcad.dfm/1".to_string()
}

fn default_version() -> String {
    "1".to_string()
}

/// One rule entry inside a `RulePack`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    /// Severity to use when the rule fires.
    #[serde(default = "default_severity")]
    pub severity: String,
    /// Fix template id; check modules read this to decide which
    /// [`crate::DfmFix`] variant to emit.
    #[serde(default = "default_fix")]
    pub fix: String,
    /// Free-form numeric parameters — checks read whichever keys they
    /// care about (`min_wall_mm`, `max_overhang_deg`, …).
    #[serde(flatten)]
    pub params: HashMap<String, toml::Value>,
}

fn default_severity() -> String {
    "warning".into()
}

fn default_fix() -> String {
    "manual".into()
}

impl Rule {
    /// Look up a numeric parameter by key, with a fallback.
    pub fn num(&self, key: &str, fallback: f64) -> f64 {
        self.params
            .get(key)
            .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
            .unwrap_or(fallback)
    }

    /// Look up an optional string parameter.
    pub fn string(&self, key: &str) -> Option<String> {
        self.params
            .get(key)
            .and_then(|v| v.as_str().map(String::from))
    }

    /// Severity parsed into the strongly-typed enum.
    pub fn severity_enum(&self) -> crate::DfmSeverity {
        match self.severity.as_str() {
            "error" => crate::DfmSeverity::Error,
            "info" => crate::DfmSeverity::Info,
            _ => crate::DfmSeverity::Warning,
        }
    }
}

impl RulePack {
    /// Parse a TOML rule pack.
    pub fn from_toml(s: &str) -> Result<Self, DfmError> {
        let pack: RulePack = toml::from_str(s)?;
        Ok(pack)
    }

    /// Bundled default pack for a process.
    pub fn default_for(process: Process) -> Self {
        let src = DefaultPacks::source(process);
        Self::from_toml(src).expect("bundled default rule pack must parse")
    }

    /// Look up a rule by id (returns `None` if it isn't enabled in this pack).
    pub fn rule(&self, id: &str) -> Option<&Rule> {
        self.rules.get(id)
    }

    /// Iterate `(id, &Rule)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Rule)> {
        self.rules.iter()
    }
}

/// Access to the bundled default rule pack TOML sources.
pub struct DefaultPacks;

impl DefaultPacks {
    /// Raw TOML source for a process's default pack.
    pub fn source(process: Process) -> &'static str {
        match process {
            Process::Cnc3Axis => include_str!("../../../../lib/dfm/cnc-3axis.toml"),
            Process::Fdm => include_str!("../../../../lib/dfm/fdm.toml"),
            Process::Sla => include_str!("../../../../lib/dfm/sla.toml"),
            Process::Injection => include_str!("../../../../lib/dfm/injection-molding.toml"),
            Process::SheetMetal => include_str!("../../../../lib/dfm/sheet-metal.toml"),
            Process::CastingSand => include_str!("../../../../lib/dfm/casting-sand.toml"),
            Process::CastingInvestment => {
                include_str!("../../../../lib/dfm/casting-investment.toml")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_process_has_a_bundled_pack() {
        for p in [
            Process::Cnc3Axis,
            Process::Fdm,
            Process::Sla,
            Process::Injection,
            Process::SheetMetal,
            Process::CastingSand,
            Process::CastingInvestment,
        ] {
            let pack = RulePack::default_for(p);
            assert_eq!(pack.process, p);
            assert!(!pack.rules.is_empty(), "{:?} pack has no rules", p);
        }
    }

    #[test]
    fn rule_num_lookup_with_fallback() {
        let toml_src = r#"
            process = "cnc_3axis"
            name = "Test"
            [rules.thin_wall]
            severity = "error"
            min_wall_mm = 1.5
        "#;
        let pack = RulePack::from_toml(toml_src).unwrap();
        let rule = pack.rule("thin_wall").unwrap();
        assert!((rule.num("min_wall_mm", 0.0) - 1.5).abs() < 1e-9);
        assert!((rule.num("nonexistent", 9.0) - 9.0).abs() < 1e-9);
        assert_eq!(rule.severity_enum(), crate::DfmSeverity::Error);
    }
}
