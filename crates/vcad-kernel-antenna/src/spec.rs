//! The `.vcad` seam: a serde antenna schema with named parameters.
//!
//! An [`AntennaSpec`] is the serialization contract between vcad documents
//! and this crate. Every numeric field is a [`ParamValue`]: either a
//! literal, or the **name** of a document parameter supplied at resolve
//! time. Resolution is fail-closed — an unbound name is an error, never a
//! default (the particle-crate discipline, verbatim).
//!
//! All named parameters here are geometric, so every one of them prices
//! its gradient through [`crate::adjoint::z_in_gradient`] (the adjoint
//! identity plus fill-level differences); there is no role split like the
//! particle crate's potentials-vs-geometry classification.
//!
//! Geometry extraction from BRep/board documents (outlines, trace paths)
//! deliberately lives on the vcad side of the seam, emitting this schema —
//! the same division of labor as `document_parameter_gradient`. The
//! PCB-trace equivalence rule itself is in [`crate::ecad`].

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::error::AntennaError;
use crate::geometry::{Mesh, WireGrid};

/// A literal value or the name of a document parameter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    /// A literal number.
    Literal(f64),
    /// The name of a parameter bound at [`AntennaSpec::resolve`] time.
    Named(String),
}

impl From<f64> for ParamValue {
    fn from(v: f64) -> Self {
        ParamValue::Literal(v)
    }
}

impl From<&str> for ParamValue {
    fn from(name: &str) -> Self {
        ParamValue::Named(name.to_string())
    }
}

impl ParamValue {
    fn resolve(&self, params: &BTreeMap<String, f64>) -> Result<f64, AntennaError> {
        match self {
            ParamValue::Literal(v) => Ok(*v),
            ParamValue::Named(name) => params
                .get(name)
                .copied()
                .ok_or_else(|| AntennaError::UnboundParameter { name: name.clone() }),
        }
    }

    fn collect(&self, out: &mut BTreeSet<String>) {
        if let ParamValue::Named(n) = self {
            out.insert(n.clone());
        }
    }
}

/// A point whose coordinates may be named parameters, mm.
pub type PointSpec = [ParamValue; 3];

fn resolve_point(p: &PointSpec, params: &BTreeMap<String, f64>) -> Result<[f64; 3], AntennaError> {
    Ok([
        p[0].resolve(params)?,
        p[1].resolve(params)?,
        p[2].resolve(params)?,
    ])
}

/// One wire element of the antenna.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ElementSpec {
    /// A straight wire.
    Wire {
        /// Start point, mm.
        start_mm: PointSpec,
        /// End point, mm.
        end_mm: PointSpec,
        /// Wire radius, mm.
        radius_mm: ParamValue,
        /// Number of segments.
        segments: usize,
    },
    /// An open polyline.
    Path {
        /// Waypoints, mm.
        points_mm: Vec<PointSpec>,
        /// Wire radius, mm.
        radius_mm: ParamValue,
        /// Segments per leg (`points − 1` entries).
        segments_per_leg: Vec<usize>,
    },
    /// A closed loop (last point connects back to the first).
    Loop {
        /// Waypoints, mm.
        points_mm: Vec<PointSpec>,
        /// Wire radius, mm.
        radius_mm: ParamValue,
        /// Segments per leg (`points` entries, closing leg included).
        segments_per_leg: Vec<usize>,
    },
}

/// Serializable antenna with named parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AntennaSpec {
    /// Perfect electric conductor at z = 0 (defaults to false).
    #[serde(default)]
    pub ground_plane: bool,
    /// Wire elements.
    pub elements: Vec<ElementSpec>,
    /// Delta-gap feed location, mm — resolves to the nearest basis node.
    pub feed_mm: PointSpec,
}

impl AntennaSpec {
    /// Every parameter name referenced anywhere in the spec.
    pub fn parameter_names(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let point = |p: &PointSpec, out: &mut BTreeSet<String>| {
            for c in p {
                c.collect(out);
            }
        };
        for e in &self.elements {
            match e {
                ElementSpec::Wire {
                    start_mm,
                    end_mm,
                    radius_mm,
                    ..
                } => {
                    point(start_mm, &mut out);
                    point(end_mm, &mut out);
                    radius_mm.collect(&mut out);
                }
                ElementSpec::Path {
                    points_mm,
                    radius_mm,
                    ..
                }
                | ElementSpec::Loop {
                    points_mm,
                    radius_mm,
                    ..
                } => {
                    for p in points_mm {
                        point(p, &mut out);
                    }
                    radius_mm.collect(&mut out);
                }
            }
        }
        point(&self.feed_mm, &mut out);
        out
    }

