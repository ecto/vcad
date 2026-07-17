//! The `.vcad` seam (M3): a serde shield schema with named parameters.
//!
//! A [`ShieldSpec`] is the serialization contract between vcad documents
//! and this crate. Every numeric field is a [`ParamValue`]: a literal or
//! the **name** of a document parameter supplied at resolve time.
//! Resolution is **fail-closed**: an unbound name, an unknown material,
//! a source energy outside the group structure, a detector outside the
//! geometry or overlapping another — each is an error, never a default.
//!
//! Detectors are declared as radii; the resolver *builds* the tally
//! regions by splitting the containing layer around each detector radius
//! (thin-shell track-length detectors, the M0 estimator), and returns
//! the label → region mapping. The BRep side of the seam (extracting a
//! layer stack from a revolved shield solid) lands on the vcad side,
//! emitting this schema — same division of labor as the particle crate.
//!
//! Parameter roles ([`ShieldSpec::parameter_roles`]): thickness
//! parameters ride the M2 diffusion-adjoint gradient
//! ([`d_dose_d_param_via_diffusion`]); the source rate is exactly linear
//! (the response is per source neutron); the source energy is
//! group-discrete and has no smooth gradient.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::diffusion::DiffusionOptions;
use crate::geometry::{Geometry, Layer};
use crate::groups::group_of_energy_ev;
use crate::materials;
use crate::tally::Estimate;
use crate::transport::{run, ConfigError, RunConfig, RunResult, Source};

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
                .ok_or_else(|| SpecError::UnboundParameter(name.clone())),
        }
    }

    fn name(&self) -> Option<&str> {
        match self {
            ParamValue::Literal(_) => None,
            ParamValue::Named(n) => Some(n),
        }
    }
}

/// One shield layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerSpec {
    /// Library material name (see [`materials::by_name`]).
    pub material: String,
    /// Radial thickness, mm.
    pub thickness_mm: ParamValue,
}

/// The neutron source (isotropic point at the center).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceSpec {
    /// Emission rate, n/s.
    pub rate_n_per_s: ParamValue,
    /// Line energy, eV (mapped to its group; outside the structure =
    /// error).
    pub energy_ev: ParamValue,
}

fn default_half_width() -> ParamValue {
    ParamValue::Literal(20.0)
}

/// A dose detector: a thin spherical tally shell at a radius.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetectorSpec {
    /// Label carried onto claims ("operator", "fence-line", …).
    pub label: String,
    /// Shell center radius, mm.
    pub radius_mm: ParamValue,
    /// Shell half-width, mm (default 20 — a 4 cm tally shell).
    #[serde(default = "default_half_width")]
    pub half_width_mm: ParamValue,
}

fn default_histories() -> usize {
    20_000
}
fn default_batches() -> usize {
    20
}
fn default_seed() -> u64 {
    20260717
}

/// Monte Carlo run parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSpec {
    /// Histories per batch.
    #[serde(default = "default_histories")]
    pub histories_per_batch: usize,
    /// Batch count.
    #[serde(default = "default_batches")]
    pub batches: usize,
    /// RNG seed.
    #[serde(default = "default_seed")]
    pub seed: u64,
}

impl Default for RunSpec {
    fn default() -> Self {
        RunSpec {
            histories_per_batch: default_histories(),
            batches: default_batches(),
            seed: default_seed(),
        }
    }
}

/// The serializable shield problem: spherical layer stack, central
/// point source, dose detectors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShieldSpec {
    /// Layer stack from r = 0 outward.
    pub layers: Vec<LayerSpec>,
    /// The source.
    pub source: SourceSpec,
    /// Detectors (≥ 1 required — a shield spec with nothing to protect
    /// is a bookkeeping bug).
    pub detectors: Vec<DetectorSpec>,
    /// Monte Carlo parameters.
    #[serde(default)]
    pub run: RunSpec,
}

