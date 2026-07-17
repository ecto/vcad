//! The `.vcad` seam: a serde thermal-problem schema with named parameters.
//!
//! A [`ThermalSpec`] is the serialization contract between vcad documents
//! and this crate. Every physical number is a [`ParamValue`]: a literal,
//! or the **name** of a document parameter supplied at resolve time.
//! Resolution is fail-closed — an unbound name is an error, never a
//! default (the particle-crate contract, verbatim).
//!
//! [`ThermalSpec::parameter_roles`] classifies each named parameter by how
//! its gradient is obtained ([`ParamRole`]): conductivities, film
//! coefficients, and source powers flow through the discrete adjoint
//! ([`crate::adjoint::smooth_max_gradient`]); geometry moves the voxel
//! material mask, which is discrete, so geometric parameters take finite
//! differences. Temperatures (ambients, reservoirs, the θ reference) are
//! adjoint-*capable* — they enter the right-hand side linearly — but that
//! path is not wired yet, so they are conservatively classified
//! finite-difference until it is.
//!
//! **Geometry in, two ways.** Region shapes cover the parametric case;
//! the [`crate::model::VoxelMaterials`] pass-through carries externally
//! voxelized *tessellated parts*. The voxelizer itself deliberately lands
//! on the vcad side of the seam (sample voxel centers with the kernel's
//! point-in-solid machinery, emit indices into the material table) — the
//! same division of labor as the particle crate's BRep-to-rings
//! extraction. Voxel indices are data, not parameters.
//!
//! **MaterialCard hookup (documented, honestly).** The atoms-side
//! homogenization (`vcad-kernel-atoms::homogenize::MaterialCard`) today
//! carries **density and elastic constants only** — no thermal
//! conductivity, no specific heat. The intended mapping when that
//! extension lands: `k_w_mk` from a phonon/Green–Kubo conductivity
//! calculation, `heat_capacity_j_m3k` = `density_kg_m3` × c_p (c_p from a
//! phonon DOS or a tabulated value). Until then, thermal properties come
//! from handbooks; wiring a card field that does not exist would be a
//! silent default, which this crate does not do.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::{
    Axis, Boundary, FixedTemperature, MaterialRegion, PowerSource, Shape, ThermalModel,
    VoxelMaterials,
};

/// A literal value or the name of a document parameter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    /// A literal number.
    Literal(f64),
    /// The name of a parameter bound at [`ThermalSpec::resolve`] time.
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

/// How a named parameter's gradient is computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParamRole {
    /// Reverse-mode via [`crate::adjoint::smooth_max_gradient`]
    /// (conductivity components, film coefficients, source powers).
    Adjoint,
    /// Finite differences (geometry moves the discrete material mask;
    /// temperatures are adjoint-capable but not yet wired).
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
                write!(f, "unbound thermal parameter: {name:?}")
            }
        }
    }
}

impl std::error::Error for SpecError {}

/// Serializable [`Shape`] with parameterizable dimensions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ShapeSpec {
    /// Axis-aligned box.
    Box {
        /// Minimum corner, mm.
        min_mm: [ParamValue; 3],
        /// Extent per axis, mm.
        size_mm: [ParamValue; 3],
    },
    /// Axis-aligned tube (cylinder when the inner radius is 0).
    Tube {
        /// Tube axis.
        axis: Axis,
        /// Cross-axis center, mm (ascending axis order).
        center_mm: [ParamValue; 2],
        /// `[lo, hi]` extent along the axis, mm.
        span_mm: [ParamValue; 2],
        /// Outer radius, mm.
        outer_radius_mm: ParamValue,
        /// Inner radius, mm.
        inner_radius_mm: ParamValue,
    },
}

/// Serializable material region.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaterialSpec {
    /// Region shape.
    pub shape: ShapeSpec,
    /// Per-axis conductivity, W/(m·K).
    pub k_w_mk: [ParamValue; 3],
    /// Volumetric heat capacity ρc_p, J/(m³·K) (transient solves only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heat_capacity_j_m3k: Option<ParamValue>,
}

