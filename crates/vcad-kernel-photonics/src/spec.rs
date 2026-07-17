//! The `.vcad` seam: a serde topology-design schema with named parameters.
//!
//! A [`TopologyProblemSpec`] is the serialization contract between vcad
//! documents and this crate's inverse-design machinery. Every scalar
//! field is a [`ParamValue`]: a literal, or the **name** of a document
//! parameter supplied at resolve time. Resolution is fail-closed — an
//! unbound name is an error, never a default (the `vcad-kernel-particle`
//! seam discipline, verbatim).
//!
//! The density vector is *data*, not a parameter: thousands of ρ values
//! travel as a plain array (empty ⇒ uniform `rho_init`), while the
//! physical knobs a document would sweep — wavelength, indices, guide
//! width, feature size — are nameable scalars.
//!
//! Geometry synthesis (which waveguides feed the design box, where
//! monitors sit) stays on the problem-builder side (M5's splitter);
//! this module owns the parameterization seam only.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::adjoint::DesignRegion;
use crate::design::TopologyParam;

/// Schema tag for this spec family.
pub const SPEC_SCHEMA: &str = "vcad.photonics-spec/1";

/// A literal value or the name of a document parameter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    /// A literal number.
    Literal(f64),
    /// The name of a parameter bound at [`TopologyProblemSpec::resolve`]
    /// time.
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
}

/// Serializable topology-design problem parameterization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TopologyProblemSpec {
    /// Schema tag ([`SPEC_SCHEMA`]).
    pub schema: String,
    /// Design wavelength (length units; f = 1/λ).
    pub wavelength: ParamValue,
    /// Core refractive index (ε_max = n²).
    pub n_core: ParamValue,
    /// Cladding refractive index (ε_min = n²).
    pub n_clad: ParamValue,
    /// Grid resolution in cells per vacuum wavelength.
    pub resolution: usize,
    /// Design-region extent in cells: `[i0, i1, j0, j1]` (Ez nodes,
    /// inclusive).
    pub region: [usize; 4],
    /// Cone-filter radius in cells (minimum feature scale).
    pub filter_radius_cells: ParamValue,
    /// Projection threshold η.
    pub eta: ParamValue,
    /// Initial uniform density used when `rho` is empty.
    pub rho_init: ParamValue,
    /// Raw densities, region-local row-major; empty ⇒ uniform
    /// `rho_init`. Length must otherwise equal the region cell count.
    #[serde(default)]
    pub rho: Vec<f64>,
}

impl TopologyProblemSpec {
    /// A fresh spec with everything literal and an empty density vector.
    pub fn new(
        wavelength: f64,
        n_core: f64,
        n_clad: f64,
        resolution: usize,
        region: DesignRegion,
    ) -> Self {
        Self {
            schema: SPEC_SCHEMA.to_string(),
            wavelength: wavelength.into(),
            n_core: n_core.into(),
            n_clad: n_clad.into(),
            resolution,
            region: [region.i0, region.i1, region.j0, region.j1],
            filter_radius_cells: 2.0.into(),
            eta: 0.5.into(),
            rho_init: 0.5.into(),
            rho: Vec::new(),
        }
    }

    /// Resolve every named parameter (fail-closed) into a concrete
    /// [`ResolvedTopology`]. `beta` is the caller's current binarization
    /// sharpness — a schedule state, not a document parameter.
    pub fn resolve(
        &self,
        params: &BTreeMap<String, f64>,
        beta: f64,
    ) -> Result<ResolvedTopology, SpecError> {
        if self.schema != SPEC_SCHEMA {
            return Err(SpecError::WrongSchema(self.schema.clone()));
        }
        let wavelength = self.wavelength.resolve(params)?;
        let n_core = self.n_core.resolve(params)?;
        let n_clad = self.n_clad.resolve(params)?;
        let filter_radius = self.filter_radius_cells.resolve(params)?;
        let eta = self.eta.resolve(params)?;
        let rho_init = self.rho_init.resolve(params)?;
        // NaN-fail-closed: NaN must land in the error arm.
        let bad = |v: f64| v.is_nan();
        if bad(wavelength)
            || bad(n_core)
            || bad(n_clad)
            || wavelength <= 0.0
            || n_core <= n_clad
            || n_clad < 1.0
        {
            return Err(SpecError::UnphysicalParameters);
        }
        if !(0.0..=1.0).contains(&rho_init) {
            return Err(SpecError::UnphysicalParameters);
        }
        let [i0, i1, j0, j1] = self.region;
        if i1 < i0 || j1 < j0 {
            return Err(SpecError::BadRegion);
        }
        let region = DesignRegion { i0, i1, j0, j1 };
        let mut param = TopologyParam::uniform(region, rho_init, n_clad * n_clad, n_core * n_core);
        param.filter_radius_cells = filter_radius;
        param.eta = eta;
        param.beta = beta;
        if !self.rho.is_empty() {
            if self.rho.len() != region.len() {
                return Err(SpecError::DensityLengthMismatch {
                    expected: region.len(),
                    got: self.rho.len(),
                });
            }
            if self.rho.iter().any(|v| !(0.0..=1.0).contains(v)) {
                return Err(SpecError::DensityOutOfRange);
            }
            param.rho = self.rho.clone();
        }
        Ok(ResolvedTopology {
            wavelength,
            n_core,
            n_clad,
            resolution: self.resolution,
            param,
        })
    }

