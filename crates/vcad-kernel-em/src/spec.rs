//! The `.vcad` seam: serde device schemas with named parameters.
//!
//! A spec is the serialization contract between vcad documents and this
//! crate. Every numeric field is a [`ParamValue`]: a literal, or the
//! **name** of a document parameter supplied at resolve time. Resolution
//! is fail-closed — an unbound name is an error, never a default
//! (pattern inherited from `vcad_kernel_particle::spec`).
//!
//! [`AxisymSpec::parameter_roles`] / [`PlanarSpec::parameter_roles`]
//! classify each named parameter by how its gradient is obtained,
//! mirroring what [`crate::adjoint`] actually implements:
//!
//! - [`ParamRole::Adjoint`]: coil/conductor currents, linear-material
//!   μ_r, magnet remanence components — priced by one adjoint solve;
//! - [`ParamRole::FiniteDifference`]: geometry (region edges move the
//!   deposit masks — discrete), turns (linear in principle but not
//!   exposed by the adjoint API yet — classified by what is
//!   implemented, not what is possible), and μ_r of **saturable**
//!   materials (the secant ν depends on the field through the Picard
//!   fixed point; the frozen-ν adjoint refuses them fail-closed).
//!
//! BRep extraction (solids of revolution → coil/material annuli, board
//! stacks → planar rects) deliberately lives on the vcad side of the
//! seam, emitting these schemas.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::axisym::{Annulus, AxisymMagnetostatics, Coil, Material};
use crate::grid::Bc;
use crate::material::Saturation;
use crate::planar::{Conductor, MagnetBlock, PlanarMagnetostatics, PlanarMaterial, Rect};

/// A literal value or the name of a document parameter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    /// A literal number.
    Literal(f64),
    /// The name of a parameter bound at resolve time.
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

fn one() -> ParamValue {
    ParamValue::Literal(1.0)
}

fn bc_zero() -> Bc {
    Bc::Zero
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

/// How a named parameter's gradient is computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamRole {
    /// Reverse-mode via [`crate::adjoint`].
    Adjoint,
    /// Finite differences (geometry, turns, saturable-material μ).
    FiniteDifference,
}

/// An axis-aligned mm rectangle with parameterizable bounds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RectSpec {
    /// Left / inner-radius edge, mm.
    pub x_min_mm: ParamValue,
    /// Right / outer-radius edge, mm.
    pub x_max_mm: ParamValue,
    /// Bottom edge, mm.
    pub y_min_mm: ParamValue,
    /// Top edge, mm.
    pub y_max_mm: ParamValue,
}

impl RectSpec {
    fn resolve(&self, p: &BTreeMap<String, f64>) -> Result<(f64, f64, f64, f64), SpecError> {
        Ok((
            self.x_min_mm.resolve(p)?,
            self.x_max_mm.resolve(p)?,
            self.y_min_mm.resolve(p)?,
            self.y_max_mm.resolve(p)?,
        ))
    }

    fn collect_roles(&self, roles: &mut RoleMap) {
        for v in [
            &self.x_min_mm,
            &self.x_max_mm,
            &self.y_min_mm,
            &self.y_max_mm,
        ] {
            roles.put(v, ParamRole::FiniteDifference);
        }
    }
}

/// One coil of revolution in an [`AxisymSpec`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoilSpec {
    /// Cross-section (x = radius, y = z).
    pub region: RectSpec,
    /// Turns over the cross-section (defaults to 1).
    #[serde(default = "one")]
    pub turns: ParamValue,
    /// Current per turn, amperes (defaults to 0).
    #[serde(default = "zero")]
    pub current_a: ParamValue,
}

/// One material region in an [`AxisymSpec`] / [`PlanarSpec`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaterialSpec {
    /// Region it occupies.
    pub region: RectSpec,
    /// Relative permeability (initial permeability when saturable).
    pub mu_r: ParamValue,
    /// Saturation polarization J_s, tesla; absent = linear.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub js_t: Option<ParamValue>,
}

/// One magnet block in a [`PlanarSpec`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MagnetSpec {
    /// The block.
    pub region: RectSpec,
    /// Remanence x-component, tesla (defaults to 0).
    #[serde(default = "zero")]
    pub br_x_t: ParamValue,
    /// Remanence y-component, tesla (defaults to 0).
    #[serde(default = "zero")]
    pub br_y_t: ParamValue,
    /// Recoil permeability (defaults to 1).
    #[serde(default = "one")]
    pub mu_r: ParamValue,
}