/// Serializable power source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceSpec {
    /// Source name.
    pub name: String,
    /// Deposit region.
    pub shape: ShapeSpec,
    /// Total power, W.
    pub power_w: ParamValue,
}

/// Serializable fixed-temperature reservoir.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FixedSpec {
    /// Pinned region.
    pub shape: ShapeSpec,
    /// Pinned temperature, °C.
    pub temperature_c: ParamValue,
}

/// Serializable boundary condition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BoundarySpec {
    /// No heat crosses the face.
    Adiabatic,
    /// Dirichlet surface.
    FixedTemperature {
        /// Surface temperature, °C.
        temperature_c: ParamValue,
    },
    /// Robin surface (film + ambient).
    Convection {
        /// Film coefficient, W/(m²·K).
        h_w_m2k: ParamValue,
        /// Ambient temperature, °C.
        ambient_c: ParamValue,
    },
}

fn adiabatic() -> [BoundarySpec; 6] {
    [
        BoundarySpec::Adiabatic,
        BoundarySpec::Adiabatic,
        BoundarySpec::Adiabatic,
        BoundarySpec::Adiabatic,
        BoundarySpec::Adiabatic,
        BoundarySpec::Adiabatic,
    ]
}

fn adiabatic_one() -> BoundarySpec {
    BoundarySpec::Adiabatic
}

/// Serializable thermal problem with named parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThermalSpec {
    /// Domain minimum corner, mm.
    pub origin_mm: [ParamValue; 3],
    /// Domain extent, mm.
    pub size_mm: [ParamValue; 3],
    /// Voxel counts (data, not parameters — the grid is provenance).
    pub divisions: [usize; 3],
    /// Material regions.
    pub materials: Vec<MaterialSpec>,
    /// Power sources.
    #[serde(default)]
    pub sources: Vec<SourceSpec>,
    /// Fixed-temperature reservoirs.
    #[serde(default)]
    pub fixed: Vec<FixedSpec>,
    /// Domain-face boundary conditions, `[-x, +x, -y, +y, -z, +z]`.
    #[serde(default = "adiabatic")]
    pub domain_faces: [BoundarySpec; 6],
    /// Exposed solid↔void boundary condition.
    #[serde(default = "adiabatic_one")]
    pub exposed: BoundarySpec,
    /// θ reference temperature, °C.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_c: Option<ParamValue>,
    /// Externally voxelized materials (tessellated-part seam), passed
    /// through verbatim — indices are data, not parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voxel_materials: Option<VoxelMaterials>,
}

impl ShapeSpec {
    fn resolve(&self, params: &BTreeMap<String, f64>) -> Result<Shape, SpecError> {
        Ok(match self {
            ShapeSpec::Box { min_mm, size_mm } => Shape::Box {
                min_mm: resolve3(min_mm, params)?,
                size_mm: resolve3(size_mm, params)?,
            },
            ShapeSpec::Tube {
                axis,
                center_mm,
                span_mm,
                outer_radius_mm,
                inner_radius_mm,
            } => Shape::Tube {
                axis: *axis,
                center_mm: [center_mm[0].resolve(params)?, center_mm[1].resolve(params)?],
                span_mm: [span_mm[0].resolve(params)?, span_mm[1].resolve(params)?],
                outer_radius_mm: outer_radius_mm.resolve(params)?,
                inner_radius_mm: inner_radius_mm.resolve(params)?,
            },
        })
    }

    fn from_shape(shape: &Shape) -> Self {
        match shape {
            Shape::Box { min_mm, size_mm } => ShapeSpec::Box {
                min_mm: lit3(min_mm),
                size_mm: lit3(size_mm),
            },
            Shape::Tube {
                axis,
                center_mm,
                span_mm,
                outer_radius_mm,
                inner_radius_mm,
            } => ShapeSpec::Tube {
                axis: *axis,
                center_mm: [center_mm[0].into(), center_mm[1].into()],
                span_mm: [span_mm[0].into(), span_mm[1].into()],
                outer_radius_mm: (*outer_radius_mm).into(),
                inner_radius_mm: (*inner_radius_mm).into(),
            },
        }
    }

