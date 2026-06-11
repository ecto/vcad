//! Built-in shop catalogs — per-material/thickness bending specs for real
//! fab services, structured as data so values update without code changes.
//!
//! The first (and reference) catalog is **SendCutSend**
//! (`data/sendcutsend.json`, embedded at compile time): fixed inside bend
//! radius and K-factor per material/thickness, max bending dimensions, min
//! flange heights, bend-relief depth, and die width. Source URLs and the
//! retrieval date are recorded in the data file itself.
//!
//! Consumers:
//!
//! - `sheet_metal_create` / the WASM chain evaluator resolve `(radius, K)`
//!   through [`ShopCatalog::resolve_bend`] when a part targets a shop, so
//!   flat patterns match the shop's own calculator. Custom radii are
//!   **rejected** (these services don't bend custom radii) with an error
//!   naming the nearest valid radius.
//! - `sheet_metal_check` builds an effective [`ShopProfile`] from the
//!   catalog row via [`ShopCatalog::shop_profile_for`].
//! - The bend-table tool exposes the catalog rows via
//!   [`ShopCatalog::bend_table`].

use crate::bend_table::{BendTable, BendTableRow};
use crate::manufacturability::ShopProfile;
use serde::Deserialize;
use std::sync::OnceLock;

/// Tolerance when matching a requested thickness to a catalog row (mm).
const THICKNESS_TOL_MM: f64 = 0.13;

/// Tolerance when validating a requested radius against the fixed catalog
/// radius (mm).
const RADIUS_TOL_MM: f64 = 0.02;

/// One thickness row of a shop's bending catalog. All lengths in mm.
#[derive(Debug, Clone, PartialEq, Deserialize, serde::Serialize)]
pub struct ShopRow {
    /// Stock thickness.
    pub thickness_mm: f64,
    /// The K-factor the shop publishes for this stock.
    pub k_factor: f64,
    /// Fixed (effective @90°) inside bend radius.
    pub inside_radius_mm: f64,
    /// Brake die width — deformation half-width is half of this.
    pub die_width_mm: f64,
    /// Minimum formed flange height at 90°.
    pub min_flange_formed_mm: f64,
    /// Minimum flange in the flat pattern (before bending).
    pub min_flange_flat_mm: f64,
    /// Published bend-relief depth, measured from the bend line.
    pub relief_depth_mm: f64,
    /// Minimum corner-relief distance from the bend line.
    pub corner_relief_clearance_mm: f64,
    /// Maximum bend length for this stock.
    pub max_bend_length_mm: f64,
}

/// One material family of a shop catalog.
#[derive(Debug, Clone, PartialEq, Deserialize, serde::Serialize)]
pub struct ShopMaterial {
    /// Canonical key (matches the vcad materials registry where possible).
    pub key: String,
    /// Human-readable name as the shop lists it.
    pub display: String,
    /// Accepted aliases (case-insensitive).
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Per-thickness rows.
    pub rows: Vec<ShopRow>,
}

/// A complete shop bending catalog (deserialized from the embedded JSON).
#[derive(Debug, Clone, PartialEq, Deserialize, serde::Serialize)]
pub struct ShopCatalog {
    /// Stable id (`"sendcutsend"`).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Source URLs the numbers were transcribed from.
    pub sources: Vec<String>,
    /// Retrieval date (`YYYY-MM-DD`).
    pub retrieved: String,
    /// Free-form caveats.
    #[serde(default)]
    pub notes: Vec<String>,
    /// Minimum flat part size for bending `[short, long]` (mm).
    pub min_part_bend_mm: [f64; 2],
    /// Maximum flat part size for bending `[short, long]` (mm).
    pub max_part_bend_mm: [f64; 2],
    /// Maximum bend angle the shop forms (degrees from flat).
    pub max_bend_angle_deg: f64,
    /// Published minimum relief width as a multiple of thickness.
    pub relief_width_min_factor: f64,
    /// Material families.
    pub materials: Vec<ShopMaterial>,
}

