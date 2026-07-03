//! Process recipes as plain, serializable data.
//!
//! A [`Recipe`] is a substrate plus an ordered list of [`ProcessStep`]s —
//! the whole thing derives serde so recipes can live in JSON files next to
//! the layouts they build.

use serde::{Deserialize, Serialize};

/// Fraction of silicon consumed per unit of thermally grown oxide.
///
/// Thermal oxidation converts silicon into SiO₂; the classic rule of thumb
/// is that 0.46 units of silicon are consumed for every 1.0 unit of oxide
/// grown (the Si lattice expands by ~2.2× when oxidized). [`super::sim`]
/// lowers the surface film by `0.46 × thickness` and grows the oxide from
/// that recessed level, so 54% of the oxide sits above the original
/// surface and 46% below.
pub const SI_CONSUMED_PER_OXIDE: f64 = 0.46;

/// Which side of a photomask survives a patterned etch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Polarity {
    /// Film survives where mask polygons are present (etch the field).
    /// This is the usual subtractive patterning of a deposited film:
    /// deposit blanket poly, keep it only under the poly mask.
    KeepMasked,
    /// Film is removed where mask polygons are present (etch the shapes).
    /// E.g. opening the field oxide over active area.
    RemoveMasked,
}

/// One step of a planar process flow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "step")]
pub enum ProcessStep {
    /// Blanket-deposit a film of `material` over the current top surface.
    /// Planar v0: the film is a flat slab starting at the current top.
    Deposit {
        /// Material name (drives the display color, e.g. `"sio2"`).
        material: String,
        /// Film thickness in µm.
        thickness_um: f64,
    },
    /// Pattern-etch the current top film using a GDS layer as the mask.
    PatternEtch {
        /// GDS layer number whose polygons form the mask.
        mask_layer: i16,
        /// Whether masked or unmasked regions survive.
        polarity: Polarity,
        /// Etch depth in µm. Depth ≥ film thickness clears the film in
        /// the etched regions; a shallower depth leaves a recessed film.
        depth_um: f64,
    },
    /// Thermally grow oxide on the current top surface. Unlike
    /// [`ProcessStep::Deposit`], growth consumes
    /// [`SI_CONSUMED_PER_OXIDE`] × `thickness_um` of the film below.
    GrowOxide {
        /// Total oxide thickness in µm.
        thickness_um: f64,
    },
    /// Implant dopant into the top of the substrate under mask openings.
    /// Planar v0: a thin colored slab inset into the substrate wherever
    /// the mask layer has polygons.
    Implant {
        /// GDS layer number whose polygons define the implanted regions.
        mask_layer: i16,
        /// Dopant name (drives the display color, e.g. `"ndiff"`).
        dopant: String,
        /// Junction depth in µm below the current substrate top.
        depth_um: f64,
    },
    /// Chemical-mechanical planarization: clip everything above a height.
    Planarize {
        /// Height (µm, relative to the original substrate top at z = 0)
        /// above which all material is removed.
        to_um: f64,
    },
    /// Spin-coat a blanket photoresist film over the current top surface.
    SpinResist {
        /// Resist film thickness in µm.
        thickness_um: f64,
        /// Resist tone: which regions survive [`ProcessStep::Develop`].
        tone: ResistTone,
    },
    /// Expose the topmost resist film through a GDS layer. (A maskless
    /// stepper writing the same polygons directly is equivalent — the
    /// layer is simply where the light lands.) The pattern and dose are
    /// recorded on the resist film; exposure is binary for now and the
    /// dose is pure bookkeeping — dose-to-clear thresholding is future
    /// work. Multiple exposures accumulate (their patterns union).
    Expose {
        /// GDS layer number whose polygons are the exposed regions.
        mask_layer: i16,
        /// Exposure dose in mJ/cm² (recorded, not yet modeled).
        dose_mj_cm2: f64,
    },
    /// Develop the topmost exposed, undeveloped resist film:
    /// [`ResistTone::Positive`] removes the exposed regions,
    /// [`ResistTone::Negative`] keeps only the exposed regions. After
    /// develop the resist footprint reflects the pattern.
    Develop,
    /// Etch the topmost non-resist film wherever resist is absent, to
    /// `depth_um` — the physically honest replacement for
    /// [`ProcessStep::PatternEtch`]'s idealized mask → result shortcut.
    /// Same through/partial-etch semantics as `PatternEtch`.
    EtchThroughResist {
        /// Etch depth in µm. Depth ≥ film thickness clears the film in
        /// the open regions; a shallower depth leaves a recessed remnant.
        depth_um: f64,
    },
    /// Strip all resist films from the stack.
    Strip,
}

