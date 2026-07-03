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
use crate::recipe::{Polarity, ProcessStep, Recipe, SI_CONSUMED_PER_OXIDE};

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
                let film = &mut films[surface];
                let keep = match polarity {
                    Polarity::KeepMasked => film.footprint.intersection(&mask),
                    Polarity::RemoveMasked => film.footprint.difference(&mask),
                };
                let thickness = film.thickness_um();
                if depth + MIN_THICKNESS_UM >= thickness {
                    // Etched through: the film only survives where kept.
                    film.footprint = keep;
                } else {
                    // Partial etch: full-height film where kept, recessed
                    // remnant where etched.
                    let removed = match polarity {
                        Polarity::KeepMasked => film.footprint.difference(&mask),
                        Polarity::RemoveMasked => film.footprint.intersection(&mask),
                    };
                    let (name, material, kind) =
                        (film.name.clone(), film.material.clone(), film.kind);
                    let (z_bottom, z_top) = (film.z_bottom_um, film.z_top_um - depth);
                    film.footprint = keep;
                    films.push(Film {
                        name: format!("{name}_recess"),
                        material,
                        kind,
                        z_bottom_um: z_bottom,
                        z_top_um: z_top,
                        footprint: removed,
                    });
                }
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
}
