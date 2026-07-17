//! The `.vcad` seam: a serde device schema with named parameters.
//!
//! A [`DeviceSpec`] is the serialization contract between vcad documents
//! and this crate. Every numeric field is a [`ParamValue`]: either a
//! literal, or the **name** of a document parameter to be supplied at
//! resolve time. Resolution is fail-closed — an unbound name is an error,
//! never a default.
//!
//! The spec also classifies each named parameter by how its gradient is
//! obtained ([`ParamRole`]): potentials and coil currents flow through the
//! discrete adjoint ([`crate::adjoint::yield_gradient`]); geometry moves
//! the Dirichlet mask, which is discrete, so geometric parameters take
//! finite differences until the shape-adjoint milestone.
//!
//! BRep extraction (revolved sketch sections → rings) is deliberately not
//! here: it lands on the vcad side of the seam, emitting this schema.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::device::{Device, WireRing};

/// A literal value or the name of a document parameter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    /// A literal number.
    Literal(f64),
    /// The name of a parameter bound at [`DeviceSpec::resolve`] time.
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

fn zero() -> ParamValue {
    ParamValue::Literal(0.0)
}

/// One wire ring electrode in the spec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RingSpec {
    /// Ring radius, mm.
    pub ring_radius_mm: ParamValue,
    /// Axial position, mm.
    pub z_mm: ParamValue,
    /// Wire (minor) radius, mm.
    pub wire_radius_mm: ParamValue,
    /// Electrode potential, volts.
    pub potential_v: ParamValue,
    /// Circulating current, ampere-turns (defaults to 0).
    #[serde(default = "zero")]
    pub ampere_turns: ParamValue,
}

/// Serializable axisymmetric device with named parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceSpec {
    /// Chamber inner radius, mm.
    pub chamber_radius_mm: ParamValue,
    /// Chamber half-height, mm.
    pub chamber_half_height_mm: ParamValue,
    /// Chamber wall potential, volts (defaults to 0 — grounded).
    #[serde(default = "zero")]
    pub wall_potential_v: ParamValue,
    /// Wire ring electrodes.
    pub rings: Vec<RingSpec>,
}

/// How a named parameter's gradient is computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamRole {
    /// Reverse-mode via [`crate::adjoint::yield_gradient`] (potentials,
    /// ampere-turns).
    Adjoint,
    /// Finite differences (geometry: it moves the Dirichlet mask).
    FiniteDifference,
}

/// Resolution failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecError {
    /// A named parameter had no binding.
    UnknownParameter(String),
}

impl std::fmt::Display for SpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpecError::UnknownParameter(name) => {
                write!(f, "unbound device parameter: {name:?}")
            }
        }
    }
}

impl std::error::Error for SpecError {}

impl DeviceSpec {
    /// Resolve every field against `params`, fail-closed.
    pub fn resolve(&self, params: &BTreeMap<String, f64>) -> Result<Device, SpecError> {
        let rings = self
            .rings
            .iter()
            .map(|r| {
                Ok(WireRing {
                    ring_radius_mm: r.ring_radius_mm.resolve(params)?,
                    z_mm: r.z_mm.resolve(params)?,
                    wire_radius_mm: r.wire_radius_mm.resolve(params)?,
                    potential_v: r.potential_v.resolve(params)?,
                    ampere_turns: r.ampere_turns.resolve(params)?,
                })
            })
            .collect::<Result<Vec<_>, SpecError>>()?;
        Ok(Device {
            chamber_radius_mm: self.chamber_radius_mm.resolve(params)?,
            chamber_half_height_mm: self.chamber_half_height_mm.resolve(params)?,
            wall_potential_v: self.wall_potential_v.resolve(params)?,
            rings,
        })
    }

    /// A literal (parameter-free) spec mirroring `device` — the round-trip
    /// starting point for documents that don't parameterize.
    pub fn from_device(device: &Device) -> Self {
        Self {
            chamber_radius_mm: device.chamber_radius_mm.into(),
            chamber_half_height_mm: device.chamber_half_height_mm.into(),
            wall_potential_v: device.wall_potential_v.into(),
            rings: device
                .rings
                .iter()
                .map(|r| RingSpec {
                    ring_radius_mm: r.ring_radius_mm.into(),
                    z_mm: r.z_mm.into(),
                    wire_radius_mm: r.wire_radius_mm.into(),
                    potential_v: r.potential_v.into(),
                    ampere_turns: r.ampere_turns.into(),
                })
                .collect(),
        }
    }

