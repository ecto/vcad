//! Bend tables: queryable `(material, t, R, ...) → (BA, K, springback)`.
//!
//! Replaces the "single global K-factor lie" with a structured, provenanced
//! lookup. Every bend in a model carries a [`KFactorSource`] pointing back
//! to the table row that produced its allowance, so changing the table
//! propagates to the model deterministically.
//!
//! For the foundation tier we ship only the math (`BA = θ·(R + K·t)`) and a
//! tiny built-in table of common K-factors. The full registry — community
//! submissions, measured-vs-predicted residuals, shop overrides — lives in
//! `vcad-kernel-bend-tables` (later tier).

/// Result of a bend-allowance computation, with provenance.
#[derive(Debug, Clone, PartialEq)]
pub struct BendAllowance {
    /// Bend allowance: arc length of the neutral axis through the bend (mm).
    pub ba: f64,
    /// Bend deduction: amount subtracted from theoretical sharp-corner
    /// flange-sum to get the flat-pattern length (mm).
    pub bd: f64,
    /// K-factor used.
    pub k_factor: f64,
    /// Where this K came from (e.g. `"builtin:Al-soft/R1.0t1.0"`).
    pub source: String,
}

/// Where a K-factor was sourced from. Drives the colored provenance dot in
/// the property panel.
#[derive(Debug, Clone, PartialEq)]
pub enum KFactorSource {
    /// Built-in default table (green dot in UI).
    Builtin {
        /// Stable key into the built-in table (e.g. `"Al-soft/R1.0t1.0"`).
        key: String,
    },
    /// Shop-provided override table (blue dot in UI).
    Shop {
        /// Identifier of the shop profile.
        shop_id: String,
        /// Row key inside the shop's table.
        key: String,
    },
    /// Measured-on-coupons override (purple dot in UI).
    Measured {
        /// Free-form note (e.g. operator initials, date, coupon batch).
        note: String,
    },
    /// Manual user override (no provenance — surfaced as a warning).
    Manual,
}

impl KFactorSource {
    /// Render to the short string stored on each [`crate::Bend`].
    pub fn label(&self) -> String {
        match self {
            KFactorSource::Builtin { key } => format!("builtin:{key}"),
            KFactorSource::Shop { shop_id, key } => format!("shop:{shop_id}/{key}"),
            KFactorSource::Measured { note } => format!("measured:{note}"),
            KFactorSource::Manual => "manual".to_string(),
        }
    }
}

/// A queryable bend table. The foundation-tier implementation is a flat list
/// of rows; later tiers replace this with an interpolating model.
#[derive(Debug, Clone, PartialEq)]
pub struct BendTable {
    /// Identifier (e.g. `"builtin"`, `"shop:acme-machining"`).
    pub id: String,
    /// Rows.
    pub rows: Vec<BendTableRow>,
}

/// A single row in a [`BendTable`].
#[derive(Debug, Clone, PartialEq)]
pub struct BendTableRow {
    /// Material name (free-form for now; later: a typed registry).
    pub material: String,
    /// Material thickness (mm).
    pub thickness: f64,
    /// Inside bend radius (mm).
    pub radius: f64,
    /// K-factor.
    pub k_factor: f64,
}

impl BendTableRow {
    /// `R/t` ratio.
    pub fn r_over_t(&self) -> f64 {
        self.radius / self.thickness
    }
}

impl BendTable {
    /// Build the curated default table.
    ///
    /// Sources: Machinery's Handbook + DIN 6935 typical values. These are
    /// **starting points**; real shops calibrate against measured coupons
    /// and contribute corrections back to the open registry.
    pub fn builtin() -> Self {
        // Material × thickness × radius → K. K varies primarily with R/t and
        // material hardness; we encode a tractable cross-section.
        let rows = vec![
            // Aluminum (soft, e.g. 1100, 3003)
            row("Al-soft", 1.0, 0.5, 0.33),
            row("Al-soft", 1.0, 1.0, 0.35),
            row("Al-soft", 1.0, 2.0, 0.37),
            row("Al-soft", 1.0, 3.0, 0.38),
            row("Al-soft", 1.5, 1.5, 0.35),
            row("Al-soft", 2.0, 2.0, 0.36),
            // Aluminum (hard, e.g. 6061-T6)
            row("Al-hard", 1.0, 1.0, 0.40),
            row("Al-hard", 1.0, 2.0, 0.42),
            row("Al-hard", 1.5, 1.5, 0.41),
            row("Al-hard", 2.0, 3.0, 0.44),
            // Mild steel (CRS, A36)
            row("Steel-mild", 1.0, 1.0, 0.40),
            row("Steel-mild", 1.0, 2.0, 0.43),
            row("Steel-mild", 1.5, 1.5, 0.42),
            row("Steel-mild", 2.0, 2.0, 0.44),
            row("Steel-mild", 3.0, 3.0, 0.45),
            // Stainless 304
            row("SS-304", 1.0, 1.0, 0.44),
            row("SS-304", 1.0, 2.0, 0.47),
            row("SS-304", 1.5, 1.5, 0.45),
            row("SS-304", 2.0, 2.0, 0.47),
        ];
        Self {
            id: "builtin".to_string(),
            rows,
        }
    }