/// One conductor in a [`PlanarSpec`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConductorSpec {
    /// Cross-section.
    pub region: RectSpec,
    /// Total current (turns × amps), A (defaults to 0).
    #[serde(default = "zero")]
    pub total_current_a: ParamValue,
}

/// Serializable axisymmetric magnetostatic device with named parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AxisymSpec {
    /// Domain outer radius, mm.
    pub r_max_mm: ParamValue,
    /// Domain lower z, mm.
    pub z_min_mm: ParamValue,
    /// Domain upper z, mm.
    pub z_max_mm: ParamValue,
    /// Coils.
    #[serde(default)]
    pub coils: Vec<CoilSpec>,
    /// Material regions.
    #[serde(default)]
    pub materials: Vec<MaterialSpec>,
    /// Boundary condition at r = r_max (default flux-excluded).
    #[serde(default = "bc_zero")]
    pub bc_r_outer: Bc,
    /// Boundary condition at z = z_min.
    #[serde(default = "bc_zero")]
    pub bc_z_low: Bc,
    /// Boundary condition at z = z_max.
    #[serde(default = "bc_zero")]
    pub bc_z_high: Bc,
}

impl AxisymSpec {
    /// Resolve every field against `params`, fail-closed.
    pub fn resolve(
        &self,
        params: &BTreeMap<String, f64>,
    ) -> Result<AxisymMagnetostatics, SpecError> {
        let mut dev = AxisymMagnetostatics::new(
            self.r_max_mm.resolve(params)?,
            self.z_min_mm.resolve(params)?,
            self.z_max_mm.resolve(params)?,
        );
        dev.bc_r_outer = self.bc_r_outer;
        dev.bc_z_low = self.bc_z_low;
        dev.bc_z_high = self.bc_z_high;
        for c in &self.coils {
            let (r0, r1, z0, z1) = c.region.resolve(params)?;
            dev.coils.push(Coil {
                region: Annulus {
                    r_inner_mm: r0,
                    r_outer_mm: r1,
                    z_min_mm: z0,
                    z_max_mm: z1,
                },
                turns: c.turns.resolve(params)?,
                current_a: c.current_a.resolve(params)?,
            });
        }
        for m in &self.materials {
            let (r0, r1, z0, z1) = m.region.resolve(params)?;
            let region = Annulus {
                r_inner_mm: r0,
                r_outer_mm: r1,
                z_min_mm: z0,
                z_max_mm: z1,
            };
            dev.materials.push(Material {
                region,
                mu_r: m.mu_r.resolve(params)?,
                sat: match &m.js_t {
                    None => None,
                    Some(js) => Some(Saturation {
                        js_t: js.resolve(params)?,
                    }),
                },
            });
        }
        Ok(dev)
    }

    /// A literal spec mirroring `device`.
    pub fn from_device(device: &AxisymMagnetostatics) -> Self {
        let rect = |a: &Annulus| RectSpec {
            x_min_mm: a.r_inner_mm.into(),
            x_max_mm: a.r_outer_mm.into(),
            y_min_mm: a.z_min_mm.into(),
            y_max_mm: a.z_max_mm.into(),
        };
        Self {
            r_max_mm: device.r_max_mm.into(),
            z_min_mm: device.z_min_mm.into(),
            z_max_mm: device.z_max_mm.into(),
            coils: device
                .coils
                .iter()
                .map(|c| CoilSpec {
                    region: rect(&c.region),
                    turns: c.turns.into(),
                    current_a: c.current_a.into(),
                })
                .collect(),
            materials: device
                .materials
                .iter()
                .map(|m| MaterialSpec {
                    region: rect(&m.region),
                    mu_r: m.mu_r.into(),
                    js_t: m.sat.map(|s| s.js_t.into()),
                })
                .collect(),
            bc_r_outer: device.bc_r_outer,
            bc_z_low: device.bc_z_low,
            bc_z_high: device.bc_z_high,
        }
    }