/// Resolution / evaluation failures. Fail-closed, every one.
#[derive(Debug, Clone, PartialEq)]
pub enum SpecError {
    /// A named parameter had no binding.
    UnboundParameter(String),
    /// A material name not in the library.
    UnknownMaterial(String),
    /// Source energy outside the group structure.
    EnergyOutsideStructure {
        /// The rejected energy, eV.
        energy_ev: f64,
    },
    /// Source rate must be positive and finite.
    BadSourceRate(f64),
    /// No detectors declared.
    NoDetectors,
    /// A detector shell does not fit strictly inside one layer.
    DetectorDoesNotFit {
        /// Detector label.
        label: String,
    },
    /// Two detector shells overlap.
    DetectorOverlap {
        /// First label.
        a: String,
        /// Second label.
        b: String,
    },
    /// The transport config was rejected downstream.
    Config(String),
    /// Gradient requested for a parameter that is not a thickness.
    NotAThicknessParameter(String),
    /// Gradient requested for the outermost layer.
    NoNextLayer(String),
}

impl std::fmt::Display for SpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpecError::UnboundParameter(n) => write!(f, "unbound shield parameter: {n:?}"),
            SpecError::UnknownMaterial(m) => write!(f, "unknown material: {m:?}"),
            SpecError::EnergyOutsideStructure { energy_ev } => {
                write!(
                    f,
                    "source energy {energy_ev} eV outside the group structure"
                )
            }
            SpecError::BadSourceRate(r) => write!(f, "source rate {r} n/s must be positive"),
            SpecError::NoDetectors => write!(f, "a shield spec needs at least one detector"),
            SpecError::DetectorDoesNotFit { label } => {
                write!(
                    f,
                    "detector {label:?} does not fit strictly inside one layer"
                )
            }
            SpecError::DetectorOverlap { a, b } => {
                write!(f, "detector shells {a:?} and {b:?} overlap")
            }
            SpecError::Config(e) => write!(f, "transport config rejected: {e}"),
            SpecError::NotAThicknessParameter(n) => {
                write!(
                    f,
                    "parameter {n:?} is not a layer thickness — no smooth gradient"
                )
            }
            SpecError::NoNextLayer(n) => write!(
                f,
                "thickness parameter {n:?} is on the outermost layer — nothing to grow into"
            ),
        }
    }
}

impl std::error::Error for SpecError {}

/// How a named parameter's gradient is obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamRole {
    /// Layer thickness: M2 diffusion-adjoint interface derivative.
    ThicknessAdjoint {
        /// Spec layer index.
        layer: usize,
    },
    /// Source rate: the response is exactly linear in it.
    SourceLinear,
    /// Source energy: group-discrete, no smooth gradient.
    Discrete,
    /// Detector placement: geometric, finite differences if needed.
    DetectorGeometry,
}

/// A resolved, runnable shield problem.
#[derive(Debug)]
pub struct ResolvedShield {
    /// The transport config (geometry with detector shells realized).
    pub config: RunConfig,
    /// Detector label → region index in the resolved geometry.
    pub detector_regions: Vec<(String, usize)>,
    /// Source rate, n/s.
    pub source_rate_n_per_s: f64,
    /// Spec layer index → region index of that layer's outermost
    /// sub-region (the face that thickness growth moves).
    pub outer_region_of_layer: Vec<usize>,
}

