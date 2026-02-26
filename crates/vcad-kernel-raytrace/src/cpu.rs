//! CPU-based ray tracing renderer for terminal and headless use.
//!
//! Provides a simple CPU renderer that uses the existing BVH infrastructure
//! to produce pixel-perfect images without GPU dependencies.

use std::sync::Arc;
use vcad_kernel_math::{Dir3, Point3, Transform, Vec3};
use vcad_kernel_primitives::BRepSolid;

use crate::bvh::Bvh;
use crate::Ray;

/// A CPU-based ray tracer for rendering BRep solids.
///
/// This renderer traces rays through a BVH acceleration structure and
/// produces an RGBA pixel buffer suitable for terminal or image output.
#[derive(Debug, Clone)]
pub struct CpuRenderer {
    bvh: Bvh,
    material_color: [f32; 3],
}

impl CpuRenderer {
    /// Create a new CPU renderer from a BRep solid.
    ///
    /// Builds a BVH for efficient ray tracing.
    pub fn new(solid: &BRepSolid) -> Self {
        Self {
            bvh: Bvh::build(solid),
            material_color: [0.6, 0.7, 0.8], // Default light blue-gray
        }
    }

    /// Create a CPU renderer from a pre-built BVH.
    pub fn from_bvh(bvh: Bvh) -> Self {
        Self {
            bvh,
            material_color: [0.6, 0.7, 0.8],
        }
    }

    /// Set the material color (RGB, each component 0.0 to 1.0).
    pub fn set_material(&mut self, r: f32, g: f32, b: f32) {
        self.material_color = [r, g, b];
    }

    /// Get a reference to the underlying BRep solid.
    pub fn brep(&self) -> &BRepSolid {
        self.bvh.brep()
    }

    /// Render the scene to an RGBA pixel buffer.
    ///
    /// # Arguments
    ///
    /// * `camera` - Camera position in world space
    /// * `target` - Point the camera is looking at
    /// * `up` - Up direction for the camera
    /// * `width` - Output image width in pixels
    /// * `height` - Output image height in pixels
    /// * `fov` - Field of view in degrees
    ///
    /// # Returns
    ///
    /// A `Vec<u8>` containing RGBA pixel data (4 bytes per pixel),
    /// row-major order, top-to-bottom.
    pub fn render(
        &self,
        camera: Point3,
        target: Point3,
        up: Dir3,
        width: u32,
        height: u32,
        fov: f64,
    ) -> Vec<u8> {
        let mut pixels = vec![0u8; (width * height * 4) as usize];

        // Compute camera basis vectors
        let forward_vec = target - camera;
        let forward = Dir3::new_normalize(Vec3::new(forward_vec.x, forward_vec.y, forward_vec.z));
        let right = Dir3::new_normalize(forward.cross(up));
        let cam_up = Dir3::new_normalize(right.cross(forward));

        // Compute image plane dimensions
        let fov_rad = fov.to_radians();
        let half_height = (fov_rad / 2.0).tan();
        let half_width = half_height * (width as f64 / height as f64);

        // Background color (dark gray)
        let bg_color: [u8; 4] = [30, 32, 40, 255];

        for py in 0..height {
            for px in 0..width {
                // Convert pixel coordinates to normalized device coordinates
                // NDC: (0,0) at top-left, (1,1) at bottom-right
                let ndc_x = (px as f64 + 0.5) / width as f64;
                let ndc_y = (py as f64 + 0.5) / height as f64;

                // Convert to camera-relative coordinates
                // Screen space: (-half_width, half_height) at top-left
                let screen_x = (2.0 * ndc_x - 1.0) * half_width;
                let screen_y = (1.0 - 2.0 * ndc_y) * half_height;

                // Compute ray direction
                let ray_dir = Vec3::new(
                    forward.x + screen_x * right.x + screen_y * cam_up.x,
                    forward.y + screen_x * right.y + screen_y * cam_up.y,
                    forward.z + screen_x * right.z + screen_y * cam_up.z,
                );

                let ray = Ray::new(camera, ray_dir);

                // Trace the ray
                let color = if let Some(hit) = self.bvh.trace_closest(&ray) {
                    self.shade_hit(&ray, &hit)
                } else {
                    bg_color
                };

                // Write pixel
                let idx = ((py * width + px) * 4) as usize;
                pixels[idx] = color[0];
                pixels[idx + 1] = color[1];
                pixels[idx + 2] = color[2];
                pixels[idx + 3] = color[3];
            }
        }

        pixels
    }

    /// Compute the shaded color for a ray hit.
    fn shade_hit(&self, ray: &Ray, hit: &crate::RayHit) -> [u8; 4] {
        // Simple Lambertian shading with light from camera direction
        let light_dir = Dir3::new_normalize(-ray.direction.into_inner());

        // Compute dot product for diffuse lighting
        // Handle both sides of the surface
        let ndotl = hit.normal.dot(light_dir);
        let ndotl = ndotl.abs(); // Two-sided lighting

        // Ambient + diffuse lighting
        let ambient = 0.2;
        let diffuse = 0.8 * ndotl;
        let intensity = (ambient + diffuse).min(1.0);

        // Apply material color and intensity
        let intensity = intensity as f32;
        let r = (self.material_color[0] * intensity * 255.0) as u8;
        let g = (self.material_color[1] * intensity * 255.0) as u8;
        let b = (self.material_color[2] * intensity * 255.0) as u8;

        [r, g, b, 255]
    }
}