    /// Every named parameter with its gradient role. A name used by both
    /// an adjoint-capable field and a geometric field is conservatively
    /// classified [`ParamRole::FiniteDifference`].
    pub fn parameter_roles(&self) -> BTreeMap<String, ParamRole> {
        let mut roles: BTreeMap<String, ParamRole> = BTreeMap::new();
        let mut put = |value: &ParamValue, role: ParamRole| {
            if let Some(name) = value.name() {
                roles
                    .entry(name.to_string())
                    .and_modify(|existing| {
                        if role == ParamRole::FiniteDifference {
                            *existing = ParamRole::FiniteDifference;
                        }
                    })
                    .or_insert(role);
            }
        };
        put(&self.chamber_radius_mm, ParamRole::FiniteDifference);
        put(&self.chamber_half_height_mm, ParamRole::FiniteDifference);
        put(&self.wall_potential_v, ParamRole::Adjoint);
        for r in &self.rings {
            put(&r.ring_radius_mm, ParamRole::FiniteDifference);
            put(&r.z_mm, ParamRole::FiniteDifference);
            put(&r.wire_radius_mm, ParamRole::FiniteDifference);
            put(&r.potential_v, ParamRole::Adjoint);
            put(&r.ampere_turns, ParamRole::Adjoint);
        }
        roles
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::Device;

    fn params(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn json_round_trip_with_named_parameters() {
        let json = r#"{
            "chamber_radius_mm": 150.0,
            "chamber_half_height_mm": 150.0,
            "rings": [
                {
                    "ring_radius_mm": 45.0,
                    "z_mm": "ring_z",
                    "wire_radius_mm": 3.0,
                    "potential_v": "cathode_v",
                    "ampere_turns": "shield_at"
                },
                {
                    "ring_radius_mm": 45.0,
                    "z_mm": "ring_z_neg",
                    "wire_radius_mm": 3.0,
                    "potential_v": "cathode_v",
                    "ampere_turns": "shield_at_neg"
                }
            ]
        }"#;
        let spec: DeviceSpec = serde_json::from_str(json).expect("parse");
        let device = spec
            .resolve(&params(&[
                ("ring_z", 25.0),
                ("ring_z_neg", -25.0),
                ("cathode_v", -30_000.0),
                ("shield_at", 40_000.0),
                ("shield_at_neg", -40_000.0),
            ]))
            .expect("resolve");
        let reference = Device::shielded_two_ring(150.0, 45.0, 25.0, 3.0, -30_000.0, 40_000.0);
        assert_eq!(device, reference);

        // Serialize → parse → resolve again: identical device.
        let round = serde_json::to_string(&spec).expect("serialize");
        let spec2: DeviceSpec = serde_json::from_str(&round).expect("reparse");
        assert_eq!(spec, spec2);
    }

    #[test]
    fn resolution_is_fail_closed() {
        let spec = DeviceSpec {
            chamber_radius_mm: 100.0.into(),
            chamber_half_height_mm: 100.0.into(),
            wall_potential_v: 0.0.into(),
            rings: vec![RingSpec {
                ring_radius_mm: 40.0.into(),
                z_mm: ParamValue::Named("missing".into()),
                wire_radius_mm: 2.0.into(),
                potential_v: (-1_000.0).into(),
                ampere_turns: 0.0.into(),
            }],
        };
        let err = spec.resolve(&BTreeMap::new()).unwrap_err();
        assert_eq!(err, SpecError::UnknownParameter("missing".into()));
    }

    #[test]
    fn from_device_round_trips_and_solves() {
        let device = Device::classic_fusor(120.0, 40.0, 3, 1.0, -5_000.0);
        let spec = DeviceSpec::from_device(&device);
        let resolved = spec.resolve(&BTreeMap::new()).expect("literal resolve");
        assert_eq!(device, resolved);
        // And the resolved device actually runs through the solver.
        let sol =
            crate::poisson::solve(&resolved, 41, 81, &crate::poisson::SolveOptions::default())
                .expect("solve");
        assert!(sol.potential_at(0.0, 0.0) < -100.0);
    }

    #[test]
    fn parameter_roles_classify_the_gradient_path() {
        let spec = DeviceSpec {
            chamber_radius_mm: 150.0.into(),
            chamber_half_height_mm: 150.0.into(),
            wall_potential_v: 0.0.into(),
            rings: vec![RingSpec {
                ring_radius_mm: ParamValue::Named("geo".into()),
                z_mm: 25.0.into(),
                wire_radius_mm: 3.0.into(),
                potential_v: ParamValue::Named("bias".into()),
                ampere_turns: ParamValue::Named("shield".into()),
            }],
        };
        let roles = spec.parameter_roles();
        assert_eq!(roles["bias"], ParamRole::Adjoint);
        assert_eq!(roles["shield"], ParamRole::Adjoint);
        assert_eq!(roles["geo"], ParamRole::FiniteDifference);

        // A name shared across an adjoint field and a geometric field is
        // conservatively FD.
        let mut mixed = spec.clone();
        mixed.rings[0].wire_radius_mm = ParamValue::Named("bias".into());
        assert_eq!(mixed.parameter_roles()["bias"], ParamRole::FiniteDifference);
    }
}
