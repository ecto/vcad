//! The `.vcad` seam: a serde cavity schema with named parameters.
//!
//! A [`CavitySpec`] is the serialization contract between vcad documents and
//! this crate. Every numeric field is a [`ParamValue`]: a literal, or the
//! **name** of a document parameter supplied at resolve time. Resolution is
//! fail-closed — an unbound name is an error, never a default (the same
//! contract as `vcad-kernel-particle`'s `DeviceSpec`).
//!
//! The spec also classifies each named parameter by how its gradient is
//! obtained ([`ParamRole`]). In M0 every acoustic design lever is geometric
//! (port length, radius, box volume) and moves the discretisation, so all
//! parameters take **finite differences** — there is no adjoint yet. The
//! role machinery is kept for parity so a future field adjoint slots in
//! without changing callers.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::cavity::{Cavity, EndCondition, Segment};
use crate::medium::Medium;

/// A literal value or the name of a document parameter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    /// A literal number.
    Literal(f64),
    /// The name of a parameter bound at [`CavitySpec::resolve`] time.
    Named(String),
}

impl From<f64> for ParamValue {
    fn from(v: f64) -> Self {
        ParamValue::Literal(v)
    }
}

impl ParamValue {
    fn resolve(&self, params: &BTreeMap<String, f64>) -> Result<f64, SpecError> {
        match self {
            ParamValue::Literal(v) => Ok(*v),
            ParamValue::Named(name) => params
                .get(name)
                .copied()
                .ok_or_else(|| SpecError::UnknownParameter(name.clone())),
        }
    }

    fn name(&self) -> Option<&str> {
        match self {
            ParamValue::Literal(_) => None,
            ParamValue::Named(n) => Some(n),
        }
    }
}

/// One coaxial segment in the spec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SegmentSpec {
    /// Lower z bound, mm.
    pub z0_mm: ParamValue,
    /// Upper z bound, mm.
    pub z1_mm: ParamValue,
    /// Segment radius, mm.
    pub radius_mm: ParamValue,
}

/// An end condition in the spec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EndSpec {
    /// Rigid wall.
    Rigid,
    /// Open pressure-release mouth.
    Open,
    /// Driven piston of the given radius (mm).
    Piston {
        /// Piston radius, mm.
        radius_mm: ParamValue,
    },
    /// Locally-reacting impedance termination, normalized admittance `β = ρc/Z`.
    Impedance {
        /// Normalized admittance `β` (dimensionless).
        admittance: ParamValue,
    },
}

/// Serializable axisymmetric cavity with named parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CavitySpec {
    /// Coaxial segments, stacked along +z.
    pub segments: Vec<SegmentSpec>,
    /// Bottom (−z) end condition.
    pub bottom: EndSpec,
    /// Top (+z) end condition.
    pub top: EndSpec,
    /// Medium temperature, °C.
    pub temp_c: ParamValue,
}

/// How a named parameter's gradient is computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamRole {
    /// Finite differences (all M0 acoustic parameters — they move the mesh).
    FiniteDifference,
}

/// Resolution failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecError {
    /// A named parameter had no binding.
    UnknownParameter(String),
    /// The spec had no segments.
    Empty,
}

impl std::fmt::Display for SpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpecError::UnknownParameter(name) => write!(f, "unbound cavity parameter: {name:?}"),
            SpecError::Empty => write!(f, "cavity spec has no segments"),
        }
    }
}

impl std::error::Error for SpecError {}

impl EndSpec {
    fn resolve(&self, params: &BTreeMap<String, f64>) -> Result<EndCondition, SpecError> {
        Ok(match self {
            EndSpec::Rigid => EndCondition::Rigid,
            EndSpec::Open => EndCondition::Open,
            EndSpec::Piston { radius_mm } => EndCondition::Piston {
                radius_mm: radius_mm.resolve(params)?,
            },
            EndSpec::Impedance { admittance } => EndCondition::Impedance {
                admittance: admittance.resolve(params)?,
            },
        })
    }
}

impl CavitySpec {
    /// Resolve every field against `params`, fail-closed.
    pub fn resolve(&self, params: &BTreeMap<String, f64>) -> Result<Cavity, SpecError> {
        if self.segments.is_empty() {
            return Err(SpecError::Empty);
        }
        let segments = self
            .segments
            .iter()
            .map(|s| {
                Ok(Segment {
                    z0_mm: s.z0_mm.resolve(params)?,
                    z1_mm: s.z1_mm.resolve(params)?,
                    radius_mm: s.radius_mm.resolve(params)?,
                })
            })
            .collect::<Result<Vec<_>, SpecError>>()?;
        Ok(Cavity {
            segments,
            bottom: self.bottom.resolve(params)?,
            top: self.top.resolve(params)?,
            medium: Medium::air(self.temp_c.resolve(params)?),
        })
    }