/// Render multiple solids with individual transforms and colors.
///
/// This is a convenience function for rendering scenes with multiple parts.
///
/// # Arguments
///
/// * `solids` - Slice of BRep solids to render
/// * `transforms` - Flattened 4x4 row-major transform matrices (16 f64 values per solid)
/// * `colors` - RGB colors (3 f32 values per solid, each 0.0-1.0)
/// * `camera` - Camera position
/// * `target` - Camera target
/// * `up` - Up direction
/// * `width` - Image width
/// * `height` - Image height
/// * `fov` - Field of view in degrees
///
/// # Returns
///
/// RGBA pixel buffer.
#[allow(clippy::too_many_arguments)]
pub fn render_scene(
    solids: &[Arc<BRepSolid>],
    transforms: &[f64],
    colors: &[f32],
    camera: Point3,
    target: Point3,
    up: Dir3,
    width: u32,
    height: u32,
    fov: f64,
) -> Vec<u8> {
    // For now, render each solid separately and composite using depth buffer
    // This is simpler than building a combined BVH

    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let mut depth = vec![f64::INFINITY; (width * height) as usize];

    // Background color
    for i in 0..(width * height) as usize {
        pixels[i * 4] = 30;
        pixels[i * 4 + 1] = 32;
        pixels[i * 4 + 2] = 40;
        pixels[i * 4 + 3] = 255;
    }

    // Compute camera basis vectors
    let forward_vec = target - camera;
    let forward = Dir3::new_normalize(Vec3::new(forward_vec.x, forward_vec.y, forward_vec.z));
    let right = Dir3::new_normalize(forward.cross(up));
    let cam_up = Dir3::new_normalize(right.cross(forward));

    let fov_rad = fov.to_radians();
    let half_height = (fov_rad / 2.0).tan();
    let half_width = half_height * (width as f64 / height as f64);

    for (solid_idx, solid) in solids.iter().enumerate() {
        // Get transform for this solid (identity if not provided)
        let transform = if transforms.len() >= (solid_idx + 1) * 16 {
            Some(&transforms[solid_idx * 16..(solid_idx + 1) * 16])
        } else {
            None
        };

        // Get color for this solid
        let color = if colors.len() >= (solid_idx + 1) * 3 {
            [
                colors[solid_idx * 3],
                colors[solid_idx * 3 + 1],
                colors[solid_idx * 3 + 2],
            ]
        } else {
            [0.6, 0.7, 0.8]
        };

        // Apply transform to solid if provided
        let transformed_solid = if let Some(t) = transform {
            transform_brep(solid.as_ref(), t)
        } else {
            solid.as_ref().clone()
        };

        let bvh = Bvh::build(&transformed_solid);

        // Render each pixel
        for py in 0..height {
            for px in 0..width {
                let ndc_x = (px as f64 + 0.5) / width as f64;
                let ndc_y = (py as f64 + 0.5) / height as f64;

                let screen_x = (2.0 * ndc_x - 1.0) * half_width;
                let screen_y = (1.0 - 2.0 * ndc_y) * half_height;

                let ray_dir = Vec3::new(
                    forward.x + screen_x * right.x + screen_y * cam_up.x,
                    forward.y + screen_x * right.y + screen_y * cam_up.y,
                    forward.z + screen_x * right.z + screen_y * cam_up.z,
                );

                let ray = Ray::new(camera, ray_dir);

                if let Some(hit) = bvh.trace_closest(&ray) {
                    let pixel_idx = (py * width + px) as usize;

                    // Depth test
                    if hit.t < depth[pixel_idx] {
                        depth[pixel_idx] = hit.t;

                        // Shade
                        let light_dir = Dir3::new_normalize(-ray.direction.into_inner());
                        let ndotl = hit.normal.dot(light_dir).abs();
                        let intensity = (0.2 + 0.8 * ndotl).min(1.0) as f32;

                        let idx = pixel_idx * 4;
                        pixels[idx] = (color[0] * intensity * 255.0) as u8;
                        pixels[idx + 1] = (color[1] * intensity * 255.0) as u8;
                        pixels[idx + 2] = (color[2] * intensity * 255.0) as u8;
                        pixels[idx + 3] = 255;
                    }
                }
            }
        }
    }

    pixels
}