    fn names(&self) -> Vec<&str> {
        match self {
            ShapeSpec::Box { min_mm, size_mm } => min_mm
                .iter()
                .chain(size_mm.iter())
                .filter_map(ParamValue::name)
                .collect(),
            ShapeSpec::Tube {
                center_mm,
                span_mm,
                outer_radius_mm,
                inner_radius_mm,
                ..
            } => center_mm
                .iter()
                .chain(span_mm.iter())
                .chain([outer_radius_mm, inner_radius_mm])
                .filter_map(ParamValue::name)
                .collect(),
        }
    }
}

fn resolve3(v: &[ParamValue; 3], params: &BTreeMap<String, f64>) -> Result<[f64; 3], SpecError> {
    Ok([
        v[0].resolve(params)?,
        v[1].resolve(params)?,
        v[2].resolve(params)?,
    ])
}

fn lit3(v: &[f64; 3]) -> [ParamValue; 3] {
    [v[0].into(), v[1].into(), v[2].into()]
}

impl BoundarySpec {
    fn resolve(&self, params: &BTreeMap<String, f64>) -> Result<Boundary, SpecError> {
        Ok(match self {
            BoundarySpec::Adiabatic => Boundary::Adiabatic,
            BoundarySpec::FixedTemperature { temperature_c } => Boundary::FixedTemperature {
                temperature_c: temperature_c.resolve(params)?,
            },
            BoundarySpec::Convection { h_w_m2k, ambient_c } => Boundary::Convection {
                h_w_m2k: h_w_m2k.resolve(params)?,
                ambient_c: ambient_c.resolve(params)?,
            },
        })
    }

    fn from_boundary(b: &Boundary) -> Self {
        match b {
            Boundary::Adiabatic => BoundarySpec::Adiabatic,
            Boundary::FixedTemperature { temperature_c } => BoundarySpec::FixedTemperature {
                temperature_c: (*temperature_c).into(),
            },
            Boundary::Convection { h_w_m2k, ambient_c } => BoundarySpec::Convection {
                h_w_m2k: (*h_w_m2k).into(),
                ambient_c: (*ambient_c).into(),
            },
        }
    }
}

impl ThermalSpec {
    /// Resolve every field against `params`, fail-closed.
    pub fn resolve(&self, params: &BTreeMap<String, f64>) -> Result<ThermalModel, SpecError> {
        Ok(ThermalModel {
            origin_mm: resolve3(&self.origin_mm, params)?,
            size_mm: resolve3(&self.size_mm, params)?,
            divisions: self.divisions,
            materials: self
                .materials
                .iter()
                .map(|m| {
                    Ok(MaterialRegion {
                        shape: m.shape.resolve(params)?,
                        k_w_mk: resolve3(&m.k_w_mk, params)?,
                        heat_capacity_j_m3k: m
                            .heat_capacity_j_m3k
                            .as_ref()
                            .map(|c| c.resolve(params))
                            .transpose()?,
                    })
                })
                .collect::<Result<Vec<_>, SpecError>>()?,
            sources: self
                .sources
                .iter()
                .map(|s| {
                    Ok(PowerSource {
                        name: s.name.clone(),
                        shape: s.shape.resolve(params)?,
                        power_w: s.power_w.resolve(params)?,
                    })
                })
                .collect::<Result<Vec<_>, SpecError>>()?,
            fixed: self
                .fixed
                .iter()
                .map(|fx| {
                    Ok(FixedTemperature {
                        shape: fx.shape.resolve(params)?,
                        temperature_c: fx.temperature_c.resolve(params)?,
                    })
                })
                .collect::<Result<Vec<_>, SpecError>>()?,
            domain_faces: {
                let mut faces = [Boundary::Adiabatic; 6];
                for (slot, spec) in self.domain_faces.iter().enumerate() {
                    faces[slot] = spec.resolve(params)?;
                }
                faces
            },
            exposed: self.exposed.resolve(params)?,
            reference_c: self
                .reference_c
                .as_ref()
                .map(|r| r.resolve(params))
                .transpose()?,
            voxel_materials: self.voxel_materials.clone(),
        })
    }