    /// Every named parameter with its gradient role (all
    /// [`ParamRole::FiniteDifference`] in M0).
    pub fn parameter_roles(&self) -> BTreeMap<String, ParamRole> {
        let mut roles = BTreeMap::new();
        let mut put = |v: &ParamValue| {
            if let Some(name) = v.name() {
                roles.insert(name.to_string(), ParamRole::FiniteDifference);
            }
        };
        for s in &self.segments {
            put(&s.z0_mm);
            put(&s.z1_mm);
            put(&s.radius_mm);
        }
        for e in [&self.bottom, &self.top] {
            match e {
                EndSpec::Piston { radius_mm } => put(radius_mm),
                EndSpec::Impedance { admittance } => put(admittance),
                EndSpec::Rigid | EndSpec::Open => {}
            }
        }
        put(&self.temp_c);
        roles
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn json_round_trip_with_named_port_length() {
        let json = r#"{
            "segments": [
                { "z0_mm": 0.0, "z1_mm": 250.0, "radius_mm": 90.0 },
                { "z0_mm": 250.0, "z1_mm": "port_top", "radius_mm": 25.0 }
            ],
            "bottom": { "kind": "piston", "radius_mm": 70.0 },
            "top": { "kind": "open" },
            "temp_c": 20.0
        }"#;
        let spec: CavitySpec = serde_json::from_str(json).expect("parse");
        let cav = spec
            .resolve(&params(&[("port_top", 310.0)]))
            .expect("resolve");
        assert_eq!(cav.segments.len(), 2);
        assert!((cav.port_segment().length_mm() - 60.0).abs() < 1e-9);
        assert_eq!(cav.bottom, EndCondition::Piston { radius_mm: 70.0 });

        let round = serde_json::to_string(&spec).unwrap();
        let spec2: CavitySpec = serde_json::from_str(&round).unwrap();
        assert_eq!(spec, spec2);
    }

    #[test]
    fn impedance_end_round_trips() {
        let json = r#"{
            "segments": [{ "z0_mm": 0.0, "z1_mm": 100.0, "radius_mm": 30.0 }],
            "bottom": { "kind": "rigid" },
            "top": { "kind": "impedance", "admittance": "beta" },
            "temp_c": 20.0
        }"#;
        let spec: CavitySpec = serde_json::from_str(json).expect("parse");
        assert_eq!(spec.parameter_roles()["beta"], ParamRole::FiniteDifference);
        let cav = spec.resolve(&params(&[("beta", 1.0)])).expect("resolve");
        assert_eq!(cav.top, EndCondition::Impedance { admittance: 1.0 });
        // Serialize → reparse is stable.
        let round = serde_json::to_string(&spec).unwrap();
        assert_eq!(spec, serde_json::from_str::<CavitySpec>(&round).unwrap());
    }

    #[test]
    fn resolution_is_fail_closed() {
        let spec = CavitySpec {
            segments: vec![SegmentSpec {
                z0_mm: 0.0.into(),
                z1_mm: ParamValue::Named("missing".into()),
                radius_mm: 50.0.into(),
            }],
            bottom: EndSpec::Rigid,
            top: EndSpec::Rigid,
            temp_c: 20.0.into(),
        };
        assert_eq!(
            spec.resolve(&BTreeMap::new()).unwrap_err(),
            SpecError::UnknownParameter("missing".into())
        );
    }

    #[test]
    fn parameter_roles_are_all_finite_difference() {
        let spec = CavitySpec {
            segments: vec![SegmentSpec {
                z0_mm: 0.0.into(),
                z1_mm: ParamValue::Named("h".into()),
                radius_mm: ParamValue::Named("r".into()),
            }],
            bottom: EndSpec::Piston {
                radius_mm: ParamValue::Named("rd".into()),
            },
            top: EndSpec::Open,
            temp_c: 20.0.into(),
        };
        let roles = spec.parameter_roles();
        assert_eq!(roles["h"], ParamRole::FiniteDifference);
        assert_eq!(roles["r"], ParamRole::FiniteDifference);
        assert_eq!(roles["rd"], ParamRole::FiniteDifference);
    }
}
