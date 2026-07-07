//! Sheet-metal adapter for the unified [`DesignReceipt`].
//!
//! Translates [`check_manufacturability`](crate::manufacturability::check_manufacturability)
//! findings and [`estimate_cost`](crate::cost::estimate_cost) breakdowns into
//! [`ReceiptClaim`]s. Every rule the checker knows is claimed explicitly —
//! a rule that found nothing becomes an affirmative pass claim, so the
//! receipt states what was checked, not merely what failed.

use crate::cost::CostBreakdown;
use crate::manufacturability::{Severity, ShopProfile, Violation};
use vcad_receipt::{ClaimQuantity, DesignReceipt, OracleRef, ReceiptClaim};

/// Domain tag for sheet-metal claims.
pub const DOMAIN: &str = "sheet_metal";

/// Every rule id [`check_manufacturability`](crate::manufacturability::check_manufacturability)
/// can emit, in stable order. Kept in sync with [`Violation::rule`] by
/// `all_rules_covers_every_violation_variant` below.
pub const ALL_RULES: [&str; 7] = [
    "sheet.bend_radius",
    "sheet.brake_capacity",
    "sheet.flange_height",
    "sheet.hole_to_bend",
    "sheet.bend_to_bend",
    "sheet.bend_relief",
    "sheet.bend_radius_fixed",
];

fn oracle() -> OracleRef {
    OracleRef::new(
        "vcad-kernel-sheet/manufacturability",
        env!("CARGO_PKG_VERSION"),
    )
}

fn cost_oracle() -> OracleRef {
    OracleRef::new("vcad-kernel-sheet/cost", env!("CARGO_PKG_VERSION"))
}

/// Measured-vs-required pair for a violation, when the variant carries one.
/// `MissingBendRelief` prescribes a notch instead of measuring a distance.
fn actual_required(v: &Violation) -> Option<(f64, f64)> {
    match v {
        Violation::BendRadiusBelowMinimum {
            actual_mm,
            required_mm,
            ..
        }
        | Violation::BendExceedsBrakeCapacity {
            actual_mm,
            required_mm,
            ..
        }
        | Violation::FlangeBelowMinHeight {
            actual_mm,
            required_mm,
            ..
        }
        | Violation::HoleTooCloseToBend {
            actual_mm,
            required_mm,
            ..
        }
        | Violation::BendRadiusNotFixed {
            actual_mm,
            required_mm,
            ..
        }
        | Violation::BendsTooClose {
            actual_mm,
            required_mm,
            ..
        } => Some((*actual_mm, *required_mm)),
        Violation::MissingBendRelief { .. } => None,
    }
}

fn subject(v: &Violation) -> String {
    match v {
        Violation::BendRadiusBelowMinimum { bend_id, .. }
        | Violation::BendExceedsBrakeCapacity { bend_id, .. }
        | Violation::FlangeBelowMinHeight { bend_id, .. }
        | Violation::HoleTooCloseToBend { bend_id, .. }
        | Violation::MissingBendRelief { bend_id, .. }
        | Violation::BendRadiusNotFixed { bend_id, .. } => format!("bend:{bend_id}"),
        Violation::BendsTooClose {
            bend_id_a,
            bend_id_b,
            ..
        } => format!("bend:{bend_id_a}+bend:{bend_id_b}"),
    }
}

/// One claim per known manufacturability rule.
///
/// Rules with no findings pass; rules with findings fail, carrying the first
/// (deterministically ordered) finding's measured/required pair, its subject,
/// and the total count. A `Warning`-severity finding still fails its claim —
/// "shop-ready" means zero findings — with the severity recorded in details.
pub fn manufacturability_claims(shop: &ShopProfile, violations: &[Violation]) -> Vec<ReceiptClaim> {
    ALL_RULES
        .iter()
        .map(|rule| {
            let hits: Vec<&Violation> = violations.iter().filter(|v| v.rule() == *rule).collect();
            match hits.first() {
                None => ReceiptClaim::pass(*rule, DOMAIN, format!("no {rule} findings"), oracle())
                    .with_details(format!("checked against shop profile '{}'", shop.name)),
                Some(first) => {
                    let severity = match first.severity() {
                        Severity::Error => "error",
                        Severity::Warning => "warning",
                    };
                    let mut c = ReceiptClaim::fail(*rule, DOMAIN, first.message(), oracle())
                        .with_subject(subject(first))
                        .with_details(format!(
                            "{} finding(s), severity {severity}, shop profile '{}'",
                            hits.len(),
                            shop.name
                        ));
                    if let Some((actual, required)) = actual_required(first) {
                        c = c
                            .with_predicted(ClaimQuantity::new(required, "mm"))
                            .with_measured(ClaimQuantity::new(actual, "mm"));
                    }
                    c
                }
            }
        })
        .collect()
}