    /// A literal (parameter-free) spec mirroring `model` — the round-trip
    /// starting point for documents that don't parameterize.
    pub fn from_model(model: &ThermalModel) -> Self {
        Self {
            origin_mm: lit3(&model.origin_mm),
            size_mm: lit3(&model.size_mm),
            divisions: model.divisions,
            materials: model
                .materials
                .iter()
                .map(|m| MaterialSpec {
                    shape: ShapeSpec::from_shape(&m.shape),
                    k_w_mk: lit3(&m.k_w_mk),
                    heat_capacity_j_m3k: m.heat_capacity_j_m3k.map(Into::into),
                })
                .collect(),
            sources: model
                .sources
                .iter()
                .map(|s| SourceSpec {
                    name: s.name.clone(),
                    shape: ShapeSpec::from_shape(&s.shape),
                    power_w: s.power_w.into(),
                })
                .collect(),
            fixed: model
                .fixed
                .iter()
                .map(|fx| FixedSpec {
                    shape: ShapeSpec::from_shape(&fx.shape),
                    temperature_c: fx.temperature_c.into(),
                })
                .collect(),
            domain_faces: {
                let mut faces = adiabatic();
                for (slot, b) in model.domain_faces.iter().enumerate() {
                    faces[slot] = BoundarySpec::from_boundary(b);
                }
                faces
            },
            exposed: BoundarySpec::from_boundary(&model.exposed),
            reference_c: model.reference_c.map(Into::into),
            voxel_materials: model.voxel_materials.clone(),
        }
    }