/// Lookup failures, with enough structure for an agent to self-correct.
#[derive(Debug, Clone, PartialEq)]
pub enum ShopLookupError {
    /// The shop id isn't a built-in catalog.
    UnknownShop {
        /// Requested id.
        id: String,
        /// Available catalog ids.
        available: Vec<String>,
    },
    /// The shop doesn't bend this material at all.
    UnknownMaterial {
        /// Requested material.
        material: String,
        /// Materials the shop does bend (canonical keys).
        available: Vec<String>,
    },
    /// The shop doesn't stock this material at this thickness.
    UnknownThickness {
        /// Requested material (canonical key).
        material: String,
        /// Requested thickness (mm).
        thickness_mm: f64,
        /// Thicknesses the shop stocks for this material (mm).
        available_mm: Vec<f64>,
    },
    /// A custom radius was requested but the shop bends a fixed radius.
    FixedRadius {
        /// Requested material (canonical key).
        material: String,
        /// Stock thickness (mm).
        thickness_mm: f64,
        /// The radius that was requested (mm).
        requested_mm: f64,
        /// The only radius the shop bends for this stock (mm).
        fixed_mm: f64,
    },
}

impl std::fmt::Display for ShopLookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShopLookupError::UnknownShop { id, available } => write!(
                f,
                "unknown shop profile {id:?}; built-in catalogs: {}",
                available.join(", ")
            ),
            ShopLookupError::UnknownMaterial {
                material,
                available,
            } => write!(
                f,
                "this shop does not bend {material:?}; bendable materials: {}",
                available.join(", ")
            ),
            ShopLookupError::UnknownThickness {
                material,
                thickness_mm,
                available_mm,
            } => write!(
                f,
                "this shop does not stock {material} at {thickness_mm} mm; available: {} mm",
                available_mm
                    .iter()
                    .map(|t| format!("{t:.2}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            ShopLookupError::FixedRadius {
                material,
                thickness_mm,
                requested_mm,
                fixed_mm,
            } => write!(
                f,
                "custom bend radii are not available at this shop: {material} at \
                 {thickness_mm:.2} mm bends at a fixed {fixed_mm:.2} mm inside radius \
                 (requested {requested_mm:.2} mm); use {fixed_mm:.2} mm or omit the radius"
            ),
        }
    }
}

impl std::error::Error for ShopLookupError {}

fn sendcutsend() -> &'static ShopCatalog {
    static CATALOG: OnceLock<ShopCatalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("../data/sendcutsend.json"))
            .expect("embedded sendcutsend.json must parse")
    })
}

/// Ids of all built-in shop catalogs.
pub fn builtin_shop_ids() -> Vec<String> {
    vec!["sendcutsend".to_string()]
}

/// Look up a built-in shop catalog by id (case-insensitive).
pub fn shop_catalog(id: &str) -> Result<&'static ShopCatalog, ShopLookupError> {
    match id.trim().to_ascii_lowercase().as_str() {
        "sendcutsend" | "scs" => Ok(sendcutsend()),
        _ => Err(ShopLookupError::UnknownShop {
            id: id.to_string(),
            available: builtin_shop_ids(),
        }),
    }
}

impl ShopCatalog {
    /// Find a material family by canonical key or alias (case-insensitive).
    pub fn material(&self, name: &str) -> Option<&ShopMaterial> {
        let key = name.trim().to_ascii_lowercase();
        self.materials.iter().find(|m| {
            m.key.to_ascii_lowercase() == key
                || m.aliases.iter().any(|a| a.to_ascii_lowercase() == key)
        })
    }

    /// Find the catalog row for `(material, thickness)`.
    pub fn row(&self, material: &str, thickness_mm: f64) -> Result<&ShopRow, ShopLookupError> {
        let mat = self
            .material(material)
            .ok_or_else(|| ShopLookupError::UnknownMaterial {
                material: material.to_string(),
                available: self.materials.iter().map(|m| m.key.clone()).collect(),
            })?;
        let best = mat
            .rows
            .iter()
            .min_by(|a, b| {
                (a.thickness_mm - thickness_mm)
                    .abs()
                    .total_cmp(&(b.thickness_mm - thickness_mm).abs())
            })
            .filter(|r| (r.thickness_mm - thickness_mm).abs() <= THICKNESS_TOL_MM);
        best.ok_or_else(|| ShopLookupError::UnknownThickness {
            material: mat.key.clone(),
            thickness_mm,
            available_mm: mat.rows.iter().map(|r| r.thickness_mm).collect(),
        })
    }