/// Cost claims from a [`CostBreakdown`].
///
/// The unit-price claim is a certification (the oracle vouches for the
/// number); pass `budget_each` to turn it into a pass/fail bound. A
/// non-finite or non-positive total is unverifiable, never a silent pass.
pub fn cost_claims(breakdown: &CostBreakdown, budget_each: Option<f64>) -> Vec<ReceiptClaim> {
    let mut claims = Vec::new();

    let total = breakdown.total_each;
    if !total.is_finite() || total <= 0.0 {
        claims.push(ReceiptClaim::unverifiable(
            "sheet.cost.unit_price",
            DOMAIN,
            "unit price",
            cost_oracle(),
            format!("cost model produced a non-physical unit price ({total})"),
        ));
    } else {
        let mut c = ReceiptClaim::pass(
            "sheet.cost.unit_price",
            DOMAIN,
            format!("unit price at qty {}", breakdown.quantity),
            cost_oracle(),
        )
        .with_measured(ClaimQuantity::new(total, breakdown.currency.clone()));
        if let Some(budget) = budget_each {
            c = c.with_predicted(ClaimQuantity::new(budget, breakdown.currency.clone()));
            if total > budget {
                c.verdict = vcad_receipt::ClaimVerdict::Fail;
            }
        }
        claims.push(c);
    }

    if breakdown.mass_kg_each.is_finite() && breakdown.mass_kg_each > 0.0 {
        claims.push(
            ReceiptClaim::pass("sheet.mass", DOMAIN, "mass per part", cost_oracle())
                .with_measured(ClaimQuantity::new(breakdown.mass_kg_each * 1000.0, "g")),
        );
    }

    claims
}

