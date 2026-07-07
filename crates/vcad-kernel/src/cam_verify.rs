//! CAM verification oracle: simulate toolpaths against stock and grade the
//! result against a target [`Solid`].
//!
//! This bridges `vcad-kernel-cam` (toolpath generation) and
//! `vcad-kernel-stocksim` (octree material-removal simulation): run the
//! toolpath through the stock, then check the two CAM invariants against
//! the target part — **no gouge** (the toolpath never removed material the
//! part needs) and **no excess** (the toolpath removed everything it was
//! supposed to, within the machining allowance).
//!
//! The [`StockVerification`] result is plain serializable data, suitable
//! for embedding in a build receipt.

use vcad_kernel_cam::{Tool, Toolpath};
use vcad_kernel_stocksim::{
    verify_stock_against_mesh, Stock, StockSimError, StockVerification, VerifyOptions,
};

use crate::Solid;

/// Simulate `passes` (tool + toolpath, in cutting order) against a stock
/// block and verify the remaining material against `target`.
///
/// * `target` — the part the toolpaths are supposed to produce, in the same
///   frame as the toolpath coordinates. CAM convention puts the stock top
///   at Z = 0 with material below, so a target modeled at the origin
///   usually needs to be translated down into the stock first.
/// * `stock_bounds` — `[min_x, min_y, min_z, max_x, max_y, max_z]` of the
///   stock block (mm).
/// * `resolution` — simulation cell size (mm). Violations thinner than a
///   cell can escape detection, so keep this below the finest defect that
///   matters.
/// * `passes` — each entry pairs the tool with the toolpath it cuts.
pub fn verify_toolpaths(
    target: &Solid,
    stock_bounds: [f64; 6],
    resolution: f64,
    passes: &[(Tool, Toolpath)],
    opts: &VerifyOptions,
) -> Result<StockVerification, StockSimError> {
    let dx = stock_bounds[3] - stock_bounds[0];
    let dy = stock_bounds[4] - stock_bounds[1];
    let dz = stock_bounds[5] - stock_bounds[2];
    if dx <= 0.0 || dy <= 0.0 || dz <= 0.0 {
        return Err(StockSimError::InvalidBounds(format!(
            "stock must have positive extent, got {dx} x {dy} x {dz}"
        )));
    }
    if resolution <= 0.0 || !resolution.is_finite() {
        return Err(StockSimError::ResolutionTooSmall(resolution));
    }

    let mut stock = Stock::from_box(stock_bounds, resolution);
    for (tool, toolpath) in passes {
        stock.subtract_toolpath(tool, toolpath);
    }

    let mesh = target.to_mesh(32);
    Ok(verify_stock_against_mesh(
        &stock,
        &mesh.vertices,
        &mesh.indices,
        opts,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_cam::{CamSettings, Face, ToolpathSegment};
    use vcad_kernel_stocksim::StockVerification;

    /// 50 x 50 x 10 stock, CAM convention: top at Z = 0, material below.
    const STOCK: [f64; 6] = [0.0, 0.0, -10.0, 50.0, 50.0, 0.0];

    /// The part: the stock with its top 2mm faced off.
    fn target_block() -> Solid {
        Solid::cube(50.0, 50.0, 8.0).translate(0.0, 0.0, -10.0)
    }

    fn flat_tool() -> Tool {
        Tool::FlatEndMill {
            diameter: 6.0,
            flute_length: 20.0,
            flutes: 2,
        }
    }

    fn settings() -> CamSettings {
        CamSettings {
            stepover: 4.0,
            stepdown: 2.0,
            feed_rate: 1000.0,
            plunge_rate: 300.0,
            spindle_rpm: 12000.0,
            safe_z: 5.0,
            retract_z: 10.0,
        }
    }

    fn facing_toolpath(depth: f64) -> Toolpath {
        Face::new(0.0, 0.0, 50.0, 50.0, depth)
            .generate(&flat_tool(), &settings())
            .unwrap()
    }

    fn verify(passes: &[(Tool, Toolpath)]) -> StockVerification {
        verify_toolpaths(
            &target_block(),
            STOCK,
            1.0,
            passes,
            &VerifyOptions::default(),
        )
        .unwrap()
    }

    #[test]
    fn facing_to_target_depth_passes() {
        let result = verify(&[(flat_tool(), facing_toolpath(2.0))]);
        assert!(result.gouge.pass, "no gouge expected: {:?}", result.gouge);
        assert!(
            result.excess.pass,
            "no excess expected: {:?}",
            result.excess
        );
        assert!(result.pass);
    }

    #[test]
    fn gouging_toolpath_fails_gouge_check() {
        // Correct facing, plus a rogue slot 3mm below the finished face.
        let mut rogue = facing_toolpath(2.0);
        rogue.push(ToolpathSegment::rapid(-10.0, 25.0, 5.0));
        rogue.push(ToolpathSegment::linear(-10.0, 25.0, -5.0, 300.0));
        rogue.push(ToolpathSegment::linear(60.0, 25.0, -5.0, 1000.0));

        let result = verify(&[(flat_tool(), rogue)]);
        assert!(!result.gouge.pass, "3mm slot into the part must be flagged");
        assert!(
            result.gouge.worst_depth > 1.5,
            "slot cuts ~3mm past the target surface, got {}",
            result.gouge.worst_depth
        );
        assert!(!result.gouge.examples.is_empty());
        assert!(
            result.excess.pass,
            "facing still completed: {:?}",
            result.excess
        );
        assert!(!result.pass);
    }

    #[test]
    fn shallow_facing_fails_excess_check() {
        // Faces only 1mm of the 2mm that must come off.
        let result = verify(&[(flat_tool(), facing_toolpath(1.0))]);
        assert!(result.gouge.pass, "no gouge expected: {:?}", result.gouge);
        assert!(!result.excess.pass, "1mm of leftover stock must be flagged");
        assert!(
            result.excess.worst_depth > 0.4,
            "leftover layer is ~1mm thick, got {}",
            result.excess.worst_depth
        );
        assert!(!result.pass);
    }

    #[test]
    fn rejects_degenerate_stock() {
        let result = verify_toolpaths(
            &target_block(),
            [0.0, 0.0, 0.0, 50.0, 50.0, 0.0],
            1.0,
            &[],
            &VerifyOptions::default(),
        );
        assert!(matches!(result, Err(StockSimError::InvalidBounds(_))));

        let result = verify_toolpaths(&target_block(), STOCK, 0.0, &[], &VerifyOptions::default());
        assert!(matches!(result, Err(StockSimError::ResolutionTooSmall(_))));
    }

    #[test]
    fn verification_is_plain_serializable_data() {
        let result = verify(&[(flat_tool(), facing_toolpath(2.0))]);
        let json = serde_json::to_string(&result).unwrap();
        let parsed: StockVerification = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.pass, result.pass);
        assert_eq!(parsed.grid_samples, result.grid_samples);
        assert_eq!(parsed.gouge.violation_count, result.gouge.violation_count);
    }
}