    /// Resolve the `(radius, K, provenance-label)` for a bend.
    ///
    /// `requested_radius = None` uses the shop's fixed radius. A requested
    /// radius is accepted only when it matches the fixed radius within
    /// 0.02 mm — these services do not bend custom radii.
    pub fn resolve_bend(
        &self,
        material: &str,
        thickness_mm: f64,
        requested_radius: Option<f64>,
    ) -> Result<(f64, f64, String), ShopLookupError> {
        let row = self.row(material, thickness_mm)?;
        if let Some(r) = requested_radius {
            if (r - row.inside_radius_mm).abs() > RADIUS_TOL_MM {
                return Err(ShopLookupError::FixedRadius {
                    material: self
                        .material(material)
                        .map(|m| m.key.clone())
                        .unwrap_or_else(|| material.to_string()),
                    thickness_mm: row.thickness_mm,
                    requested_mm: r,
                    fixed_mm: row.inside_radius_mm,
                });
            }
        }
        let label = format!(
            "shop:{}/{}-t{:.2}R{:.2}",
            self.id,
            self.material(material)
                .map(|m| m.key.as_str())
                .unwrap_or(material),
            row.thickness_mm,
            row.inside_radius_mm
        );
        Ok((row.inside_radius_mm, row.k_factor, label))
    }

    /// All catalog rows as a [`BendTable`] (id `"shop:<id>"`), so the
    /// bend-table tool can expose shop data alongside the builtin table.
    pub fn bend_table(&self) -> BendTable {
        let rows = self
            .materials
            .iter()
            .flat_map(|m| {
                m.rows.iter().map(|r| BendTableRow {
                    material: m.key.clone(),
                    thickness: r.thickness_mm,
                    radius: r.inside_radius_mm,
                    k_factor: r.k_factor,
                })
            })
            .collect();
        BendTable {
            id: format!("shop:{}", self.id),
            rows,
        }
    }

