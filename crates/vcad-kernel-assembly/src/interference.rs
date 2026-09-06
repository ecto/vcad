//! Interference checking over a posed assembly.
//!
//! Two stages: an AABB broad phase over the world-space bounds of every posed
//! part, then [`vcad_kernel_tessellate::clearance::mesh_clearance`] on the
//! surviving pairs — a triangle-BVH branch-and-bound that returns a signed
//! distance, negative by the depth of the deepest penetrating vertex.
//!
//! The tolerance matters as much as the check. Real models carry *deliberate*
//! small overlaps — a part modelled 0.05 mm into its neighbour so the union
//! prints without a hairline seam — so a checker that flags every contact is
//! noise. [`InterferenceOptions::tolerance_mm`] is the depth below which an
//! overlap is accepted as modelling slop rather than reported as a clash.

use crate::pose::{mesh_bounds, PosedAssembly};
use vcad_kernel_tessellate::clearance::mesh_clearance;

/// Knobs for [`check_interference`].
#[derive(Debug, Clone, PartialEq)]
pub struct InterferenceOptions {
    /// Overlap depth, in mm, that is accepted without being reported.
    ///
    /// Set this to the model's intentional-overlap budget (rana uses 0.05 mm)
    /// so a real interference stands out from deliberate seam closure.
    pub tolerance_mm: f64,
    /// Broad-phase AABB padding in mm. Pairs whose padded bounds miss each
    /// other are skipped without a narrow-phase query.
    pub broadphase_margin_mm: f64,
    /// Instance-id pairs to skip entirely, in either order — for parts that
    /// are *supposed* to interpenetrate (a press fit, a threaded insert).
    pub ignore_pairs: Vec<(String, String)>,
}

impl Default for InterferenceOptions {
    fn default() -> Self {
        Self {
            tolerance_mm: 0.0,
            broadphase_margin_mm: 0.0,
            ignore_pairs: Vec::new(),
        }
    }
}

impl InterferenceOptions {
    /// Options accepting overlaps up to `mm` deep.
    pub fn with_tolerance(mm: f64) -> Self {
        Self {
            tolerance_mm: mm,
            ..Self::default()
        }
    }

    fn ignores(&self, a: &str, b: &str) -> bool {
        self.ignore_pairs
            .iter()
            .any(|(x, y)| (x == a && y == b) || (x == b && y == a))
    }
}

/// One reported overlapping pair.
#[derive(Debug, Clone, PartialEq)]
pub struct Interference {
    /// First instance id.
    pub instance_a: String,
    /// Second instance id.
    pub instance_b: String,
    /// Approximate overlap depth in mm — the deepest penetration found by the
    /// narrow phase. Positive by construction.
    pub depth_mm: f64,
    /// A point inside the overlap, on the deeper mesh's surface.
    pub witness: [f64; 3],
}

/// Result of an interference sweep.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct InterferenceReport {
    /// Overlaps deeper than the tolerance, deepest first.
    pub pairs: Vec<Interference>,
    /// Pairs that survived the broad phase and were measured.
    pub pairs_tested: usize,
    /// Overlaps found but accepted as within tolerance.
    pub pairs_within_tolerance: usize,
    /// The tolerance the sweep ran with, in mm.
    pub tolerance_mm: f64,
}

impl InterferenceReport {
    /// True when nothing overlapped beyond the tolerance.
    pub fn is_clean(&self) -> bool {
        self.pairs.is_empty()
    }

    /// Deepest reported overlap, if any.
    pub fn worst(&self) -> Option<&Interference> {
        self.pairs.first()
    }

    /// Multi-line report form.
    pub fn summary(&self) -> String {
        if self.is_clean() {
            return format!(
                "no interference: {} pair(s) tested, {} overlap(s) within the \
                 {:.3} mm tolerance",
                self.pairs_tested, self.pairs_within_tolerance, self.tolerance_mm
            );
        }
        let mut s = format!(
            "{} interfering pair(s) of {} tested (tolerance {:.3} mm):",
            self.pairs.len(),
            self.pairs_tested,
            self.tolerance_mm
        );
        for p in &self.pairs {
            s.push_str(&format!(
                "\n  {} ∩ {} — {:.4} mm deep at ({:.2}, {:.2}, {:.2})",
                p.instance_a, p.instance_b, p.depth_mm, p.witness[0], p.witness[1], p.witness[2]
            ));
        }
        s
    }
}

/// Find every pair of posed parts that overlaps by more than the tolerance.
pub fn check_interference(
    posed: &PosedAssembly,
    options: &InterferenceOptions,
) -> InterferenceReport {
    let bounds: Vec<Option<([f64; 3], [f64; 3])>> =
        posed.parts.iter().map(|p| mesh_bounds(&p.mesh)).collect();

    let mut report = InterferenceReport {
        tolerance_mm: options.tolerance_mm,
        ..Default::default()
    };

    for i in 0..posed.parts.len() {
        for j in (i + 1)..posed.parts.len() {
            let (a, b) = (&posed.parts[i], &posed.parts[j]);
            if options.ignores(&a.instance_id, &b.instance_id) {
                continue;
            }
            let (Some(ba), Some(bb)) = (&bounds[i], &bounds[j]) else {
                continue;
            };
            if !aabb_overlap(ba, bb, options.broadphase_margin_mm) {
                continue;
            }
            report.pairs_tested += 1;

            let Some(result) = mesh_clearance(&a.mesh, &b.mesh) else {
                continue;
            };
            if !result.intersecting {
                continue;
            }
            // `distance` is negative by the deepest penetration when the
            // meshes cross, and 0.0 for a bare touch the depth pass cannot
            // resolve. Either way the overlap depth is its magnitude.
            let depth = -result.distance;
            if depth <= options.tolerance_mm {
                report.pairs_within_tolerance += 1;
                continue;
            }
            report.pairs.push(Interference {
                instance_a: a.instance_id.clone(),
                instance_b: b.instance_id.clone(),
                depth_mm: depth,
                witness: result.point_a,
            });
        }
    }

    report
        .pairs
        .sort_by(|x, y| y.depth_mm.total_cmp(&x.depth_mm));
    report
}

fn aabb_overlap(a: &([f64; 3], [f64; 3]), b: &([f64; 3], [f64; 3]), margin: f64) -> bool {
    (0..3).all(|k| a.0[k] - margin <= b.1[k] + margin && b.0[k] - margin <= a.1[k] + margin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disjoint_boxes_fail_the_broad_phase() {
        let a = ([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let b = ([2.0, 0.0, 0.0], [3.0, 1.0, 1.0]);
        assert!(!aabb_overlap(&a, &b, 0.0));
        assert!(aabb_overlap(&a, &b, 0.6));
    }
}
