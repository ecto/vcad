//! Planar film-stack simulator.
//!
//! Executes a [`Recipe`] against a set of 2D masks and produces the
//! surviving [`Film`]s. Everything is planar (v0): each film lives at a
//! fixed z range and the "current top surface" is a single scalar height,
//! not a height field — deposits over a patterned film do not conform to
//! the pattern. See the crate README for the full list of approximations.
//!
//! Units: µm in x/y/z. The substrate top is z = 0; the wafer body extends
//! down to `-substrate_thickness_um`.

use std::collections::HashMap;

use geo::{BooleanOps, MultiPolygon, Rect};

use crate::error::{ProcessError, Result};
use crate::recipe::{Polarity, ProcessStep, Recipe, ResistTone, SI_CONSUMED_PER_OXIDE};

/// Films thinner than this (µm) are dropped from the output.
const MIN_THICKNESS_UM: f64 = 1e-9;

/// How a film came to exist — drives labeling and rendering tweaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilmKind {
    /// The wafer itself.
    Substrate,
    /// Blanket-deposited ([`ProcessStep::Deposit`]).
    Deposited,
    /// Thermally grown ([`ProcessStep::GrowOxide`]).
    Grown,
    /// Doped region inset into the substrate ([`ProcessStep::Implant`]).
    Implant,
    /// Spin-coated photoresist ([`ProcessStep::SpinResist`]).
    Resist,
}

/// One recorded exposure shot on a resist film.
#[derive(Debug, Clone)]
pub struct Exposure {
    /// GDS layer that patterned the shot.
    pub mask_layer: i16,
    /// Dose in mJ/cm². Recorded for bookkeeping only — dose-to-clear
    /// thresholding is future work; exposure is binary for now.
    pub dose_mj_cm2: f64,
    /// Exposed plan-view pattern (mask polygons clipped to bounds), µm.
    pub pattern: MultiPolygon<f64>,
}

/// Litho bookkeeping carried by resist films ([`FilmKind::Resist`]).
#[derive(Debug, Clone)]
pub struct ResistState {
    /// Tone of the resist.
    pub tone: ResistTone,
    /// Exposures recorded so far, in order. [`ProcessStep::Develop`]
    /// resolves their union.
    pub exposures: Vec<Exposure>,
    /// Whether the film has been developed (its footprint reflects the
    /// exposure pattern).
    pub developed: bool,
}

/// One surviving slab of material after running a recipe.
#[derive(Debug, Clone)]
pub struct Film {
    /// Unique display label, e.g. `"s03_poly"`.
    pub name: String,
    /// Material name from the recipe (drives color).
    pub material: String,
    /// Provenance of the film.
    pub kind: FilmKind,
    /// Bottom of the film in µm (substrate top = 0).
    pub z_bottom_um: f64,
    /// Top of the film in µm.
    pub z_top_um: f64,
    /// Plan-view extent of the film, in µm.
    pub footprint: MultiPolygon<f64>,
    /// Litho state for resist films (`kind == FilmKind::Resist`);
    /// `None` for everything else.
    pub resist: Option<ResistState>,
}

impl Film {
    /// Film thickness in µm.
    pub fn thickness_um(&self) -> f64 {
        self.z_top_um - self.z_bottom_um
    }
}

/// Masks for a recipe: GDS layer number → unioned plan-view polygons (µm).
pub type Masks = HashMap<i16, MultiPolygon<f64>>;

fn bounds_polygon(bounds: Rect<f64>) -> MultiPolygon<f64> {
    MultiPolygon::new(vec![bounds.to_polygon()])
}

/// Etch `films[index]` down by `depth`: `keep` survives at full height,
/// `removed` is where the etch bites. A depth reaching the film bottom
/// clears the removed region entirely; a shallower etch leaves a
/// recessed remnant there.
fn etch_film(
    films: &mut Vec<Film>,
    index: usize,
    keep: MultiPolygon<f64>,
    removed: MultiPolygon<f64>,
    depth: f64,
) {
    let film = &mut films[index];
    let thickness = film.thickness_um();
    if depth + MIN_THICKNESS_UM >= thickness {
        // Etched through: the film only survives where kept.
        film.footprint = keep;
    } else {
        // Partial etch: full-height film where kept, recessed remnant
        // where etched.
        let (name, material, kind) = (film.name.clone(), film.material.clone(), film.kind);
        let (z_bottom, z_top) = (film.z_bottom_um, film.z_top_um - depth);
        film.footprint = keep;
        films.push(Film {
            name: format!("{name}_recess"),
            material,
            kind,
            z_bottom_um: z_bottom,
            z_top_um: z_top,
            footprint: removed,
            resist: None,
        });
    }
}