impl ShieldSpec {
    /// Resolve against parameter bindings, fail-closed.
    pub fn resolve(&self, params: &BTreeMap<String, f64>) -> Result<ResolvedShield, SpecError> {
        if self.detectors.is_empty() {
            return Err(SpecError::NoDetectors);
        }
        let rate = self.source.rate_n_per_s.resolve(params)?;
        if !(rate.is_finite() && rate > 0.0) {
            return Err(SpecError::BadSourceRate(rate));
        }
        let energy_ev = self.source.energy_ev.resolve(params)?;
        let source_group =
            group_of_energy_ev(energy_ev).ok_or(SpecError::EnergyOutsideStructure { energy_ev })?;

        // Resolve layers.
        let mut layers: Vec<(Material, f64)> = Vec::new();
        use crate::materials::Material;
        for l in &self.layers {
            let m = materials::by_name(&l.material)
                .ok_or_else(|| SpecError::UnknownMaterial(l.material.clone()))?;
            layers.push((m, l.thickness_mm.resolve(params)?));
        }

        // Resolve detectors, sorted by radius.
        let mut dets: Vec<(String, f64, f64)> = Vec::new();
        for d in &self.detectors {
            dets.push((
                d.label.clone(),
                d.radius_mm.resolve(params)?,
                d.half_width_mm.resolve(params)?,
            ));
        }
        dets.sort_by(|a, b| a.1.total_cmp(&b.1));
        for w in dets.windows(2) {
            if w[0].1 + w[0].2 > w[1].1 - w[1].2 {
                return Err(SpecError::DetectorOverlap {
                    a: w[0].0.clone(),
                    b: w[1].0.clone(),
                });
            }
        }

        // Build the region stack, splitting layers around detectors.
        let mut regions: Vec<Layer> = Vec::new();
        let mut detector_regions: Vec<(String, usize)> = Vec::new();
        let mut outer_region_of_layer: Vec<usize> = Vec::new();
        let mut det_iter = dets.iter().peekable();
        let mut r0 = 0.0f64;
        for (mat, t) in &layers {
            let r1 = r0 + t;
            let mut cursor = r0;
            while let Some((label, rc, hw)) = det_iter.peek() {
                if *rc > r1 {
                    break;
                }
                let (lo, hi) = (rc - hw, rc + hw);
                if !(*hw > 0.0 && lo > cursor && hi < r1) {
                    return Err(SpecError::DetectorDoesNotFit {
                        label: label.clone(),
                    });
                }
                regions.push(Layer::new(mat.clone(), lo - cursor));
                regions.push(Layer::new(mat.clone(), 2.0 * hw));
                detector_regions.push((label.clone(), regions.len() - 1));
                cursor = hi;
                det_iter.next();
            }
            if r1 - cursor > 0.0 {
                regions.push(Layer::new(mat.clone(), r1 - cursor));
            }
            outer_region_of_layer.push(regions.len() - 1);
            r0 = r1;
        }
        if det_iter.peek().is_some() {
            let (label, ..) = det_iter.peek().unwrap();
            return Err(SpecError::DetectorDoesNotFit {
                label: label.clone(),
            });
        }

        let mut config = RunConfig::new(
            Geometry::Sphere(regions),
            Source::IsotropicPoint,
            self.run.histories_per_batch,
            self.run.seed,
        );
        config.source_group = source_group;
        config.batches = self.run.batches;
        Ok(ResolvedShield {
            config,
            detector_regions,
            source_rate_n_per_s: rate,
            outer_region_of_layer,
        })
    }

    /// Every named parameter with its gradient role.
    pub fn parameter_roles(&self) -> BTreeMap<String, ParamRole> {
        let mut roles = BTreeMap::new();
        for (i, l) in self.layers.iter().enumerate() {
            if let Some(n) = l.thickness_mm.name() {
                roles.insert(n.to_string(), ParamRole::ThicknessAdjoint { layer: i });
            }
        }
        if let Some(n) = self.source.rate_n_per_s.name() {
            roles.insert(n.to_string(), ParamRole::SourceLinear);
        }
        if let Some(n) = self.source.energy_ev.name() {
            roles.insert(n.to_string(), ParamRole::Discrete);
        }
        for d in &self.detectors {
            for p in [&d.radius_mm, &d.half_width_mm] {
                if let Some(n) = p.name() {
                    roles.insert(n.to_string(), ParamRole::DetectorGeometry);
                }
            }
        }
        roles
    }
}

/// One detector's evaluated dose.
#[derive(Debug, Clone)]
pub struct DetectorDose {
    /// Detector label.
    pub label: String,
    /// Dose rate, µSv/h (mean ± RSE).
    pub dose_usv_per_h: Estimate,
}