    /// Every named parameter with its gradient role. A name used by both
    /// an adjoint-capable field and an FD field is conservatively
    /// [`ParamRole::FiniteDifference`].
    pub fn parameter_roles(&self) -> BTreeMap<String, ParamRole> {
        let mut roles = RoleMap::default();
        for v in [&self.r_max_mm, &self.z_min_mm, &self.z_max_mm] {
            roles.put(v, ParamRole::FiniteDifference);
        }
        for c in &self.coils {
            c.region.collect_roles(&mut roles);
            roles.put(&c.turns, ParamRole::FiniteDifference);
            roles.put(&c.current_a, ParamRole::Adjoint);
        }
        for m in &self.materials {
            m.region.collect_roles(&mut roles);
            let mu_role = if m.js_t.is_some() {
                // Saturable: the frozen-ν adjoint refuses it — FD.
                ParamRole::FiniteDifference
            } else {
                ParamRole::Adjoint
            };
            roles.put(&m.mu_r, mu_role);
            if let Some(js) = &m.js_t {
                roles.put(js, ParamRole::FiniteDifference);
            }
        }
        roles.0
    }
}

/// Serializable planar magnetostatic device with named parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanarSpec {
    /// Domain left edge, mm.
    pub x_min_mm: ParamValue,
    /// Domain right edge, mm.
    pub x_max_mm: ParamValue,
    /// Domain bottom edge, mm.
    pub y_min_mm: ParamValue,
    /// Domain top edge, mm.
    pub y_max_mm: ParamValue,
    /// Conductors.
    #[serde(default)]
    pub conductors: Vec<ConductorSpec>,
    /// Magnets.
    #[serde(default)]
    pub magnets: Vec<MagnetSpec>,
    /// Material regions.
    #[serde(default)]
    pub materials: Vec<MaterialSpec>,
    /// Unrolled-machine wrap.
    #[serde(default)]
    pub periodic_x: bool,
    /// Boundary condition, left (ignored when periodic).
    #[serde(default = "bc_zero")]
    pub bc_x_low: Bc,
    /// Boundary condition, right (ignored when periodic).
    #[serde(default = "bc_zero")]
    pub bc_x_high: Bc,
    /// Boundary condition, bottom.
    #[serde(default = "bc_zero")]
    pub bc_y_low: Bc,
    /// Boundary condition, top.
    #[serde(default = "bc_zero")]
    pub bc_y_high: Bc,
}

impl PlanarSpec {
    /// Resolve every field against `params`, fail-closed.
    pub fn resolve(
        &self,
        params: &BTreeMap<String, f64>,
    ) -> Result<PlanarMagnetostatics, SpecError> {
        let mut dev = PlanarMagnetostatics::new(
            self.x_min_mm.resolve(params)?,
            self.x_max_mm.resolve(params)?,
            self.y_min_mm.resolve(params)?,
            self.y_max_mm.resolve(params)?,
        );
        dev.periodic_x = self.periodic_x;
        dev.bc_x_low = self.bc_x_low;
        dev.bc_x_high = self.bc_x_high;
        dev.bc_y_low = self.bc_y_low;
        dev.bc_y_high = self.bc_y_high;
        let rect = |r: &RectSpec, p: &BTreeMap<String, f64>| -> Result<Rect, SpecError> {
            let (x0, x1, y0, y1) = r.resolve(p)?;
            Ok(Rect {
                x_min_mm: x0,
                x_max_mm: x1,
                y_min_mm: y0,
                y_max_mm: y1,
            })
        };
        for c in &self.conductors {
            dev.conductors.push(Conductor {
                region: rect(&c.region, params)?,
                total_current_a: c.total_current_a.resolve(params)?,
            });
        }
        for m in &self.magnets {
            dev.magnets.push(MagnetBlock {
                region: rect(&m.region, params)?,
                br_x_t: m.br_x_t.resolve(params)?,
                br_y_t: m.br_y_t.resolve(params)?,
                mu_r: m.mu_r.resolve(params)?,
            });
        }
        for m in &self.materials {
            dev.materials.push(PlanarMaterial {
                region: rect(&m.region, params)?,
                mu_r: m.mu_r.resolve(params)?,
                sat: match &m.js_t {
                    None => None,
                    Some(js) => Some(Saturation {
                        js_t: js.resolve(params)?,
                    }),
                },
            });
        }
        Ok(dev)
    }

