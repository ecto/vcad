//! Drop-cutter algorithms for 3D roughing.
//!
//! Drop-cutter determines the minimum Z height at which a tool can be positioned
//! at a given (X, Y) without colliding with a mesh. This is the foundation for
//! 3D roughing toolpath generation.
//!
//! # Supported Tool Types
//!
//! - **Flat end mill**: Tests bottom face, cylindrical edge, and corner
//! - **Ball end mill**: Tests spherical tip
//! - **Bull end mill**: Tests flat bottom, toroidal corner, and cylindrical edge

mod ball;
mod bull;
mod flat;
mod mesh_accel;

pub use ball::drop_cutter_ball;
pub use bull::drop_cutter_bull;
pub use flat::drop_cutter_flat;
pub use mesh_accel::MeshAccel;

use serde::{Deserialize, Serialize};

/// A 2D grid of height values for 3D roughing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeightField {
    /// Number of samples in X direction.
    pub nx: usize,
    /// Number of samples in Y direction.
    pub ny: usize,
    /// Bounding box [min_x, min_y, max_x, max_y].
    pub bounds: [f64; 4],
    /// Height values (Z), row-major order (Y outer, X inner).
    ///
    /// A sample where the tool touches no surface is `f64::NEG_INFINITY`:
    /// nothing below it constrains the cut, so roughing goes to its target
    /// level there. JSON has no infinity, so these serialize as `null` and
    /// deserialize from `null` back to `NEG_INFINITY` (see [`heights_json`]),
    /// which lets a height field from `camDropCutter` feed straight into
    /// `camGenerateRoughing3d`.
    #[serde(with = "heights_json")]
    pub heights: Vec<f64>,
}

/// Serde adapter for [`HeightField::heights`]: non-finite samples as `null`.
///
/// Before this, `serde_json` wrote `NEG_INFINITY` as `null` on the way out
/// but refused `null` on the way in, so a height field could not round-trip
/// through JSON (vcad.io CNC card, 2026-10-27).
mod heights_json {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(heights: &[f64], s: S) -> Result<S::Ok, S::Error> {
        heights
            .iter()
            .map(|h| h.is_finite().then_some(*h))
            .collect::<Vec<Option<f64>>>()
            .serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<f64>, D::Error> {
        Ok(Vec::<Option<f64>>::deserialize(d)?
            .into_iter()
            .map(|h| h.unwrap_or(f64::NEG_INFINITY))
            .collect())
    }
}

impl HeightField {
    /// Create a new height field filled with a constant value.
    pub fn new(nx: usize, ny: usize, bounds: [f64; 4], initial: f64) -> Self {
        Self {
            nx,
            ny,
            bounds,
            heights: vec![initial; nx * ny],
        }
    }

    /// Get the X spacing between samples.
    pub fn dx(&self) -> f64 {
        if self.nx <= 1 {
            0.0
        } else {
            (self.bounds[2] - self.bounds[0]) / (self.nx - 1) as f64
        }
    }

    /// Get the Y spacing between samples.
    pub fn dy(&self) -> f64 {
        if self.ny <= 1 {
            0.0
        } else {
            (self.bounds[3] - self.bounds[1]) / (self.ny - 1) as f64
        }
    }

    /// Get the (X, Y) coordinates for a grid index.
    pub fn xy_at(&self, ix: usize, iy: usize) -> (f64, f64) {
        let x = self.bounds[0] + ix as f64 * self.dx();
        let y = self.bounds[1] + iy as f64 * self.dy();
        (x, y)
    }

    /// Get the height at a grid index.
    pub fn get(&self, ix: usize, iy: usize) -> f64 {
        self.heights[iy * self.nx + ix]
    }

    /// Set the height at a grid index.
    pub fn set(&mut self, ix: usize, iy: usize, z: f64) {
        self.heights[iy * self.nx + ix] = z;
    }

    /// Get the index for (ix, iy).
    pub fn index(&self, ix: usize, iy: usize) -> usize {
        iy * self.nx + ix
    }