/// Build a full sheet-metal [`DesignReceipt`]: one claim per
/// manufacturability rule, plus cost claims when a breakdown is supplied.
pub fn sheet_metal_receipt(
    shop: &ShopProfile,
    violations: &[Violation],
    cost: Option<&CostBreakdown>,
) -> DesignReceipt {
    let mut claims = manufacturability_claims(shop, violations);
    if let Some(breakdown) = cost {
        claims.extend(cost_claims(breakdown, None));
    }
    DesignReceipt::with_claims(claims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_flange::base_flange_rect;
    use crate::bend_table::BendTable;
    use crate::cost::{estimate_cost, CostRates};
    use crate::edge_flange::{add_edge_flange, EdgeFlangeParams, FlangePosition};
    use crate::manufacturability::check_manufacturability;
    use crate::model::{BendDirection, SheetMetalModel};
    use crate::unfold::{unfold, FlatPattern};
    use std::f64::consts::FRAC_PI_2;
    use vcad_receipt::ClaimVerdict;

    fn l_bracket() -> (SheetMetalModel, FlatPattern) {
        let mut m = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        m.material = "al-soft".to_string();
        let table = BendTable::builtin();
        add_edge_flange(
            &mut m,
            &table,
            EdgeFlangeParams {
                panel: 0,
                edge_index: 0,
                length: 25.0,
                angle: FRAC_PI_2,
                radius: 1.0,
                direction: BendDirection::Up,
                position: FlangePosition::MaterialInside,
                material: "al-soft".into(),
                manual_k: None,
            },
        )
        .unwrap();
        unfold(&mut m).unwrap();
        let flat = FlatPattern::from_model(&m);
        (m, flat)
    }

    #[test]
    fn all_rules_covers_every_violation_variant() {
        // Construct one of each variant and confirm its rule id is listed.
        let variants = vec![
            Violation::BendRadiusBelowMinimum {
                bend_id: 0,
                actual_mm: 0.5,
                required_mm: 1.0,
                material: "al".into(),
                source: crate::manufacturability::BendRadiusSource::Material,
            },
            Violation::BendExceedsBrakeCapacity {
                bend_id: 0,
                actual_mm: 4000.0,
                required_mm: 3000.0,
            },
            Violation::FlangeBelowMinHeight {
                bend_id: 0,
                panel_id: 0,
                actual_mm: 2.0,
                required_mm: 6.0,
            },
            Violation::HoleTooCloseToBend {
                bend_id: 0,
                panel_id: 0,
                hole_index: 0,
                actual_mm: 1.0,
                required_mm: 3.0,
            },
            Violation::MissingBendRelief {
                bend_id: 0,
                panel_id: 0,
                end: 0,
                required_width_mm: 2.0,
                required_depth_mm: 3.0,
            },
            Violation::BendRadiusNotFixed {
                bend_id: 0,
                actual_mm: 2.0,
                required_mm: 1.0,
            },
            Violation::BendsTooClose {
                bend_id_a: 0,
                bend_id_b: 1,
                actual_mm: 3.0,
                required_mm: 10.0,
            },
        ];
        for v in &variants {
            assert!(
                ALL_RULES.contains(&v.rule()),
                "rule {} missing from ALL_RULES",
                v.rule()
            );
        }
        assert_eq!(variants.len(), ALL_RULES.len());
    }

    #[test]
    fn clean_part_yields_affirmative_pass_claims() {
        let shop = ShopProfile::generic();
        let claims = manufacturability_claims(&shop, &[]);
        assert_eq!(claims.len(), ALL_RULES.len());
        assert!(claims.iter().all(|c| c.verdict == ClaimVerdict::Pass));
        // pass claims still say what they were checked against
        assert!(claims[0].details.as_deref().unwrap().contains(&shop.name));
        let receipt = DesignReceipt::with_claims(claims);
        assert_eq!(receipt.overall(), ClaimVerdict::Pass);
    }

    #[test]
    fn violation_fails_its_rule_claim_with_values() {
        let shop = ShopProfile::generic();
        let violations = vec![Violation::BendRadiusBelowMinimum {
            bend_id: 2,
            actual_mm: 0.4,
            required_mm: 1.6,
            material: "aluminum".into(),
            source: crate::manufacturability::BendRadiusSource::Material,
        }];
        let claims = manufacturability_claims(&shop, &violations);
        let claim = claims.iter().find(|c| c.id == "sheet.bend_radius").unwrap();
        assert_eq!(claim.verdict, ClaimVerdict::Fail);
        assert_eq!(claim.subject.as_deref(), Some("bend:2"));
        assert_eq!(
            claim.measured.as_ref().unwrap().value,
            vcad_receipt::ClaimValue::Number(0.4)
        );
        assert_eq!(
            claim.predicted.as_ref().unwrap().value,
            vcad_receipt::ClaimValue::Number(1.6)
        );
        assert_eq!(claim.measured.as_ref().unwrap().unit.as_deref(), Some("mm"));
        // other rules still pass — the receipt fails overall
        let receipt = DesignReceipt::with_claims(claims);
        assert_eq!(receipt.overall(), ClaimVerdict::Fail);
    }

    #[test]
    fn relief_violation_fails_without_fabricating_numbers() {
        let shop = ShopProfile::generic();
        let violations = vec![Violation::MissingBendRelief {
            bend_id: 1,
            panel_id: 0,
            end: 1,
            required_width_mm: 2.0,
            required_depth_mm: 3.4,
        }];
        let claims = manufacturability_claims(&shop, &violations);
        let claim = claims.iter().find(|c| c.id == "sheet.bend_relief").unwrap();
        assert_eq!(claim.verdict, ClaimVerdict::Fail);
        assert!(claim.measured.is_none());
        assert!(claim.predicted.is_none());
    }

    #[test]
    fn end_to_end_model_receipt() {
        // Real model through the real checker and cost model.
        let (model, flat) = l_bracket();
        let shop = ShopProfile::generic();
        let violations = check_manufacturability(&model, &shop);
        let cost = estimate_cost(&model, &flat, 10, &CostRates::generic());

        let receipt = sheet_metal_receipt(&shop, &violations, Some(&cost));
        assert_eq!(receipt.schema, vcad_receipt::RECEIPT_SCHEMA);
        // one claim per rule + at least the unit-price claim
        assert!(receipt.claims.len() > ALL_RULES.len());
        let price = receipt
            .claims
            .iter()
            .find(|c| c.id == "sheet.cost.unit_price")
            .unwrap();
        assert_eq!(price.verdict, ClaimVerdict::Pass);
        assert_eq!(
            price.measured.as_ref().unwrap().unit.as_deref(),
            Some(cost.currency.as_str())
        );
        // the rollup is decided by the checker, never by omission
        match receipt.overall() {
            ClaimVerdict::Pass | ClaimVerdict::Fail => {}
            ClaimVerdict::Unverifiable => panic!("real run must be verifiable"),
        }
    }

    #[test]
    fn over_budget_unit_price_fails() {
        let (model, flat) = l_bracket();
        let cost = estimate_cost(&model, &flat, 1, &CostRates::generic());
        let claims = cost_claims(&cost, Some(cost.total_each / 2.0));
        assert_eq!(claims[0].verdict, ClaimVerdict::Fail);
        let claims = cost_claims(&cost, Some(cost.total_each * 2.0));
        assert_eq!(claims[0].verdict, ClaimVerdict::Pass);
    }
}