/// Full Monte Carlo evaluation of a spec: the oracle pass.
pub fn evaluate(
    spec: &ShieldSpec,
    params: &BTreeMap<String, f64>,
) -> Result<(Vec<DetectorDose>, RunResult), SpecError> {
    let resolved = spec.resolve(params)?;
    let result =
        run(&resolved.config).map_err(|e: ConfigError| SpecError::Config(e.to_string()))?;
    let doses = resolved
        .detector_regions
        .iter()
        .map(|(label, region)| DetectorDose {
            label: label.clone(),
            dose_usv_per_h: result.dose_rate_usv_per_h(*region, resolved.source_rate_n_per_s),
        })
        .collect();
    Ok((doses, result))
}

/// The compass pass: d(dose at `detector_label`)/d(named thickness
/// parameter), in µSv/h per mm, via the M2 diffusion adjoint. Carries
/// the documented diffusion bias — steer with it, then re-price with
/// [`evaluate`].
pub fn d_dose_d_param_via_diffusion(
    spec: &ShieldSpec,
    params: &BTreeMap<String, f64>,
    param_name: &str,
    detector_label: &str,
) -> Result<f64, SpecError> {
    let roles = spec.parameter_roles();
    let layer = match roles.get(param_name) {
        Some(ParamRole::ThicknessAdjoint { layer }) => *layer,
        _ => return Err(SpecError::NotAThicknessParameter(param_name.to_string())),
    };
    let resolved = spec.resolve(params)?;
    let det_region = resolved
        .detector_regions
        .iter()
        .find(|(l, _)| l == detector_label)
        .ok_or_else(|| SpecError::DetectorDoesNotFit {
            label: detector_label.to_string(),
        })?
        .1;
    let region = resolved.outer_region_of_layer[layer];
    if region + 1 >= resolved.config.geometry.region_count() {
        return Err(SpecError::NoNextLayer(param_name.to_string()));
    }
    // Detector radius = center of its region.
    let bounds: Vec<f64> = {
        let mut b = vec![0.0];
        for l in resolved.config.geometry.layers() {
            b.push(b.last().unwrap() + l.thickness_mm);
        }
        b
    };
    let det_mm = 0.5 * (bounds[det_region] + bounds[det_region + 1]);
    let model = crate::diffusion::DiffusionModel::new(
        &resolved.config.geometry,
        &DiffusionOptions::default(),
    )
    .map_err(|e| SpecError::Config(e.to_string()))?;
    let det_cell = model
        .cell_at_mm(det_mm)
        .map_err(|e| SpecError::Config(e.to_string()))?;
    let fwd = model
        .forward(resolved.config.source_group)
        .map_err(|e| SpecError::Config(e.to_string()))?;
    let adj = model.adjoint_dose(det_cell);
    // companion gradient is per resolved-geometry layer index.
    let grad_psv_per_mm = model
        .d_dose_d_thickness_mm(&fwd, &adj, region)
        .map_err(|e| SpecError::Config(e.to_string()))?;
    Ok(grad_psv_per_mm * resolved.source_rate_n_per_s * 3600.0 * 1.0e-6)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_json() -> &'static str {
        r#"{
          "layers": [
            {"material": "air", "thickness_mm": 300},
            {"material": "hdpe", "thickness_mm": "shield_t"},
            {"material": "air", "thickness_mm": 1800}
          ],
          "source": {"rate_n_per_s": "src_rate", "energy_ev": 2.45e6},
          "detectors": [
            {"label": "operator", "radius_mm": 2000},
            {"label": "near", "radius_mm": 1000}
          ],
          "run": {"histories_per_batch": 1000, "batches": 8, "seed": 7}
        }"#
    }

    fn bind(shield_t: f64) -> BTreeMap<String, f64> {
        BTreeMap::from([
            ("shield_t".to_string(), shield_t),
            ("src_rate".to_string(), 5.0e6),
        ])
    }

    #[test]
    fn json_round_trip_and_resolve() {
        let spec: ShieldSpec = serde_json::from_str(spec_json()).unwrap();
        let back = serde_json::to_string(&spec).unwrap();
        let again: ShieldSpec = serde_json::from_str(&back).unwrap();
        assert_eq!(spec, again);
        let resolved = spec.resolve(&bind(150.0)).unwrap();
        // air | hdpe | air-split(3) around near | air-split around operator:
        // regions: air, hdpe, air, det(near), air, det(operator), air
        assert_eq!(resolved.config.geometry.region_count(), 7);
        assert_eq!(
            resolved.detector_regions,
            vec![("near".to_string(), 3), ("operator".to_string(), 5)]
        );
        assert_eq!(resolved.source_rate_n_per_s, 5.0e6);
    }

    #[test]
    fn fail_closed_everywhere() {
        let spec: ShieldSpec = serde_json::from_str(spec_json()).unwrap();
        // Unbound parameter.
        let mut p = bind(150.0);
        p.remove("shield_t");
        assert_eq!(
            spec.resolve(&p).unwrap_err(),
            SpecError::UnboundParameter("shield_t".to_string())
        );
        // Unknown material.
        let mut s2 = spec.clone();
        s2.layers[1].material = "unobtainium".to_string();
        assert_eq!(
            s2.resolve(&bind(150.0)).unwrap_err(),
            SpecError::UnknownMaterial("unobtainium".to_string())
        );
        // Energy outside the structure.
        let mut s3 = spec.clone();
        s3.source.energy_ev = ParamValue::Literal(14.1e6);
        assert!(matches!(
            s3.resolve(&bind(150.0)).unwrap_err(),
            SpecError::EnergyOutsideStructure { .. }
        ));
        // Bad rate.
        let mut p4 = bind(150.0);
        p4.insert("src_rate".to_string(), 0.0);
        assert_eq!(
            spec.resolve(&p4).unwrap_err(),
            SpecError::BadSourceRate(0.0)
        );
        // Detector outside geometry.
        let mut s5 = spec.clone();
        s5.detectors[0].radius_mm = ParamValue::Literal(99_000.0);
        assert!(matches!(
            s5.resolve(&bind(150.0)).unwrap_err(),
            SpecError::DetectorDoesNotFit { .. }
        ));
        // Overlapping detectors.
        let mut s6 = spec.clone();
        s6.detectors[1].radius_mm = ParamValue::Literal(2010.0);
        assert!(matches!(
            s6.resolve(&bind(150.0)).unwrap_err(),
            SpecError::DetectorOverlap { .. }
        ));
        // No detectors.
        let mut s7 = spec.clone();
        s7.detectors.clear();
        assert_eq!(
            s7.resolve(&bind(150.0)).unwrap_err(),
            SpecError::NoDetectors
        );
    }

    #[test]
    fn roles_classify_gradient_paths() {
        let spec: ShieldSpec = serde_json::from_str(spec_json()).unwrap();
        let roles = spec.parameter_roles();
        assert_eq!(
            roles.get("shield_t"),
            Some(&ParamRole::ThicknessAdjoint { layer: 1 })
        );
        assert_eq!(roles.get("src_rate"), Some(&ParamRole::SourceLinear));
    }

    #[test]
    fn evaluate_and_gradient_smoke() {
        let spec: ShieldSpec = serde_json::from_str(spec_json()).unwrap();
        let (doses, result) = evaluate(&spec, &bind(150.0)).unwrap();
        assert_eq!(doses.len(), 2);
        assert_eq!(result.truncated_histories, 0);
        for d in &doses {
            assert!(d.dose_usv_per_h.mean > 0.0 && d.dose_usv_per_h.rse.is_finite());
        }
        // Compass gradient: thickening the shield must reduce the
        // operator dose.
        let g = d_dose_d_param_via_diffusion(&spec, &bind(150.0), "shield_t", "operator").unwrap();
        assert!(g < 0.0, "d(dose)/d(shield_t) = {g} µSv/h/mm");
        // Non-thickness parameter fails closed.
        assert!(matches!(
            d_dose_d_param_via_diffusion(&spec, &bind(150.0), "src_rate", "operator"),
            Err(SpecError::NotAThicknessParameter(_))
        ));
    }
}
