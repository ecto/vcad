//! Support structure generation.

use vcad_kernel_math::Point2;
use vcad_kernel_tessellate::TriangleMesh;

use crate::path::Polygon;
use crate::slice::SliceLayer;

/// Settings for support generation.
#[derive(Debug, Clone, Copy)]
pub struct SupportSettings {
    /// Overhang angle threshold (degrees). Faces steeper than this need support.
    pub overhang_angle: f64,
    /// Support density (0.0 to 1.0).
    pub density: f64,
    /// Z distance between support and model (mm).
    pub z_distance: f64,
    /// XY distance between support and model (mm).
    pub xy_distance: f64,
    /// Support pattern spacing (mm).
    pub pattern_spacing: f64,
}

impl Default for SupportSettings {
    fn default() -> Self {
        Self {
            overhang_angle: 45.0,
            density: 0.15,
            z_distance: 0.2,
            xy_distance: 0.4,
            pattern_spacing: 2.5,
        }
    }
}

/// Support region for a layer.
#[derive(Debug, Clone)]
pub struct LayerSupport {
    /// Support regions (polygons to fill).
    pub regions: Vec<Polygon>,
    /// Interface layer (denser, closer to model).
    pub is_interface: bool,
}

impl LayerSupport {
    /// Create empty support.
    pub fn new() -> Self {
        Self {
            regions: Vec::new(),
            is_interface: false,
        }
    }
}

impl Default for LayerSupport {
    fn default() -> Self {
        Self::new()
    }
}

/// Detect overhang regions from mesh normals.
///
/// Returns regions per layer that need support.
pub fn detect_overhangs(
    mesh: &TriangleMesh,
    layers: &[SliceLayer],
    settings: &SupportSettings,
) -> Vec<LayerSupport> {
    let threshold_cos = (settings.overhang_angle.to_radians()).cos();
    let num_triangles = mesh.indices.len() / 3;

    // Find triangles facing downward beyond threshold
    let mut overhang_triangles: Vec<(usize, f64, f64)> = Vec::new(); // (triangle_idx, z_min, z_max)

    for i in 0..num_triangles {
        let i0 = mesh.indices[i * 3] as usize;
        let i1 = mesh.indices[i * 3 + 1] as usize;
        let i2 = mesh.indices[i * 3 + 2] as usize;

        // Get normal (if available)
        let nz = if !mesh.normals.is_empty() {
            mesh.normals[i0 * 3 + 2]
        } else {
            // Compute from vertices
            let v0 = [
                mesh.vertices[i0 * 3] as f64,
                mesh.vertices[i0 * 3 + 1] as f64,
                mesh.vertices[i0 * 3 + 2] as f64,
            ];
            let v1 = [
                mesh.vertices[i1 * 3] as f64,
                mesh.vertices[i1 * 3 + 1] as f64,
                mesh.vertices[i1 * 3 + 2] as f64,
            ];
            let v2 = [
                mesh.vertices[i2 * 3] as f64,
                mesh.vertices[i2 * 3 + 1] as f64,
                mesh.vertices[i2 * 3 + 2] as f64,
            ];

            let e1 = [v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]];
            let e2 = [v2[0] - v0[0], v2[1] - v0[1], v2[2] - v0[2]];

            let nx = e1[1] * e2[2] - e1[2] * e2[1];
            let ny = e1[2] * e2[0] - e1[0] * e2[2];
            let nz = e1[0] * e2[1] - e1[1] * e2[0];

            let len = (nx * nx + ny * ny + nz * nz).sqrt();
            if len > 1e-10 {
                (nz / len) as f32
            } else {
                0.0
            }
        };

        // Triangle is overhang if normal points significantly downward
        if (nz as f64) < -threshold_cos {
            let z0 = mesh.vertices[i0 * 3 + 2] as f64;
            let z1 = mesh.vertices[i1 * 3 + 2] as f64;
            let z2 = mesh.vertices[i2 * 3 + 2] as f64;

            let z_min = z0.min(z1).min(z2);
            let z_max = z0.max(z1).max(z2);

            overhang_triangles.push((i, z_min, z_max));
        }
    }

    // Generate support regions for each layer
    layers
        .iter()
        .map(|layer| {
            let mut support = LayerSupport::new();

            // Find overhang triangles at this Z
            for &(tri_idx, z_min, z_max) in &overhang_triangles {
                if layer.z >= z_min - settings.z_distance && layer.z <= z_max {
                    // Project triangle to XY and add to support region
                    if let Some(poly) = project_triangle_to_xy(mesh, tri_idx, settings.xy_distance)
                    {
                        support.regions.push(poly);
                    }
                }
            }

            // Mark as interface layer if close to model
            support.is_interface = layer.index > 0
                && layers.get(layer.index + 1).is_some_and(|next| {
                    // Check if next layer has model geometry
                    !next.contours.is_empty()
                });

            support
        })
        .collect()
}

/// Project a triangle to XY plane with offset.
fn project_triangle_to_xy(mesh: &TriangleMesh, tri_idx: usize, offset: f64) -> Option<Polygon> {
    let i0 = mesh.indices[tri_idx * 3] as usize;
    let i1 = mesh.indices[tri_idx * 3 + 1] as usize;
    let i2 = mesh.indices[tri_idx * 3 + 2] as usize;

    let v0 = Point2::new(
        mesh.vertices[i0 * 3] as f64,
        mesh.vertices[i0 * 3 + 1] as f64,
    );
    let v1 = Point2::new(
        mesh.vertices[i1 * 3] as f64,
        mesh.vertices[i1 * 3 + 1] as f64,
    );
    let v2 = Point2::new(
        mesh.vertices[i2 * 3] as f64,
        mesh.vertices[i2 * 3 + 1] as f64,
    );

    // Create triangle polygon
    let poly = Polygon::new(vec![v0, v1, v2]);

    // Offset outward (negative offset for expansion)
    poly.offset(-offset)
}

/// Generate support towers from overhang regions.
/// Merges regions and extends down to build plate or model.
pub fn generate_support_towers(
    layers: &mut [LayerSupport],
    model_layers: &[SliceLayer],
    _settings: &SupportSettings,
) {
    // Extend support regions downward
    for i in (0..layers.len()).rev() {
        if layers[i].regions.is_empty() {
            continue;
        }

        // Clone regions to avoid borrow issues
        let regions_to_copy: Vec<Polygon> = layers[i].regions.clone();

        // Find layers below that need support extended
        for j in (0..i).rev() {
            // Check if support would intersect model
            let model_at_j = &model_layers[j];
            if !model_at_j.contours.is_empty() {
                // Stop support at this height (plus z_distance)
                break;
            }

            // Extend support region down
            for region in &regions_to_copy {
                layers[j].regions.push(region.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_support_settings_default() {
        let settings = SupportSettings::default();
        assert!((settings.overhang_angle - 45.0).abs() < 0.1);
        assert!(settings.density > 0.0);
    }
}