    /// Look up the K-factor for a `(material, thickness, radius)` query.
    ///
    /// Returns the K-factor and a [`KFactorSource`] tagging the row used.
    /// Falls back to the **closest row by `R/t` for that material** when no
    /// exact match exists; if the material is unknown, returns `None` and
    /// the caller should fall back to a manual K-factor (with a warning in
    /// the UI).
    pub fn lookup(
        &self,
        material: &str,
        thickness: f64,
        radius: f64,
    ) -> Option<(f64, KFactorSource)> {
        let target_rt = radius / thickness;
        let mut best: Option<(&BendTableRow, f64)> = None;
        for row in &self.rows {
            if row.material != material {
                continue;
            }
            let dist = (row.r_over_t() - target_rt).abs() + (row.thickness - thickness).abs() * 0.1;
            match best {
                None => best = Some((row, dist)),
                Some((_, d)) if dist < d => best = Some((row, dist)),
                _ => {}
            }
        }
        best.map(|(row, _)| {
            let key = format!("{}/R{:.2}t{:.2}", row.material, row.radius, row.thickness);
            (row.k_factor, KFactorSource::Builtin { key })
        })
    }
}

fn row(material: &'static str, thickness: f64, radius: f64, k_factor: f64) -> BendTableRow {
    BendTableRow {
        material: material.to_string(),
        thickness,
        radius,
        k_factor,
    }
}

/// Compute the bend allowance for an angle/radius/K-factor/thickness.
///
/// `BA = θ · (R + K · t)`. The sign of `θ` is ignored (always positive
/// allowance for any non-zero bend).
pub fn bend_allowance(angle_rad: f64, radius: f64, k_factor: f64, thickness: f64) -> f64 {
    angle_rad.abs() * (radius + k_factor * thickness)
}

/// Compute the bend deduction for an angle/radius/K-factor/thickness.
///
/// `BD = 2(R + t) · tan(θ/2) - BA`.
pub fn bend_deduction(angle_rad: f64, radius: f64, k_factor: f64, thickness: f64) -> f64 {
    let ba = bend_allowance(angle_rad, radius, k_factor, thickness);
    2.0 * (radius + thickness) * (angle_rad.abs() / 2.0).tan() - ba
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::FRAC_PI_2;

    #[test]
    fn allowance_at_90_deg_matches_classic_formula() {
        // 90° bend, R=1, K=0.4, t=1 → BA = (π/2)·1.4
        let ba = bend_allowance(FRAC_PI_2, 1.0, 0.4, 1.0);
        let expected = FRAC_PI_2 * 1.4;
        assert!((ba - expected).abs() < 1e-12);
    }

    #[test]
    fn allowance_is_zero_for_zero_angle() {
        assert!(bend_allowance(0.0, 1.0, 0.4, 1.0).abs() < 1e-15);
    }

    #[test]
    fn deduction_consistent_with_setback() {
        // For 90° bend: OSSB = (R + t)·tan(45°) = R + t
        // BD should equal 2·OSSB - BA
        let r = 1.5;
        let t = 1.0;
        let k = 0.42;
        let bd = bend_deduction(FRAC_PI_2, r, k, t);
        let ba = bend_allowance(FRAC_PI_2, r, k, t);
        let ossb_2 = 2.0 * (r + t) * (FRAC_PI_2 / 2.0).tan();
        assert!((bd - (ossb_2 - ba)).abs() < 1e-12);
    }

    #[test]
    fn builtin_table_has_expected_materials() {
        let t = BendTable::builtin();
        for mat in ["Al-soft", "Al-hard", "Steel-mild", "SS-304"] {
            assert!(t.rows.iter().any(|r| r.material == mat), "missing {mat}");
        }
    }

    #[test]
    fn lookup_returns_provenance() {
        let t = BendTable::builtin();
        let (k, src) = t.lookup("Al-soft", 1.0, 1.0).expect("should find row");
        assert!((k - 0.35).abs() < 1e-12);
        match src {
            KFactorSource::Builtin { key } => {
                assert!(key.starts_with("Al-soft/"), "got key {key}");
            }
            other => panic!("expected Builtin, got {other:?}"),
        }
    }

    #[test]
    fn lookup_unknown_material_returns_none() {
        let t = BendTable::builtin();
        assert!(t.lookup("Unobtanium", 1.0, 1.0).is_none());
    }

    #[test]
    fn lookup_falls_back_to_closest_rt() {
        let t = BendTable::builtin();
        // No exact row for Al-soft R=1.7, t=1.0 — should pick the closest.
        let (k, _) = t.lookup("Al-soft", 1.0, 1.7).expect("should find a row");
        assert!(k > 0.34 && k < 0.40, "K out of plausible range: {k}");
    }

    #[test]
    fn k_factor_source_label_round_trips_kind() {
        assert_eq!(
            KFactorSource::Builtin { key: "x".into() }.label(),
            "builtin:x"
        );
        assert_eq!(
            KFactorSource::Shop {
                shop_id: "acme".into(),
                key: "x".into()
            }
            .label(),
            "shop:acme/x"
        );
        assert_eq!(
            KFactorSource::Measured { note: "n".into() }.label(),
            "measured:n"
        );
        assert_eq!(KFactorSource::Manual.label(), "manual");
    }
}