    /// A literal spec mirroring `device`.
    pub fn from_device(device: &PlanarMagnetostatics) -> Self {
        let rect = |r: &Rect| RectSpec {
            x_min_mm: r.x_min_mm.into(),
            x_max_mm: r.x_max_mm.into(),
            y_min_mm: r.y_min_mm.into(),
            y_max_mm: r.y_max_mm.into(),
        };
        Self {
            x_min_mm: device.x_min_mm.into(),
            x_max_mm: device.x_max_mm.into(),
            y_min_mm: device.y_min_mm.into(),
            y_max_mm: device.y_max_mm.into(),
            conductors: device
                .conductors
                .iter()
                .map(|c| ConductorSpec {
                    region: rect(&c.region),
                    total_current_a: c.total_current_a.into(),
                })
                .collect(),
            magnets: device
                .magnets
                .iter()
                .map(|m| MagnetSpec {
                    region: rect(&m.region),
                    br_x_t: m.br_x_t.into(),
                    br_y_t: m.br_y_t.into(),
                    mu_r: m.mu_r.into(),
                })
                .collect(),
            materials: device
                .materials
                .iter()
                .map(|m| MaterialSpec {
                    region: rect(&m.region),
                    mu_r: m.mu_r.into(),
                    js_t: m.sat.map(|s| s.js_t.into()),
                })
                .collect(),
            periodic_x: device.periodic_x,
            bc_x_low: device.bc_x_low,
            bc_x_high: device.bc_x_high,
            bc_y_low: device.bc_y_low,
            bc_y_high: device.bc_y_high,
        }
    }

    /// Every named parameter with its gradient role (conservative on
    /// name collisions, like [`AxisymSpec::parameter_roles`]).
    pub fn parameter_roles(&self) -> BTreeMap<String, ParamRole> {
        let mut roles = RoleMap::default();
        for v in [
            &self.x_min_mm,
            &self.x_max_mm,
            &self.y_min_mm,
            &self.y_max_mm,
        ] {
            roles.put(v, ParamRole::FiniteDifference);
        }
        for c in &self.conductors {
            c.region.collect_roles(&mut roles);
            roles.put(&c.total_current_a, ParamRole::Adjoint);
        }
        for m in &self.magnets {
            m.region.collect_roles(&mut roles);
            roles.put(&m.br_x_t, ParamRole::Adjoint);
            roles.put(&m.br_y_t, ParamRole::Adjoint);
            // Recoil μ is not exposed by the adjoint (magnet cells are
            // excluded from the material map) — FD.
            roles.put(&m.mu_r, ParamRole::FiniteDifference);
        }
        for m in &self.materials {
            m.region.collect_roles(&mut roles);
            let mu_role = if m.js_t.is_some() {
                ParamRole::FiniteDifference
            } else {
                ParamRole::Adjoint
            };
            roles.put(&m.mu_r, mu_role);
            if let Some(js) = &m.js_t {
                roles.put(js, ParamRole::FiniteDifference);
            }
        }
        roles.0
    }
}

#[derive(Default)]
struct RoleMap(BTreeMap<String, ParamRole>);