/// Apply a 4x4 transform matrix to a BRep solid.
fn transform_brep(solid: &BRepSolid, matrix: &[f64]) -> BRepSolid {
    // Build transform from row-major 4x4 matrix
    // tang::Mat4::new takes row-major args, so we transpose the input
    // (input is row-major but we want the transpose)
    let t = Transform {
        matrix: tang::Mat4::new(
            matrix[0], matrix[4], matrix[8], matrix[12],
            matrix[1], matrix[5], matrix[9], matrix[13],
            matrix[2], matrix[6], matrix[10], matrix[14],
            matrix[3], matrix[7], matrix[11], matrix[15],
        ),
    };

    let mut new_solid = solid.clone();

    // Transform all vertices
    for (_id, vertex) in &mut new_solid.topology.vertices {
        vertex.point = t.apply_point(&vertex.point);
    }

    // Transform all surfaces
    for surface in &mut new_solid.geometry.surfaces {
        *surface = surface.transform(&t);
    }

    // Handle mirroring (negative determinant)
    let det = matrix[0] * (matrix[5] * matrix[10] - matrix[6] * matrix[9])
        - matrix[1] * (matrix[4] * matrix[10] - matrix[6] * matrix[8])
        + matrix[2] * (matrix[4] * matrix[9] - matrix[5] * matrix[8]);

    if det < 0.0 {
        use vcad_kernel_topo::Orientation;
        for (_id, face) in &mut new_solid.topology.faces {
            face.orientation = match face.orientation {
                Orientation::Forward => Orientation::Reversed,
                Orientation::Reversed => Orientation::Forward,
            };
        }
    }

    new_solid
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_primitives::make_cube;

    #[test]
    fn test_cpu_renderer_create() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let renderer = CpuRenderer::new(&cube);
        assert!(!renderer.brep().topology.faces.is_empty());
    }

    #[test]
    fn test_cpu_renderer_render_cube() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let renderer = CpuRenderer::new(&cube);

        let pixels = renderer.render(
            Point3::new(20.0, 20.0, 20.0),
            Point3::new(5.0, 5.0, 5.0),
            Dir3::new_normalize(Vec3::new(0.0, 1.0, 0.0)),
            32,
            32,
            45.0,
        );

        // Should have rendered something (not all background)
        assert_eq!(pixels.len(), 32 * 32 * 4);

        // Check that we have some non-background pixels
        let bg_count = pixels
            .chunks(4)
            .filter(|p| p[0] == 30 && p[1] == 32 && p[2] == 40)
            .count();

        assert!(
            bg_count < 32 * 32,
            "Should have some non-background pixels"
        );
    }

    #[test]
    fn test_cpu_renderer_render_miss() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let renderer = CpuRenderer::new(&cube);

        // Camera looking away from the cube
        let pixels = renderer.render(
            Point3::new(100.0, 100.0, 100.0),
            Point3::new(200.0, 200.0, 200.0),
            Dir3::new_normalize(Vec3::new(0.0, 1.0, 0.0)),
            16,
            16,
            45.0,
        );

        // Should be all background
        let all_bg = pixels.chunks(4).all(|p| p[0] == 30 && p[1] == 32 && p[2] == 40);
        assert!(all_bg, "Camera looking away should see only background");
    }

    #[test]
    fn test_cpu_renderer_set_material() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let mut renderer = CpuRenderer::new(&cube);
        renderer.set_material(1.0, 0.0, 0.0); // Red

        let pixels = renderer.render(
            Point3::new(20.0, 20.0, 20.0),
            Point3::new(5.0, 5.0, 5.0),
            Dir3::new_normalize(Vec3::new(0.0, 1.0, 0.0)),
            16,
            16,
            45.0,
        );

        // Find a non-background pixel and check it's reddish
        for chunk in pixels.chunks(4) {
            if chunk[0] != 30 || chunk[1] != 32 || chunk[2] != 40 {
                // Non-background pixel should have red > green and red > blue
                assert!(
                    chunk[0] > chunk[1] && chunk[0] > chunk[2],
                    "Expected red material: {:?}",
                    chunk
                );
                return;
            }
        }
        panic!("No non-background pixels found");
    }

    #[test]
    fn test_render_scene_empty() {
        let pixels = render_scene(
            &[],
            &[],
            &[],
            Point3::new(20.0, 20.0, 20.0),
            Point3::new(0.0, 0.0, 0.0),
            Dir3::new_normalize(Vec3::new(0.0, 1.0, 0.0)),
            16,
            16,
            45.0,
        );

        // Should be all background
        let all_bg = pixels.chunks(4).all(|p| p[0] == 30 && p[1] == 32 && p[2] == 40);
        assert!(all_bg, "Empty scene should be all background");
    }

    #[test]
    fn test_render_scene_single_solid() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let solids = vec![Arc::new(cube)];

        let pixels = render_scene(
            &solids,
            &[], // No transform
            &[0.8, 0.2, 0.2], // Red
            Point3::new(20.0, 20.0, 20.0),
            Point3::new(5.0, 5.0, 5.0),
            Dir3::new_normalize(Vec3::new(0.0, 1.0, 0.0)),
            32,
            32,
            45.0,
        );

        // Should have some non-background pixels
        let bg_count = pixels
            .chunks(4)
            .filter(|p| p[0] == 30 && p[1] == 32 && p[2] == 40)
            .count();

        assert!(bg_count < 32 * 32, "Should render the cube");
    }
}