/// Photoresist tone: which regions survive [`ProcessStep::Develop`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResistTone {
    /// Exposed regions dissolve in the developer and are removed — the
    /// common case.
    Positive,
    /// Exposed regions cross-link and survive; everything unexposed is
    /// removed.
    Negative,
}

/// A full process flow: substrate plus ordered steps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    /// Substrate material name (usually `"silicon"`).
    pub substrate_material: String,
    /// Substrate thickness in µm. The substrate top surface is z = 0;
    /// the wafer body spans `-substrate_thickness_um .. 0`.
    pub substrate_thickness_um: f64,
    /// Process steps, executed in order.
    pub steps: Vec<ProcessStep>,
}

/// Axis a [`CutLine`] runs parallel to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    /// Cut line parallel to the X axis (a "horizontal" cut at fixed Y).
    X,
    /// Cut line parallel to the Y axis (a "vertical" cut at fixed X).
    Y,
}

/// A straight cut through the die for cross-section extraction.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CutLine {
    /// Axis the cut line runs parallel to.
    pub axis: Axis,
    /// Position of the line on the *other* axis, in µm (an [`Axis::X`]
    /// cut sits at `y = position_um`).
    pub position_um: f64,
    /// Extent of the cut along its own axis, `[start, end]` in µm.
    pub span: [f64; 2],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipe_serde_round_trip() {
        let recipe = Recipe {
            substrate_material: "silicon".into(),
            substrate_thickness_um: 1.5,
            steps: vec![
                ProcessStep::Implant {
                    mask_layer: 65,
                    dopant: "ndiff".into(),
                    depth_um: 0.12,
                },
                ProcessStep::GrowOxide { thickness_um: 0.3 },
                ProcessStep::Deposit {
                    material: "poly".into(),
                    thickness_um: 0.18,
                },
                ProcessStep::PatternEtch {
                    mask_layer: 66,
                    polarity: Polarity::KeepMasked,
                    depth_um: 0.18,
                },
                ProcessStep::Planarize { to_um: 0.8 },
            ],
        };
        let json = serde_json::to_string_pretty(&recipe).unwrap();
        let parsed: Recipe = serde_json::from_str(&json).unwrap();
        assert_eq!(recipe, parsed);
        // Steps are tagged so recipe files read like a run sheet.
        assert!(json.contains("\"step\": \"GrowOxide\""));
    }

    #[test]
    fn litho_steps_serde_round_trip() {
        let recipe = Recipe {
            substrate_material: "silicon".into(),
            substrate_thickness_um: 1.5,
            steps: vec![
                ProcessStep::SpinResist {
                    thickness_um: 1.0,
                    tone: ResistTone::Positive,
                },
                ProcessStep::Expose {
                    mask_layer: 66,
                    dose_mj_cm2: 150.0,
                },
                ProcessStep::Develop,
                ProcessStep::EtchThroughResist { depth_um: 0.18 },
                ProcessStep::Strip,
                ProcessStep::SpinResist {
                    thickness_um: 0.8,
                    tone: ResistTone::Negative,
                },
            ],
        };
        let json = serde_json::to_string_pretty(&recipe).unwrap();
        let parsed: Recipe = serde_json::from_str(&json).unwrap();
        assert_eq!(recipe, parsed);
        // Same run-sheet tagging as the rest of the steps.
        assert!(json.contains("\"step\": \"SpinResist\""));
        assert!(json.contains("\"step\": \"Develop\""));
        assert!(json.contains("\"tone\": \"Positive\""));
        assert!(json.contains("\"tone\": \"Negative\""));
    }

    #[test]
    fn cutline_serde_round_trip() {
        let cut = CutLine {
            axis: Axis::X,
            position_um: 63.5,
            span: [40.0, 90.0],
        };
        let json = serde_json::to_string(&cut).unwrap();
        let parsed: CutLine = serde_json::from_str(&json).unwrap();
        assert_eq!(cut, parsed);
    }
}