impl RoleMap {
    fn put(&mut self, value: &ParamValue, role: ParamRole) {
        if let Some(name) = value.name() {
            self.0
                .entry(name.to_string())
                .and_modify(|existing| {
                    if role == ParamRole::FiniteDifference {
                        *existing = ParamRole::FiniteDifference;
                    }
                })
                .or_insert(role);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::SolveOptions;

    fn params(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn axisym_json_round_trip_with_named_parameters() {
        let json = r#"{
            "r_max_mm": 60.0,
            "z_min_mm": -40.0,
            "z_max_mm": 40.0,
            "coils": [
                {
                    "region": {"x_min_mm": 20.0, "x_max_mm": 24.0,
                               "y_min_mm": "coil_z0", "y_max_mm": "coil_z1"},
                    "turns": 50.0,
                    "current_a": "drive"
                }
            ],
            "materials": [
                {
                    "region": {"x_min_mm": 0.0, "x_max_mm": 10.0,
                               "y_min_mm": -30.0, "y_max_mm": 30.0},
                    "mu_r": "core_mu"
                }
            ]
        }"#;
        let spec: AxisymSpec = serde_json::from_str(json).expect("parse");
        let dev = spec
            .resolve(&params(&[
                ("coil_z0", -12.0),
                ("coil_z1", -8.0),
                ("drive", 2.0),
                ("core_mu", 200.0),
            ]))
            .expect("resolve");
        assert_eq!(dev.coils[0].current_a, 2.0);
        assert_eq!(dev.materials[0].mu_r, 200.0);
        // The resolved device actually runs through the solver.
        let sol = dev.solve(21, 31, &SolveOptions::default()).expect("solve");
        assert!(sol.flux_linkage(0) > 0.0);

        // Serialize → reparse: identical spec.
        let round = serde_json::to_string(&spec).expect("serialize");
        let spec2: AxisymSpec = serde_json::from_str(&round).expect("reparse");
        assert_eq!(spec, spec2);
    }

    #[test]
    fn resolution_is_fail_closed() {
        let spec = AxisymSpec {
            r_max_mm: 60.0.into(),
            z_min_mm: (-40.0).into(),
            z_max_mm: 40.0.into(),
            coils: vec![CoilSpec {
                region: RectSpec {
                    x_min_mm: 20.0.into(),
                    x_max_mm: 24.0.into(),
                    y_min_mm: ParamValue::Named("missing".into()),
                    y_max_mm: 12.0.into(),
                },
                turns: 50.0.into(),
                current_a: 1.0.into(),
            }],
            materials: vec![],
            bc_r_outer: Bc::Zero,
            bc_z_low: Bc::Zero,
            bc_z_high: Bc::Zero,
        };
        let err = spec.resolve(&BTreeMap::new()).unwrap_err();
        assert_eq!(err, SpecError::UnknownParameter("missing".into()));
    }

    #[test]
    fn parameter_roles_mirror_the_adjoint_implementation() {
        let json = r#"{
            "x_min_mm": 0.0, "x_max_mm": "span", "y_min_mm": 0.0, "y_max_mm": 30.0,
            "conductors": [
                {"region": {"x_min_mm": 10.0, "x_max_mm": 14.0,
                            "y_min_mm": 8.0, "y_max_mm": 10.0},
                 "total_current_a": "i_a"}
            ],
            "magnets": [
                {"region": {"x_min_mm": "mag_x0", "x_max_mm": 25.0,
                            "y_min_mm": 18.0, "y_max_mm": 22.0},
                 "br_y_t": "br", "mu_r": "recoil"}
            ],
            "materials": [
                {"region": {"x_min_mm": 0.0, "x_max_mm": 40.0,
                            "y_min_mm": 0.0, "y_max_mm": 6.0},
                 "mu_r": "iron_mu"},
                {"region": {"x_min_mm": 0.0, "x_max_mm": 40.0,
                            "y_min_mm": 25.0, "y_max_mm": 30.0},
                 "mu_r": "ferrite_mu", "js_t": 0.45}
            ]
        }"#;
        let spec: PlanarSpec = serde_json::from_str(json).expect("parse");
        let roles = spec.parameter_roles();
        assert_eq!(roles["i_a"], ParamRole::Adjoint);
        assert_eq!(roles["br"], ParamRole::Adjoint);
        assert_eq!(roles["iron_mu"], ParamRole::Adjoint);
        // Geometry, recoil μ, and saturable μ are FD.
        assert_eq!(roles["span"], ParamRole::FiniteDifference);
        assert_eq!(roles["mag_x0"], ParamRole::FiniteDifference);
        assert_eq!(roles["recoil"], ParamRole::FiniteDifference);
        assert_eq!(roles["ferrite_mu"], ParamRole::FiniteDifference);

        // A name shared between an adjoint field and a geometric field
        // demotes to FD.
        let mut mixed = spec.clone();
        mixed.conductors[0].region.x_min_mm = ParamValue::Named("i_a".into());
        assert_eq!(mixed.parameter_roles()["i_a"], ParamRole::FiniteDifference);
    }

    #[test]
    fn from_device_round_trips_saturation_and_bcs() {
        let mut dev = AxisymMagnetostatics::new(40.0, 0.0, 100.0);
        dev.bc_r_outer = Bc::Neumann;
        dev.bc_z_low = Bc::Neumann;
        dev.bc_z_high = Bc::Neumann;
        dev.coils.push(Coil {
            region: Annulus {
                r_inner_mm: 20.0,
                r_outer_mm: 22.0,
                z_min_mm: 0.0,
                z_max_mm: 100.0,
            },
            turns: 1000.0,
            current_a: 0.5,
        });
        dev.materials
            .push(Material::saturable(dev.coils[0].region, 1000.0, 0.45));
        let spec = AxisymSpec::from_device(&dev);
        let json = serde_json::to_string(&spec).expect("serialize");
        assert!(json.contains("neumann"), "BCs must serialize: {json}");
        assert!(json.contains("js_t"), "saturation must serialize");
        let back: AxisymSpec = serde_json::from_str(&json).expect("parse");
        let resolved = back.resolve(&BTreeMap::new()).expect("resolve");
        assert_eq!(resolved, dev);
    }
}