    /// Store a density vector back into the spec (after optimization).
    pub fn set_rho(&mut self, rho: Vec<f64>) {
        self.rho = rho;
    }
}

/// A fully resolved topology parameterization plus its physical context.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedTopology {
    /// Design wavelength.
    pub wavelength: f64,
    /// Core index.
    pub n_core: f64,
    /// Cladding index.
    pub n_clad: f64,
    /// Cells per vacuum wavelength.
    pub resolution: usize,
    /// The realized design parameterization.
    pub param: TopologyParam,
}

/// Resolution failures — every variant is a refusal, not a default.
#[derive(Debug, Clone, PartialEq)]
pub enum SpecError {
    /// A named parameter had no binding.
    UnknownParameter(String),
    /// The schema tag was not [`SPEC_SCHEMA`].
    WrongSchema(String),
    /// Parameters violate physics (λ ≤ 0, n_core ≤ n_clad, n_clad < 1…).
    UnphysicalParameters,
    /// Region indices are not ordered.
    BadRegion,
    /// The density vector does not match the region.
    DensityLengthMismatch {
        /// Region cell count.
        expected: usize,
        /// Supplied density count.
        got: usize,
    },
    /// A density fell outside [0, 1].
    DensityOutOfRange,
}

impl std::fmt::Display for SpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpecError::UnknownParameter(name) => write!(f, "unbound parameter: {name:?}"),
            SpecError::WrongSchema(s) => write!(f, "wrong schema: {s:?} (want {SPEC_SCHEMA})"),
            SpecError::UnphysicalParameters => write!(f, "unphysical parameters"),
            SpecError::BadRegion => write!(f, "design region indices not ordered"),
            SpecError::DensityLengthMismatch { expected, got } => {
                write!(f, "density length {got} does not match region ({expected})")
            }
            SpecError::DensityOutOfRange => write!(f, "density outside [0, 1]"),
        }
    }
}

impl std::error::Error for SpecError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn region() -> DesignRegion {
        DesignRegion {
            i0: 10,
            i1: 19,
            j0: 5,
            j1: 12,
        }
    }

    #[test]
    fn named_parameters_resolve_fail_closed() {
        let mut spec = TopologyProblemSpec::new(1.55, 3.48, 1.44, 30, region());
        spec.wavelength = ParamValue::Named("lambda".into());
        let empty = BTreeMap::new();
        assert_eq!(
            spec.resolve(&empty, 8.0),
            Err(SpecError::UnknownParameter("lambda".into()))
        );
        let mut params = BTreeMap::new();
        params.insert("lambda".to_string(), 1.31);
        let r = spec.resolve(&params, 8.0).unwrap();
        assert_eq!(r.wavelength, 1.31);
        assert_eq!(r.param.beta, 8.0);
        assert_eq!(r.param.rho.len(), region().len());
    }

    #[test]
    fn density_validation() {
        let mut spec = TopologyProblemSpec::new(1.55, 3.48, 1.44, 30, region());
        spec.rho = vec![0.5; 3];
        let e = spec.resolve(&BTreeMap::new(), 1.0).unwrap_err();
        assert!(matches!(e, SpecError::DensityLengthMismatch { .. }));
        spec.rho = vec![1.5; region().len()];
        assert_eq!(
            spec.resolve(&BTreeMap::new(), 1.0).unwrap_err(),
            SpecError::DensityOutOfRange
        );
    }

    #[test]
    fn unphysical_rejected() {
        let spec = TopologyProblemSpec::new(1.55, 1.2, 1.44, 30, region());
        assert_eq!(
            spec.resolve(&BTreeMap::new(), 1.0).unwrap_err(),
            SpecError::UnphysicalParameters
        );
    }
}
