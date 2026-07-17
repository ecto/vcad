//! The analysis specification: material, loads, supports, resolution.
//!
//! Loads and supports select mesh nodes with axis-aligned box regions —
//! the same face-selection idiom as `vcad-kernel-topopt`. Selection is
//! fail-closed: a region that matches no node is an error, never a silent
//! no-op (an unanchored bracket "passing" its analysis would be a lie).

use serde::{Deserialize, Serialize};

/// An axis-aligned box region, mm. A node is selected when it lies inside
/// the box inflated by a small tolerance (¼ of the lattice pitch), so a
/// zero-thickness box on a face selects that face's nodes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RegionBox {
    /// Minimum corner, mm.
    pub min: [f64; 3],
    /// Maximum corner, mm.
    pub max: [f64; 3],
}

impl RegionBox {
    /// Whether `p` lies inside the region inflated by `tol` on every side.
    pub fn contains(&self, p: [f64; 3], tol: f64) -> bool {
        (0..3).all(|a| p[a] >= self.min[a] - tol && p[a] <= self.max[a] + tol)
    }
}

/// A force applied to the nodes in a region, split evenly among them.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Load {
    /// Node-selection region.
    pub region: RegionBox,
    /// Total force over the region, Newtons, `[Fx, Fy, Fz]`.
    pub force: [f64; 3],
}

/// A support fixing displacement components of the nodes in a region.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Support {
    /// Node-selection region.
    pub region: RegionBox,
    /// Which components to fix, `[x, y, z]`.
    pub fix: [bool; 3],
}

/// Full specification of a static structural analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeaSpec {
    /// Lattice cell count along the longest bounding-box axis, clamped to
    /// `[2, 256]`. This is the *coarse* level; the convergence pass also
    /// solves at 2× this. Linear tets are stiff in bending — keep at
    /// least ~6 cells through the thinnest load-bearing section.
    #[serde(default = "default_resolution")]
    pub resolution: usize,
    /// Young's modulus, MPa (N/mm²) — e.g. 69 000 for 6061 aluminum,
    /// 200 000 for steel, 2 300 for PLA.
    #[serde(default = "default_youngs_modulus")]
    pub youngs_modulus_mpa: f64,
    /// Poisson's ratio, in `[0, 0.5)`.
    #[serde(default = "default_poisson")]
    pub poisson: f64,
    /// Yield strength, MPa. When given, the safety factor
    /// `yield / max_von_mises` is computed and claimed.
    #[serde(default)]
    pub yield_strength_mpa: Option<f64>,
    /// Applied loads (at least one, each with nonzero force).
    pub loads: Vec<Load>,
    /// Supports (at least one, each fixing at least one component).
    pub supports: Vec<Support>,
}

fn default_resolution() -> usize {
    24
}
fn default_youngs_modulus() -> f64 {
    69_000.0 // 6061-T6 aluminum
}
fn default_poisson() -> f64 {
    0.33
}

/// Spec validation failures.
#[derive(Debug, Clone, PartialEq)]
pub enum SpecError {
    /// The spec is structurally invalid; the message says how.
    Invalid(String),
}

impl std::fmt::Display for SpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpecError::Invalid(msg) => write!(f, "invalid FEA spec: {msg}"),
        }
    }
}

impl std::error::Error for SpecError {}

impl FeaSpec {
    /// Validate the physical inputs (geometry-independent checks).
    pub fn validate(&self) -> Result<(), SpecError> {
        if self.loads.is_empty() {
            return Err(SpecError::Invalid("at least one load is required".into()));
        }
        if self.supports.is_empty() {
            return Err(SpecError::Invalid(
                "at least one support is required".into(),
            ));
        }
        if self
            .loads
            .iter()
            .any(|l| l.force.iter().all(|c| *c == 0.0) || l.force.iter().any(|c| !c.is_finite()))
        {
            return Err(SpecError::Invalid(
                "every load needs a finite, nonzero force".into(),
            ));
        }
        if self.supports.iter().any(|s| !s.fix.iter().any(|f| *f)) {
            return Err(SpecError::Invalid(
                "a support fixes no displacement component".into(),
            ));
        }
        if !self.youngs_modulus_mpa.is_finite() || self.youngs_modulus_mpa <= 0.0 {
            return Err(SpecError::Invalid(format!(
                "youngs_modulus_mpa must be positive, got {}",
                self.youngs_modulus_mpa
            )));
        }
        if !(0.0..0.5).contains(&self.poisson) {
            return Err(SpecError::Invalid(format!(
                "poisson must be in [0, 0.5), got {}",
                self.poisson
            )));
        }
        if let Some(y) = self.yield_strength_mpa {
            if !y.is_finite() || y <= 0.0 {
                return Err(SpecError::Invalid(format!(
                    "yield_strength_mpa must be positive, got {y}"
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_spec() -> FeaSpec {
        FeaSpec {
            resolution: 24,
            youngs_modulus_mpa: 69_000.0,
            poisson: 0.33,
            yield_strength_mpa: Some(276.0),
            loads: vec![Load {
                region: RegionBox {
                    min: [10.0, 0.0, 0.0],
                    max: [10.0, 5.0, 5.0],
                },
                force: [0.0, 0.0, -50.0],
            }],
            supports: vec![Support {
                region: RegionBox {
                    min: [0.0, 0.0, 0.0],
                    max: [0.0, 5.0, 5.0],
                },
                fix: [true, true, true],
            }],
        }
    }

    #[test]
    fn validation_is_fail_closed() {
        assert!(ok_spec().validate().is_ok());
        let mut s = ok_spec();
        s.loads.clear();
        assert!(s.validate().is_err());
        let mut s = ok_spec();
        s.loads[0].force = [0.0; 3];
        assert!(s.validate().is_err());
        let mut s = ok_spec();
        s.supports[0].fix = [false; 3];
        assert!(s.validate().is_err());
        let mut s = ok_spec();
        s.poisson = 0.5;
        assert!(s.validate().is_err());
        let mut s = ok_spec();
        s.yield_strength_mpa = Some(-1.0);
        assert!(s.validate().is_err());
    }

    #[test]
    fn region_tolerance_selects_faces() {
        let r = RegionBox {
            min: [0.0, 0.0, 0.0],
            max: [0.0, 10.0, 10.0],
        };
        assert!(r.contains([0.0, 5.0, 5.0], 0.25));
        assert!(r.contains([0.2, 5.0, 5.0], 0.25));
        assert!(!r.contains([1.0, 5.0, 5.0], 0.25));
    }

    #[test]
    fn spec_round_trips_json() {
        let s = ok_spec();
        let json = serde_json::to_string(&s).unwrap();
        let back: FeaSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(back.loads, s.loads);
        assert_eq!(back.supports, s.supports);
        // Defaults fill in when omitted.
        let minimal: FeaSpec = serde_json::from_str(
            r#"{"loads":[{"region":{"min":[0,0,0],"max":[1,1,1]},"force":[0,0,-1]}],
                "supports":[{"region":{"min":[0,0,0],"max":[0,1,1]},"fix":[true,true,true]}]}"#,
        )
        .unwrap();
        assert_eq!(minimal.resolution, 24);
        assert_eq!(minimal.yield_strength_mpa, None);
    }
}