    /// Every named parameter with its gradient role. A name used by both
    /// an adjoint-capable field and any other field is conservatively
    /// classified [`ParamRole::FiniteDifference`].
    pub fn parameter_roles(&self) -> BTreeMap<String, ParamRole> {
        let mut roles: BTreeMap<String, ParamRole> = BTreeMap::new();
        let mut put = |name: Option<&str>, role: ParamRole| {
            if let Some(name) = name {
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
        for v in self.origin_mm.iter().chain(self.size_mm.iter()) {
            put(v.name(), ParamRole::FiniteDifference);
        }
        for m in &self.materials {
            for n in m.shape.names() {
                put(Some(n), ParamRole::FiniteDifference);
            }
            for k in &m.k_w_mk {
                put(k.name(), ParamRole::Adjoint);
            }
            if let Some(c) = &m.heat_capacity_j_m3k {
                put(c.name(), ParamRole::FiniteDifference);
            }
        }
        for s in &self.sources {
            for n in s.shape.names() {
                put(Some(n), ParamRole::FiniteDifference);
            }
            put(s.power_w.name(), ParamRole::Adjoint);
        }
        for fx in &self.fixed {
            for n in fx.shape.names() {
                put(Some(n), ParamRole::FiniteDifference);
            }
            put(fx.temperature_c.name(), ParamRole::FiniteDifference);
        }
        for bc in self.domain_faces.iter().chain([&self.exposed]) {
            match bc {
                BoundarySpec::Adiabatic => {}
                BoundarySpec::FixedTemperature { temperature_c } => {
                    put(temperature_c.name(), ParamRole::FiniteDifference);
                }
                BoundarySpec::Convection { h_w_m2k, ambient_c } => {
                    put(h_w_m2k.name(), ParamRole::Adjoint);
                    put(ambient_c.name(), ParamRole::FiniteDifference);
                }
            }
        }
        if let Some(r) = &self.reference_c {
            put(r.name(), ParamRole::FiniteDifference);
        }
        roles
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solve::{solve_steady, SolveOptions};

    fn params(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn json_round_trip_with_named_parameters() {
        let json = r#"{
            "origin_mm": [0.0, 0.0, 0.0],
            "size_mm": ["board_w", "board_w", 1.6],
            "divisions": [20, 20, 2],
            "materials": [
                {
                    "shape": {"type": "Box", "min_mm": [0.0, 0.0, 0.0],
                              "size_mm": ["board_w", "board_w", 1.6]},
                    "k_w_mk": ["k_plane", "k_plane", "k_z"]
                }
            ],
            "sources": [
                {"name": "die",
                 "shape": {"type": "Box", "min_mm": [15.0, 15.0, 0.0],
                           "size_mm": [10.0, 10.0, 1.6]},
                 "power_w": "p_die"}
            ],
            "domain_faces": [
                {"type": "Adiabatic"}, {"type": "Adiabatic"},
                {"type": "Adiabatic"}, {"type": "Adiabatic"},
                {"type": "Convection", "h_w_m2k": "h_air", "ambient_c": 25.0},
                {"type": "Convection", "h_w_m2k": "h_air", "ambient_c": 25.0}
            ],
            "exposed": {"type": "Adiabatic"}
        }"#;
        let spec: ThermalSpec = serde_json::from_str(json).expect("parse");
        let model = spec
            .resolve(&params(&[
                ("board_w", 40.0),
                ("k_plane", 15.0),
                ("k_z", 0.5),
                ("p_die", 2.0),
                ("h_air", 12.0),
            ]))
            .expect("resolve");
        assert_eq!(model.size_mm, [40.0, 40.0, 1.6]);
        assert_eq!(model.materials[0].k_w_mk, [15.0, 15.0, 0.5]);
        assert_eq!(model.sources[0].power_w, 2.0);
        // And it actually solves.
        let sol = solve_steady(&model, &SolveOptions::default()).expect("solve");
        assert!(sol.t_max_c > 25.0);

        // Serialize → reparse: identical spec.
        let round = serde_json::to_string(&spec).expect("serialize");
        let spec2: ThermalSpec = serde_json::from_str(&round).expect("reparse");
        assert_eq!(spec, spec2);
    }

    #[test]
    fn resolution_is_fail_closed() {
        let json = r#"{
            "origin_mm": [0.0, 0.0, 0.0],
            "size_mm": [10.0, 10.0, "missing_height"],
            "divisions": [2, 2, 2],
            "materials": []
        }"#;
        let spec: ThermalSpec = serde_json::from_str(json).expect("parse");
        let err = spec.resolve(&BTreeMap::new()).unwrap_err();
        assert_eq!(err, SpecError::UnknownParameter("missing_height".into()));
    }

    #[test]
    fn from_model_round_trips_and_solves() {
        let mut m = ThermalModel::new([0.0, 0.0, 0.0], [20.0, 5.0, 5.0], [8, 1, 1]);
        m.materials.push(
            MaterialRegion::isotropic(
                Shape::Box {
                    min_mm: [0.0, 0.0, 0.0],
                    size_mm: [20.0, 5.0, 5.0],
                },
                50.0,
            )
            .with_heat_capacity(2.4e6),
        );
        m.domain_faces[0] = Boundary::FixedTemperature {
            temperature_c: 80.0,
        };
        m.domain_faces[1] = Boundary::Convection {
            h_w_m2k: 100.0,
            ambient_c: 20.0,
        };
        let spec = ThermalSpec::from_model(&m);
        let resolved = spec.resolve(&BTreeMap::new()).expect("literal resolve");
        assert_eq!(m, resolved);
        let sol = solve_steady(&resolved, &SolveOptions::default()).expect("solve");
        assert!(sol.t_max_c <= 80.0 + 1e-9);
    }

    #[test]
    fn parameter_roles_classify_the_gradient_path() {
        let json = r#"{
            "origin_mm": [0.0, 0.0, 0.0],
            "size_mm": ["w", 10.0, 10.0],
            "divisions": [4, 4, 4],
            "materials": [
                {"shape": {"type": "Box", "min_mm": [0.0, 0.0, 0.0],
                           "size_mm": ["w", 10.0, 10.0]},
                 "k_w_mk": ["k_cu", "k_cu", "k_z"],
                 "heat_capacity_j_m3k": "rc"}
            ],
            "sources": [
                {"name": "u1",
                 "shape": {"type": "Tube", "axis": "Z", "center_mm": ["cx", 5.0],
                           "span_mm": [0.0, 10.0], "outer_radius_mm": "r_die",
                           "inner_radius_mm": 0.0},
                 "power_w": "p1"}
            ],
            "domain_faces": [
                {"type": "Convection", "h_w_m2k": "h_top", "ambient_c": "t_amb"},
                {"type": "Adiabatic"}, {"type": "Adiabatic"}, {"type": "Adiabatic"},
                {"type": "Adiabatic"}, {"type": "Adiabatic"}
            ],
            "exposed": {"type": "Adiabatic"}
        }"#;
        let spec: ThermalSpec = serde_json::from_str(json).expect("parse");
        let roles = spec.parameter_roles();
        assert_eq!(roles["k_cu"], ParamRole::Adjoint);
        assert_eq!(roles["k_z"], ParamRole::Adjoint);
        assert_eq!(roles["p1"], ParamRole::Adjoint);
        assert_eq!(roles["h_top"], ParamRole::Adjoint);
        assert_eq!(roles["w"], ParamRole::FiniteDifference);
        assert_eq!(roles["cx"], ParamRole::FiniteDifference);
        assert_eq!(roles["r_die"], ParamRole::FiniteDifference);
        assert_eq!(roles["t_amb"], ParamRole::FiniteDifference);
        assert_eq!(roles["rc"], ParamRole::FiniteDifference);

        // A name shared by an adjoint field and a geometric field is
        // conservatively FD.
        let mut mixed = spec.clone();
        if let ShapeSpec::Box { size_mm, .. } = &mut mixed.materials[0].shape {
            size_mm[1] = ParamValue::Named("k_cu".into());
        }
        assert_eq!(mixed.parameter_roles()["k_cu"], ParamRole::FiniteDifference);
    }

    #[test]
    fn voxel_materials_pass_through_and_solve() {
        // A 4×1×1 bar voxelized externally: two copper voxels, one FR4,
        // one void — the tessellated-part seam end to end.
        let mut m = ThermalModel::new([0.0, 0.0, 0.0], [4.0, 1.0, 1.0], [4, 1, 1]);
        m.materials.push(MaterialRegion::isotropic(
            Shape::Box {
                min_mm: [0.0, 0.0, 0.0],
                size_mm: [4.0, 1.0, 1.0],
            },
            400.0,
        ));
        m.materials.push(MaterialRegion::isotropic(
            Shape::Box {
                min_mm: [0.0, 0.0, 0.0],
                size_mm: [4.0, 1.0, 1.0],
            },
            0.3,
        ));
        m.voxel_materials = Some(VoxelMaterials {
            material_index: vec![0, 0, 1, -1],
        });
        m.domain_faces[0] = Boundary::FixedTemperature {
            temperature_c: 100.0,
        };
        m.exposed = Boundary::Convection {
            h_w_m2k: 50.0,
            ambient_c: 20.0,
        };

        let spec = ThermalSpec::from_model(&m);
        let json = serde_json::to_string(&spec).expect("serialize");
        let back: ThermalSpec = serde_json::from_str(&json).expect("reparse");
        let resolved = back.resolve(&BTreeMap::new()).expect("resolve");
        assert_eq!(resolved, m);

        let sol = solve_steady(&resolved, &SolveOptions::default()).expect("solve");
        // The void voxel is not solid; the FR4 voxel's right face is
        // exposed and convects.
        assert!(sol.solid[0] && sol.solid[1] && sol.solid[2]);
        assert!(!sol.solid[3]);
        assert!(sol.t_c[3].is_nan());
        assert!(sol.t_max_c <= 100.0 + 1e-9);
    }
}