fn positive(value: f64, what: &str) -> Result<f64> {
    if value.is_finite() && value > 0.0 {
        Ok(value)
    } else {
        Err(ProcessError::BadRecipe(format!(
            "{what} must be positive and finite, got {value}"
        )))
    }
}

/// Run `recipe` over `masks` within `bounds` and return the surviving
/// films, bottom-up (substrate first).
///
/// `masks` must contain an entry for every GDS layer any step references
/// (an empty [`MultiPolygon`] is fine — it means "no mask openings within
/// bounds"); a missing entry is reported as
/// [`ProcessError::UnknownMaskLayer`].
pub fn simulate(recipe: &Recipe, masks: &Masks, bounds: Rect<f64>) -> Result<Vec<Film>> {
    positive(recipe.substrate_thickness_um, "substrate thickness")?;
    let blanket = bounds_polygon(bounds);

    let mut films: Vec<Film> = vec![Film {
        name: format!("s00_{}", recipe.substrate_material),
        material: recipe.substrate_material.clone(),
        kind: FilmKind::Substrate,
        z_bottom_um: -recipe.substrate_thickness_um,
        z_top_um: 0.0,
        footprint: blanket.clone(),
        resist: None,
    }];
    // Height of the (planar) top surface and index of the film forming it.
    let mut top_um = 0.0_f64;
    let mut surface = 0_usize;

    let mask = |layer: i16| -> Result<&MultiPolygon<f64>> {
        masks
            .get(&layer)
            .ok_or(ProcessError::UnknownMaskLayer(layer))
    };

    for (step_index, step) in recipe.steps.iter().enumerate() {
        let sn = step_index + 1; // s00 is the substrate
        match step {
            ProcessStep::Deposit {
                material,
                thickness_um,
            } => {
                let t = positive(*thickness_um, "deposit thickness")?;
                films.push(Film {
                    name: format!("s{sn:02}_{material}"),
                    material: material.clone(),
                    kind: FilmKind::Deposited,
                    z_bottom_um: top_um,
                    z_top_um: top_um + t,
                    footprint: blanket.clone(),
                    resist: None,
                });
                top_um += t;
                surface = films.len() - 1;
            }
            ProcessStep::GrowOxide { thickness_um } => {
                let t = positive(*thickness_um, "oxide thickness")?;
                let consumed = SI_CONSUMED_PER_OXIDE * t;
                let film = &mut films[surface];
                let old_top = film.z_top_um;
                // Consume the top of the surface film (clamped — growing
                // more oxide than there is silicon just eats the film).
                film.z_top_um = (old_top - consumed).max(film.z_bottom_um);
                let oxide_bottom = film.z_top_um;
                films.push(Film {
                    name: format!("s{sn:02}_sio2"),
                    material: "sio2".to_string(),
                    kind: FilmKind::Grown,
                    z_bottom_um: oxide_bottom,
                    z_top_um: oxide_bottom + t,
                    footprint: blanket.clone(),
                    resist: None,
                });
                top_um = oxide_bottom + t;
                surface = films.len() - 1;
            }
            ProcessStep::PatternEtch {
                mask_layer,
                polarity,
                depth_um,
            } => {
                let depth = positive(*depth_um, "etch depth")?;
                let mask = mask(*mask_layer)?.intersection(&blanket);
                let film = &films[surface];
                let (keep, removed) = match polarity {
                    Polarity::KeepMasked => (
                        film.footprint.intersection(&mask),
                        film.footprint.difference(&mask),
                    ),
                    Polarity::RemoveMasked => (
                        film.footprint.difference(&mask),
                        film.footprint.intersection(&mask),
                    ),
                };
                etch_film(&mut films, surface, keep, removed, depth);
                // Planar approximation: `top_um` stays at the un-etched
                // film top; the next deposit bridges over the etched gaps.
            }
            ProcessStep::Implant {
                mask_layer,
                dopant,
                depth_um,
            } => {
                let depth = positive(*depth_um, "implant depth")?;
                let mask = mask(*mask_layer)?.intersection(&blanket);
                let substrate_top = films[0].z_top_um;
                films.push(Film {
                    name: format!("s{sn:02}_{dopant}"),
                    material: dopant.clone(),
                    kind: FilmKind::Implant,
                    z_bottom_um: substrate_top - depth,
                    z_top_um: substrate_top,
                    footprint: mask,
                    resist: None,
                });
                // Implants do not change the surface.
            }
            ProcessStep::Planarize { to_um } => {
                if !to_um.is_finite() {
                    return Err(ProcessError::BadRecipe(format!(
                        "planarize height must be finite, got {to_um}"
                    )));
                }
                for film in &mut films {
                    film.z_top_um = film.z_top_um.min(*to_um);
                }
                top_um = top_um.min(*to_um);
                // `surface` may now be a zero-thickness film; the next
                // deposit lands on `top_um` either way, and etches of a
                // fully clipped film are no-ops on an empty slab.
            }
            ProcessStep::SpinResist { thickness_um, tone } => {
                let t = positive(*thickness_um, "resist thickness")?;
                films.push(Film {
                    name: format!("s{sn:02}_resist"),
                    material: "resist".to_string(),
                    kind: FilmKind::Resist,
                    z_bottom_um: top_um,
                    z_top_um: top_um + t,
                    footprint: blanket.clone(),
                    resist: Some(ResistState {
                        tone: *tone,
                        exposures: Vec::new(),
                        developed: false,
                    }),
                });
                top_um += t;
                surface = films.len() - 1;
            }
            ProcessStep::Expose {
                mask_layer,
                dose_mj_cm2,
            } => {
                let dose = positive(*dose_mj_cm2, "exposure dose")?;
                let pattern = mask(*mask_layer)?.intersection(&blanket);
                let film = films
                    .iter_mut()
                    .rev()
                    .find(|f| f.kind == FilmKind::Resist)
                    .ok_or_else(|| {
                        ProcessError::BadRecipe(
                            "Expose requires a resist film; add a SpinResist step first".into(),
                        )
                    })?;
                let state = film
                    .resist
                    .as_mut()
                    .expect("resist films carry litho state");
                state.exposures.push(Exposure {
                    mask_layer: *mask_layer,
                    dose_mj_cm2: dose,
                    pattern,
                });
            }
            ProcessStep::Develop => {
                let film = films
                    .iter_mut()
                    .rev()
                    .find(
                        |f| matches!(&f.resist, Some(r) if !r.developed && !r.exposures.is_empty()),
                    )
                    .ok_or_else(|| {
                        ProcessError::BadRecipe(
                            "Develop requires an exposed, undeveloped resist film".into(),
                        )
                    })?;
                let state = film.resist.as_mut().expect("matched a resist film");
                let tone = state.tone;
                let exposed = state
                    .exposures
                    .iter()
                    .skip(1)
                    .fold(state.exposures[0].pattern.clone(), |acc, e| {
                        acc.union(&e.pattern)
                    });
                state.developed = true;
                film.footprint = match tone {
                    ResistTone::Positive => film.footprint.difference(&exposed),
                    ResistTone::Negative => film.footprint.intersection(&exposed),
                };
                // Planar approximation: like PatternEtch, `top_um` stays
                // at the resist top; the openings are gaps, not a new
                // surface height.
            }
            ProcessStep::EtchThroughResist { depth_um } => {
                let depth = positive(*depth_um, "etch depth")?;
                let mut resists = films.iter().filter(|f| f.kind == FilmKind::Resist);
                let Some(first) = resists.next() else {
                    return Err(ProcessError::BadRecipe(
                        "EtchThroughResist requires a resist film; \
                         run SpinResist → Expose → Develop first"
                            .into(),
                    ));
                };
                // Everything under resist is protected; the union covers
                // the (unusual) multi-resist case.
                let protected =
                    resists.fold(first.footprint.clone(), |acc, f| acc.union(&f.footprint));
                // Target the film forming the surface under the resist:
                // the topmost film that is neither resist nor an implant
                // (implants live inside the substrate).
                let target = films
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| !matches!(f.kind, FilmKind::Resist | FilmKind::Implant))
                    .max_by(|a, b| a.1.z_top_um.total_cmp(&b.1.z_top_um))
                    .map(|(i, _)| i)
                    .expect("the substrate always survives");
                let film = &films[target];
                let keep = film.footprint.intersection(&protected);
                let removed = film.footprint.difference(&protected);
                etch_film(&mut films, target, keep, removed, depth);
                // Planar approximation as for PatternEtch: `top_um` is
                // unchanged (and still includes the resist on top).
            }
            ProcessStep::Strip => {
                films.retain(|f| f.kind != FilmKind::Resist);
                // The surface reverts to the tallest remaining film
                // (implants sit inside the substrate and never form the
                // surface; ties go to the most recent film).
                let (index, z_top) = films
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| f.kind != FilmKind::Implant)
                    .max_by(|a, b| a.1.z_top_um.total_cmp(&b.1.z_top_um))
                    .map(|(i, f)| (i, f.z_top_um))
                    .expect("the substrate always survives a strip");
                surface = index;
                top_um = z_top;
            }
        }
    }

    films.retain(|f| f.thickness_um() > MIN_THICKNESS_UM && !f.footprint.0.is_empty());
    films.sort_by(|a, b| a.z_bottom_um.total_cmp(&b.z_bottom_um));
    Ok(films)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::{Polarity, ProcessStep, Recipe};
    use geo::{coord, polygon, Area};

    fn bounds() -> Rect<f64> {
        Rect::new(coord! { x: 0.0, y: 0.0 }, coord! { x: 10.0, y: 10.0 })
    }

    fn square_mask(x0: f64, y0: f64, x1: f64, y1: f64) -> MultiPolygon<f64> {
        MultiPolygon::new(vec![polygon![
            (x: x0, y: y0),
            (x: x1, y: y0),
            (x: x1, y: y1),
            (x: x0, y: y1),
        ]])
    }

    fn recipe(steps: Vec<ProcessStep>) -> Recipe {
        Recipe {
            substrate_material: "silicon".into(),
            substrate_thickness_um: 2.0,
            steps,
        }
    }

    #[test]
    fn etch_removes_geometry() {
        // Blanket film, then remove where the mask is: surviving area is
        // bounds minus mask.
        let mut masks = Masks::new();
        masks.insert(1, square_mask(2.0, 2.0, 6.0, 6.0));
        let films = simulate(
            &recipe(vec![
                ProcessStep::Deposit {
                    material: "poly".into(),
                    thickness_um: 0.2,
                },
                ProcessStep::PatternEtch {
                    mask_layer: 1,
                    polarity: Polarity::RemoveMasked,
                    depth_um: 0.2,
                },
            ]),
            &masks,
            bounds(),
        )
        .unwrap();
        assert_eq!(films.len(), 2); // substrate + patterned poly
        let poly = &films[1];
        assert_eq!(poly.material, "poly");
        // 10×10 field minus 4×4 mask hole.
        assert!((poly.footprint.unsigned_area() - (100.0 - 16.0)).abs() < 1e-9);
    }

    #[test]
    fn keep_masked_inverts_the_etch() {
        let mut masks = Masks::new();
        masks.insert(1, square_mask(2.0, 2.0, 6.0, 6.0));
        let films = simulate(
            &recipe(vec![
                ProcessStep::Deposit {
                    material: "poly".into(),
                    thickness_um: 0.2,
                },
                ProcessStep::PatternEtch {
                    mask_layer: 1,
                    polarity: Polarity::KeepMasked,
                    depth_um: 0.2,
                },
            ]),
            &masks,
            bounds(),
        )
        .unwrap();
        assert!((films[1].footprint.unsigned_area() - 16.0).abs() < 1e-9);
    }

    #[test]
    fn partial_etch_leaves_a_recessed_film() {
        let mut masks = Masks::new();
        masks.insert(1, square_mask(0.0, 0.0, 5.0, 10.0));
        let films = simulate(
            &recipe(vec![
                ProcessStep::Deposit {
                    material: "sio2".into(),
                    thickness_um: 0.4,
                },
                ProcessStep::PatternEtch {
                    mask_layer: 1,
                    polarity: Polarity::RemoveMasked,
                    depth_um: 0.1,
                },
            ]),
            &masks,
            bounds(),
        )
        .unwrap();
        // substrate + kept full film + recessed remnant.
        assert_eq!(films.len(), 3);
        let full = films.iter().find(|f| f.name == "s01_sio2").unwrap();
        let recess = films.iter().find(|f| f.name.ends_with("_recess")).unwrap();
        assert!((full.z_top_um - 0.4).abs() < 1e-12);
        assert!((recess.z_top_um - 0.3).abs() < 1e-12);
        assert!((recess.footprint.unsigned_area() - 50.0).abs() < 1e-9);
    }

    #[test]
    fn grow_oxide_consumes_silicon() {
        let films = simulate(
            &recipe(vec![ProcessStep::GrowOxide { thickness_um: 1.0 }]),
            &Masks::new(),
            bounds(),
        )
        .unwrap();
        assert_eq!(films.len(), 2);
        let substrate = &films[0];
        let oxide = &films[1];
        // 0.46 µm of Si consumed → substrate top drops to -0.46; oxide
        // spans -0.46 .. +0.54 (total 1.0, 54% above the original surface).
        assert!((substrate.z_top_um - (-0.46)).abs() < 1e-12);
        assert!((oxide.z_bottom_um - (-0.46)).abs() < 1e-12);
        assert!((oxide.z_top_um - 0.54).abs() < 1e-12);
        assert_eq!(oxide.material, "sio2");
    }

    #[test]
    fn planarize_clips_films() {
        let films = simulate(
            &recipe(vec![
                ProcessStep::Deposit {
                    material: "sio2".into(),
                    thickness_um: 0.5,
                },
                ProcessStep::Deposit {
                    material: "aluminum".into(),
                    thickness_um: 0.4,
                },
                ProcessStep::Planarize { to_um: 0.7 },
            ]),
            &Masks::new(),
            bounds(),
        )
        .unwrap();
        let al = films.iter().find(|f| f.material == "aluminum").unwrap();
        assert!((al.z_top_um - 0.7).abs() < 1e-12);
        assert!((al.thickness_um() - 0.2).abs() < 1e-12);
        // Planarizing below a film's bottom removes it entirely.
        let films = simulate(
            &recipe(vec![
                ProcessStep::Deposit {
                    material: "sio2".into(),
                    thickness_um: 0.5,
                },
                ProcessStep::Deposit {
                    material: "aluminum".into(),
                    thickness_um: 0.4,
                },
                ProcessStep::Planarize { to_um: 0.5 },
            ]),
            &Masks::new(),
            bounds(),
        )
        .unwrap();
        assert!(films.iter().all(|f| f.material != "aluminum"));
    }

    #[test]
    fn implant_insets_into_substrate() {
        let mut masks = Masks::new();
        masks.insert(65, square_mask(1.0, 1.0, 3.0, 3.0));
        let films = simulate(
            &recipe(vec![ProcessStep::Implant {
                mask_layer: 65,
                dopant: "ndiff".into(),
                depth_um: 0.12,
            }]),
            &masks,
            bounds(),
        )
        .unwrap();
        let implant = films.iter().find(|f| f.kind == FilmKind::Implant).unwrap();
        assert!((implant.z_bottom_um - (-0.12)).abs() < 1e-12);
        assert!((implant.z_top_um - 0.0).abs() < 1e-12);
        assert!((implant.footprint.unsigned_area() - 4.0).abs() < 1e-9);
    }

    #[test]
    fn missing_mask_layer_errors() {
        let result = simulate(
            &recipe(vec![ProcessStep::Implant {
                mask_layer: 42,
                dopant: "ndiff".into(),
                depth_um: 0.1,
            }]),
            &Masks::new(),
            bounds(),
        );
        assert!(matches!(result, Err(ProcessError::UnknownMaskLayer(42))));
    }

    // --- photolithography ---

    use crate::recipe::ResistTone;

    /// Masks with the standard 4×4 test square on layer 1.
    fn litho_masks() -> Masks {
        let mut masks = Masks::new();
        masks.insert(1, square_mask(2.0, 2.0, 6.0, 6.0));
        masks
    }

    fn spin(tone: ResistTone) -> ProcessStep {
        ProcessStep::SpinResist {
            thickness_um: 1.0,
            tone,
        }
    }

    fn expose() -> ProcessStep {
        ProcessStep::Expose {
            mask_layer: 1,
            dose_mj_cm2: 150.0,
        }
    }

    #[test]
    fn spin_resist_deposits_a_blanket_resist_film() {
        let films = simulate(
            &recipe(vec![spin(ResistTone::Positive)]),
            &Masks::new(),
            bounds(),
        )
        .unwrap();
        assert_eq!(films.len(), 2);
        let resist = &films[1];
        assert_eq!(resist.kind, FilmKind::Resist);
        assert_eq!(resist.material, "resist");
        assert!((resist.z_bottom_um - 0.0).abs() < 1e-12);
        assert!((resist.z_top_um - 1.0).abs() < 1e-12);
        assert!((resist.footprint.unsigned_area() - 100.0).abs() < 1e-9);
        let state = resist.resist.as_ref().unwrap();
        assert_eq!(state.tone, ResistTone::Positive);
        assert!(state.exposures.is_empty() && !state.developed);
    }

    #[test]
    fn expose_records_pattern_and_dose_on_the_resist() {
        let films = simulate(
            &recipe(vec![spin(ResistTone::Positive), expose()]),
            &litho_masks(),
            bounds(),
        )
        .unwrap();
        let state = films[1].resist.as_ref().unwrap();
        assert_eq!(state.exposures.len(), 1);
        let shot = &state.exposures[0];
        assert_eq!(shot.mask_layer, 1);
        assert!((shot.dose_mj_cm2 - 150.0).abs() < 1e-12);
        assert!((shot.pattern.unsigned_area() - 16.0).abs() < 1e-9);
        // Exposure alone does not change the footprint.
        assert!((films[1].footprint.unsigned_area() - 100.0).abs() < 1e-9);
    }

    #[test]
    fn positive_develop_removes_exposed_regions() {
        let films = simulate(
            &recipe(vec![
                spin(ResistTone::Positive),
                expose(),
                ProcessStep::Develop,
            ]),
            &litho_masks(),
            bounds(),
        )
        .unwrap();
        let resist = &films[1];
        // 10×10 blanket minus the 4×4 exposed square.
        assert!((resist.footprint.unsigned_area() - 84.0).abs() < 1e-9);
        assert!(resist.resist.as_ref().unwrap().developed);
    }

    #[test]
    fn negative_develop_keeps_only_exposed_regions() {
        let films = simulate(
            &recipe(vec![
                spin(ResistTone::Negative),
                expose(),
                ProcessStep::Develop,
            ]),
            &litho_masks(),
            bounds(),
        )
        .unwrap();
        assert!((films[1].footprint.unsigned_area() - 16.0).abs() < 1e-9);
    }

    #[test]
    fn litho_steps_out_of_order_error() {
        // Expose with no resist on the wafer.
        let result = simulate(&recipe(vec![expose()]), &litho_masks(), bounds());
        assert!(matches!(result, Err(ProcessError::BadRecipe(_))));
        // Develop with no exposed resist.
        let result = simulate(
            &recipe(vec![spin(ResistTone::Positive), ProcessStep::Develop]),
            &litho_masks(),
            bounds(),
        );
        assert!(matches!(result, Err(ProcessError::BadRecipe(_))));
        // EtchThroughResist with no resist at all.
        let result = simulate(
            &recipe(vec![ProcessStep::EtchThroughResist { depth_um: 0.1 }]),
            &litho_masks(),
            bounds(),
        );
        assert!(matches!(result, Err(ProcessError::BadRecipe(_))));
    }

    #[test]
    fn expose_missing_mask_layer_errors() {
        let result = simulate(
            &recipe(vec![
                spin(ResistTone::Positive),
                ProcessStep::Expose {
                    mask_layer: 7,
                    dose_mj_cm2: 100.0,
                },
            ]),
            &litho_masks(),
            bounds(),
        );
        assert!(matches!(result, Err(ProcessError::UnknownMaskLayer(7))));
    }

    #[test]
    fn etch_through_resist_removes_only_where_resist_is_open() {
        // Positive resist over blanket poly: develop opens the mask
        // square, the etch bites only there.
        let films = simulate(
            &recipe(vec![
                ProcessStep::Deposit {
                    material: "poly".into(),
                    thickness_um: 0.2,
                },
                spin(ResistTone::Positive),
                expose(),
                ProcessStep::Develop,
                ProcessStep::EtchThroughResist { depth_um: 0.2 },
            ]),
            &litho_masks(),
            bounds(),
        )
        .unwrap();
        let poly = films.iter().find(|f| f.material == "poly").unwrap();
        assert!((poly.footprint.unsigned_area() - 84.0).abs() < 1e-9);
        // The resist is still on the wafer until stripped.
        assert!(films.iter().any(|f| f.kind == FilmKind::Resist));
    }

    #[test]
    fn etch_through_resist_respects_depth() {
        // A 0.1 µm etch into a 0.4 µm film leaves a recessed remnant in
        // the open regions instead of clearing them.
        let films = simulate(
            &recipe(vec![
                ProcessStep::Deposit {
                    material: "sio2".into(),
                    thickness_um: 0.4,
                },
                spin(ResistTone::Positive),
                expose(),
                ProcessStep::Develop,
                ProcessStep::EtchThroughResist { depth_um: 0.1 },
                ProcessStep::Strip,
            ]),
            &litho_masks(),
            bounds(),
        )
        .unwrap();
        let full = films.iter().find(|f| f.name == "s01_sio2").unwrap();
        let recess = films.iter().find(|f| f.name.ends_with("_recess")).unwrap();
        assert!((full.z_top_um - 0.4).abs() < 1e-12);
        assert!((full.footprint.unsigned_area() - 84.0).abs() < 1e-9);
        assert!((recess.z_top_um - 0.3).abs() < 1e-12);
        assert!((recess.footprint.unsigned_area() - 16.0).abs() < 1e-9);
    }

    #[test]
    fn strip_removes_resist_but_nothing_else() {
        let films = simulate(
            &recipe(vec![
                ProcessStep::Deposit {
                    material: "poly".into(),
                    thickness_um: 0.2,
                },
                spin(ResistTone::Positive),
                expose(),
                ProcessStep::Develop,
                ProcessStep::Strip,
                // The next deposit must land back on the poly top, not on
                // the departed resist's top.
                ProcessStep::Deposit {
                    material: "aluminum".into(),
                    thickness_um: 0.3,
                },
            ]),
            &litho_masks(),
            bounds(),
        )
        .unwrap();
        assert!(films.iter().all(|f| f.kind != FilmKind::Resist));
        assert!(films.iter().any(|f| f.material == "poly"));
        let al = films.iter().find(|f| f.material == "aluminum").unwrap();
        assert!((al.z_bottom_um - 0.2).abs() < 1e-12);
        assert!((al.z_top_um - 0.5).abs() < 1e-12);
    }

    #[test]
    fn litho_sequence_matches_idealized_pattern_etch() {
        // The whole point of the resist steps: spin → expose → develop →
        // etch-through-resist → strip must land exactly where the
        // one-step idealized PatternEtch lands, for both tones.
        let masks = litho_masks();
        for (tone, polarity) in [
            (ResistTone::Negative, Polarity::KeepMasked),
            (ResistTone::Positive, Polarity::RemoveMasked),
        ] {
            let litho = simulate(
                &recipe(vec![
                    ProcessStep::Deposit {
                        material: "poly".into(),
                        thickness_um: 0.2,
                    },
                    spin(tone),
                    expose(),
                    ProcessStep::Develop,
                    ProcessStep::EtchThroughResist { depth_um: 0.2 },
                    ProcessStep::Strip,
                ]),
                &masks,
                bounds(),
            )
            .unwrap();
            let ideal = simulate(
                &recipe(vec![
                    ProcessStep::Deposit {
                        material: "poly".into(),
                        thickness_um: 0.2,
                    },
                    ProcessStep::PatternEtch {
                        mask_layer: 1,
                        polarity,
                        depth_um: 0.2,
                    },
                ]),
                &masks,
                bounds(),
            )
            .unwrap();
            assert_eq!(litho.len(), ideal.len(), "film count differs ({tone:?})");
            for (a, b) in litho.iter().zip(&ideal) {
                assert_eq!(a.material, b.material);
                assert_eq!(a.kind, b.kind);
                assert!((a.z_bottom_um - b.z_bottom_um).abs() < 1e-12);
                assert!((a.z_top_um - b.z_top_um).abs() < 1e-12);
                let sym = a.footprint.difference(&b.footprint).unsigned_area()
                    + b.footprint.difference(&a.footprint).unsigned_area();
                assert!(sym < 1e-9, "{} footprint differs ({tone:?}): {sym}", a.name);
            }
        }
    }
}