    /// Build the effective [`ShopProfile`] for checking a part of the given
    /// material/thickness against this shop.
    ///
    /// Falls back to row-independent defaults when the material/thickness
    /// isn't in the catalog (the check will additionally flag the unknown
    /// stock through the radius rule, since `fixed_bend_radius_mm` stays
    /// `None`).
    pub fn shop_profile_for(&self, material: &str, thickness_mm: f64) -> ShopProfile {
        let row = self.row(material, thickness_mm).ok();
        ShopProfile {
            name: self.name.clone(),
            max_bend_length_mm: row.map_or(self.max_part_bend_mm[1], |r| r.max_bend_length_mm),
            // Radius is fixed per stock, not a ratio floor.
            min_bend_radius_ratio: 0.0,
            min_flange_height_mm: row.map_or(ShopProfile::generic().min_flange_height_mm, |r| {
                r.min_flange_formed_mm
            }),
            // Cut features inside half the die width of a bend distort.
            min_hole_to_bend_mm: row.map_or(ShopProfile::generic().min_hole_to_bend_mm, |r| {
                r.die_width_mm * 0.5
            }),
            min_distance_between_bends_mm: row
                .map_or(ShopProfile::generic().min_distance_between_bends_mm, |r| {
                    r.die_width_mm
                }),
            die_width_mm: row.map(|r| r.die_width_mm),
            relief_width_mm: None, // formula default ≥ shop's 0.5·t minimum
            relief_depth_mm: row.map(|r| r.relief_depth_mm),
            fixed_bend_radius_mm: row.map(|r| r.inside_radius_mm),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_parses_and_has_materials() {
        let cat = shop_catalog("sendcutsend").unwrap();
        assert_eq!(cat.id, "sendcutsend");
        assert_eq!(cat.retrieved, "2026-06-10");
        assert!(cat.materials.len() >= 8);
        assert!(!cat.sources.is_empty());
    }

    #[test]
    fn unknown_shop_lists_available() {
        match shop_catalog("nope") {
            Err(ShopLookupError::UnknownShop { available, .. }) => {
                assert_eq!(available, vec!["sendcutsend".to_string()]);
            }
            other => panic!("expected UnknownShop, got {other:?}"),
        }
    }

    #[test]
    fn material_aliases_resolve() {
        let cat = shop_catalog("sendcutsend").unwrap();
        for alias in ["al-5052", "AL-SOFT", "aluminum", "5052"] {
            assert!(cat.material(alias).is_some(), "alias {alias} should hit");
        }
        assert!(cat.material("unobtanium").is_none());
        // 6061 is cut but not bent — deliberately absent.
        assert!(cat.material("6061").is_none());
    }

    #[test]
    fn row_matches_nearest_gauge_within_tolerance() {
        let cat = shop_catalog("sendcutsend").unwrap();
        // 1.6 mm 5052 exists exactly.
        let row = cat.row("al-5052", 1.6).unwrap();
        assert!((row.inside_radius_mm - 0.89).abs() < 1e-9);
        assert!((row.k_factor - 0.42).abs() < 1e-9);
        // 1.5 mm is within 0.13 mm of the 1.6 gauge.
        assert!(cat.row("al-5052", 1.5).is_ok());
        // 1.3 mm is not stocked.
        match cat.row("al-5052", 1.3) {
            Err(ShopLookupError::UnknownThickness { available_mm, .. }) => {
                assert!(available_mm.contains(&1.6));
            }
            other => panic!("expected UnknownThickness, got {other:?}"),
        }
    }

    #[test]
    fn resolve_bend_uses_fixed_radius_and_k() {
        let cat = shop_catalog("sendcutsend").unwrap();
        let (r, k, label) = cat.resolve_bend("aluminum", 1.6, None).unwrap();
        assert!((r - 0.89).abs() < 1e-9);
        assert!((k - 0.42).abs() < 1e-9);
        assert!(label.starts_with("shop:sendcutsend/al-5052"), "{label}");
    }

    #[test]
    fn resolve_bend_rejects_custom_radius_naming_the_fixed_one() {
        let cat = shop_catalog("sendcutsend").unwrap();
        match cat.resolve_bend("al-5052", 1.6, Some(2.0)) {
            Err(ShopLookupError::FixedRadius {
                fixed_mm,
                requested_mm,
                ..
            }) => {
                assert!((fixed_mm - 0.89).abs() < 1e-9);
                assert!((requested_mm - 2.0).abs() < 1e-9);
            }
            other => panic!("expected FixedRadius, got {other:?}"),
        }
        // Matching the fixed radius (within 0.02 mm) is accepted.
        assert!(cat.resolve_bend("al-5052", 1.6, Some(0.9)).is_ok());
    }

    #[test]
    fn shop_profile_for_carries_row_data() {
        let cat = shop_catalog("sendcutsend").unwrap();
        let p = cat.shop_profile_for("steel-mild", 1.5);
        assert_eq!(p.name, "SendCutSend");
        assert!((p.min_flange_height_mm - 7.85).abs() < 1e-9);
        assert_eq!(p.die_width_mm, Some(12.0));
        assert_eq!(p.fixed_bend_radius_mm, Some(1.6));
        assert!((p.min_hole_to_bend_mm - 6.0).abs() < 1e-9);
        assert!((p.relief_depth_mm.unwrap() - 3.61).abs() < 1e-9);
    }

    #[test]
    fn bend_table_projection_round_trips() {
        let cat = shop_catalog("sendcutsend").unwrap();
        let table = cat.bend_table();
        assert_eq!(table.id, "shop:sendcutsend");
        assert!(table.rows.len() > 40);
        let (k, _) = table.lookup("al-5052", 1.6, 0.89).unwrap();
        assert!((k - 0.42).abs() < 1e-9);
    }
}