    /// Interpolate height at an arbitrary (x, y) position using bilinear interpolation.
    pub fn interpolate(&self, x: f64, y: f64) -> Option<f64> {
        if x < self.bounds[0] || x > self.bounds[2] || y < self.bounds[1] || y > self.bounds[3] {
            return None;
        }

        let dx = self.dx();
        let dy = self.dy();

        // Guard against a degenerate (zero-extent or near-zero) grid to
        // avoid a divide-by-zero below. An exact `== 0.0` check would miss
        // grids with subnormal-but-nonzero steps, which still blow up fx/fy.
        if dx.abs() < f64::EPSILON || dy.abs() < f64::EPSILON {
            return Some(self.heights[0]);
        }

        let fx = (x - self.bounds[0]) / dx;
        let fy = (y - self.bounds[1]) / dy;

        let ix0 = (fx.floor() as usize).min(self.nx - 1);
        let iy0 = (fy.floor() as usize).min(self.ny - 1);
        let ix1 = (ix0 + 1).min(self.nx - 1);
        let iy1 = (iy0 + 1).min(self.ny - 1);

        let tx = fx - ix0 as f64;
        let ty = fy - iy0 as f64;

        let z00 = self.get(ix0, iy0);
        let z10 = self.get(ix1, iy0);
        let z01 = self.get(ix0, iy1);
        let z11 = self.get(ix1, iy1);

        let z0 = z00 * (1.0 - tx) + z10 * tx;
        let z1 = z01 * (1.0 - tx) + z11 * tx;

        Some(z0 * (1.0 - ty) + z1 * ty)
    }
}

use crate::Tool;

/// Generate a height field from a mesh using drop-cutter algorithm.
///
/// # Arguments
///
/// * `accel` - Spatial acceleration structure for the mesh
/// * `tool` - The cutting tool
/// * `bounds` - Bounding box [min_x, min_y, max_x, max_y]
/// * `resolution` - Sample spacing in mm
///
/// # Returns
///
/// A height field with minimum safe Z values for each sample point.
pub fn generate_height_field(
    accel: &MeshAccel,
    tool: &Tool,
    bounds: [f64; 4],
    resolution: f64,
) -> HeightField {
    let nx = ((bounds[2] - bounds[0]) / resolution).ceil() as usize + 1;
    let ny = ((bounds[3] - bounds[1]) / resolution).ceil() as usize + 1;

    let mut hf = HeightField::new(nx, ny, bounds, f64::NEG_INFINITY);

    for iy in 0..ny {
        for ix in 0..nx {
            let (x, y) = hf.xy_at(ix, iy);
            let z = drop_cutter(accel, tool, x, y);
            hf.set(ix, iy, z);
        }
    }

    hf
}

/// Compute drop-cutter height for a single point.
pub fn drop_cutter(accel: &MeshAccel, tool: &Tool, x: f64, y: f64) -> f64 {
    match tool {
        Tool::FlatEndMill { diameter, .. } => drop_cutter_flat(accel, *diameter / 2.0, x, y),
        Tool::BallEndMill { diameter, .. } => drop_cutter_ball(accel, *diameter / 2.0, x, y),
        Tool::BullEndMill {
            diameter,
            corner_radius,
            ..
        } => drop_cutter_bull(accel, *diameter / 2.0, *corner_radius, x, y),
        _ => f64::NEG_INFINITY, // Unsupported tool type
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An 80 × 50 × 8 slab as a triangle mesh (12 triangles).
    fn slab() -> (Vec<[f64; 3]>, Vec<u32>) {
        let (x, y, z) = (80.0, 50.0, 8.0);
        let v = vec![
            [0.0, 0.0, 0.0],
            [x, 0.0, 0.0],
            [x, y, 0.0],
            [0.0, y, 0.0],
            [0.0, 0.0, z],
            [x, 0.0, z],
            [x, y, z],
            [0.0, y, z],
        ];
        let i = vec![
            0, 2, 1, 0, 3, 2, // bottom
            4, 5, 6, 4, 6, 7, // top
            0, 1, 5, 0, 5, 4, // -y
            1, 2, 6, 1, 6, 5, // +x
            2, 3, 7, 2, 7, 6, // +y
            3, 0, 4, 3, 4, 7, // -x
        ];
        (v, i)
    }

    #[test]
    fn height_field_round_trips_through_json_with_empty_samples() {
        // Bounds wider than the slab: the margin samples hit nothing.
        let (v, i) = slab();
        let accel = MeshAccel::new(&v, &i, 5.0);
        let tool = Tool::BallEndMill {
            diameter: 6.0,
            flute_length: 20.0,
            flutes: 2,
        };
        let hf = generate_height_field(&accel, &tool, [-10.0, -10.0, 90.0, 60.0], 2.0);
        let empty = hf
            .heights
            .iter()
            .filter(|h| **h == f64::NEG_INFINITY)
            .count();
        assert!(
            empty > 0,
            "the margin should produce samples that hit nothing"
        );

        let json = serde_json::to_string(&hf).unwrap();
        assert_eq!(
            json.matches("null").count(),
            empty,
            "empty samples serialize as null"
        );

        let back: HeightField =
            serde_json::from_str(&json).expect("a height field must read its own JSON");
        assert_eq!((back.nx, back.ny, back.bounds), (hf.nx, hf.ny, hf.bounds));
        for (a, b) in hf.heights.iter().zip(&back.heights) {
            assert!(a == b, "{a} != {b}");
        }
    }

    #[test]
    fn finite_heights_serialize_as_plain_numbers() {
        let hf = HeightField::new(2, 1, [0.0, 0.0, 1.0, 0.0], 3.5);
        assert_eq!(
            serde_json::to_value(&hf).unwrap()["heights"],
            serde_json::json!([3.5, 3.5])
        );
    }

    #[test]
    fn test_height_field_new() {
        let hf = HeightField::new(10, 10, [0.0, 0.0, 100.0, 100.0], 0.0);
        assert_eq!(hf.nx, 10);
        assert_eq!(hf.ny, 10);
        assert_eq!(hf.heights.len(), 100);
    }

    #[test]
    fn test_height_field_spacing() {
        let hf = HeightField::new(11, 6, [0.0, 0.0, 100.0, 50.0], 0.0);
        assert!((hf.dx() - 10.0).abs() < 1e-6);
        assert!((hf.dy() - 10.0).abs() < 1e-6);
    }

    #[test]
    fn test_height_field_xy_at() {
        let hf = HeightField::new(11, 6, [0.0, 0.0, 100.0, 50.0], 0.0);
        let (x, y) = hf.xy_at(5, 2);
        assert!((x - 50.0).abs() < 1e-6);
        assert!((y - 20.0).abs() < 1e-6);
    }

    #[test]
    fn test_height_field_interpolate() {
        let mut hf = HeightField::new(3, 3, [0.0, 0.0, 2.0, 2.0], 0.0);
        // Set corners
        hf.set(0, 0, 0.0);
        hf.set(2, 0, 2.0);
        hf.set(0, 2, 2.0);
        hf.set(2, 2, 4.0);
        hf.set(1, 1, 2.0);

        // Center should interpolate to 2.0
        let z = hf.interpolate(1.0, 1.0).unwrap();
        assert!((z - 2.0).abs() < 1e-6);
    }
}