    /// Resolve every parameter (fail-closed), build the mesh, and locate
    /// the feed basis. Returns `(mesh, feed_basis)`.
    pub fn resolve(&self, params: &BTreeMap<String, f64>) -> Result<(Mesh, usize), AntennaError> {
        let mut grid = WireGrid::new();
        grid.set_ground_plane(self.ground_plane);
        for e in &self.elements {
            match e {
                ElementSpec::Wire {
                    start_mm,
                    end_mm,
                    radius_mm,
                    segments,
                } => {
                    grid.add_wire(
                        resolve_point(start_mm, params)?,
                        resolve_point(end_mm, params)?,
                        radius_mm.resolve(params)?,
                        *segments,
                    )?;
                }
                ElementSpec::Path {
                    points_mm,
                    radius_mm,
                    segments_per_leg,
                } => {
                    let pts: Vec<[f64; 3]> = points_mm
                        .iter()
                        .map(|p| resolve_point(p, params))
                        .collect::<Result<_, _>>()?;
                    grid.add_path(&pts, radius_mm.resolve(params)?, segments_per_leg)?;
                }
                ElementSpec::Loop {
                    points_mm,
                    radius_mm,
                    segments_per_leg,
                } => {
                    let pts: Vec<[f64; 3]> = points_mm
                        .iter()
                        .map(|p| resolve_point(p, params))
                        .collect::<Result<_, _>>()?;
                    grid.add_loop(&pts, radius_mm.resolve(params)?, segments_per_leg)?;
                }
            }
        }
        let mesh = Mesh::build(&grid)?;
        let feed = mesh.nearest_basis(resolve_point(&self.feed_mm, params)?)?;
        Ok((mesh, feed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mom::{find_resonance, SolveOptions};

    fn dipole_spec() -> AntennaSpec {
        AntennaSpec {
            ground_plane: false,
            elements: vec![ElementSpec::Wire {
                start_mm: [0.0.into(), 0.0.into(), ParamValue::Named("half_len".into())],
                end_mm: [
                    0.0.into(),
                    0.0.into(),
                    ParamValue::Named("neg_half_len".into()),
                ],
                radius_mm: 1.0.into(),
                segments: 20,
            }],
            feed_mm: [0.0.into(), 0.0.into(), 0.0.into()],
        }
    }

    #[test]
    fn json_round_trip_preserves_the_spec() {
        let spec = dipole_spec();
        let json = serde_json::to_string_pretty(&spec).unwrap();
        // Named parameters serialize as bare strings, literals as numbers.
        assert!(json.contains("\"half_len\""));
        assert!(json.contains("\"type\": \"wire\""));
        let back: AntennaSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, back);
    }

    #[test]
    fn unbound_parameter_fails_closed() {
        let spec = dipole_spec();
        let mut params = BTreeMap::new();
        params.insert("half_len".to_string(), 500.0);
        // neg_half_len missing → error, never a default.
        match spec.resolve(&params) {
            Err(AntennaError::UnboundParameter { name }) => {
                assert_eq!(name, "neg_half_len");
            }
            other => panic!("expected unbound-parameter error, got {other:?}"),
        }
    }

    #[test]
    fn parameter_names_are_collected() {
        let names = dipole_spec().parameter_names();
        assert_eq!(
            names.into_iter().collect::<Vec<_>>(),
            vec!["half_len".to_string(), "neg_half_len".to_string()]
        );
    }

    #[test]
    fn spec_built_dipole_matches_the_direct_build() {
        let spec = dipole_spec();
        let mut params = BTreeMap::new();
        params.insert("half_len".to_string(), 500.0);
        params.insert("neg_half_len".to_string(), -500.0);
        let (mesh, feed) = spec.resolve(&params).unwrap();
        assert_eq!(mesh.segments.len(), 20);
        let opts = SolveOptions::default();
        let f_half = crate::constants::C0 / 2.0;
        let f_res = find_resonance(&mesh, feed, 0.8 * f_half, 1.05 * f_half, &opts).unwrap();
        // Same resonance as the directly-built 1 m dipole (M0 tests).
        let l_over_lambda = 1.0 * f_res / crate::constants::C0;
        assert!((0.46..=0.49).contains(&l_over_lambda));
    }
}
