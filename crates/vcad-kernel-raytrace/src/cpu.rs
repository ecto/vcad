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

    /// Get a reference to the underlying BRep solid, if this renderer was
    /// built over one. `None` when [`from_bvh`](Self::from_bvh) was handed a
    /// mesh-backed BVH.
    pub fn brep(&self) -> Option<&BRepSolid> {
        self.bvh.brep()
    }

    /// Render the scene with stratified multi-sampling for smoother silhouettes.
    ///
    /// # Arguments
    ///
    /// * `camera` - Camera position in world space
    /// * `target` - Point the camera is looking at
    /// * `up` - Up direction for the camera
    /// * `width` - Output image width in pixels
    /// * `height` - Output image height in pixels
    /// * `fov` - Field of view in degrees
    /// * `samples` - Number of samples per pixel (1 = single-sample, 4/9/16 = grid AA)
    ///
    /// # Returns
    ///
    /// A `Vec<u8>` containing RGBA pixel data (4 bytes per pixel),
    /// row-major order, top-to-bottom.
    #[allow(clippy::too_many_arguments)]
    pub fn render_samples(
        &self,
        camera: Point3,
        target: Point3,
        up: Dir3,
        width: u32,
        height: u32,
        fov: f64,
        samples: u32,
    ) -> Vec<u8> {
        let samples = samples.max(1);
        let grid = (samples as f64).sqrt() as u32;
        let grid = grid.max(1);

        if grid == 1 {
            return self.render(camera, target, up, width, height, fov);
        }

        let mut pixels = vec![0u8; (width * height * 4) as usize];

        let forward_vec = target - camera;
        let forward = Dir3::new_normalize(Vec3::new(forward_vec.x, forward_vec.y, forward_vec.z));
        let right = Dir3::new_normalize(forward.cross(up));
        let cam_up = Dir3::new_normalize(right.cross(forward));

        let fov_rad = fov.to_radians();
        let half_height = (fov_rad / 2.0).tan();
        let half_width = half_height * (width as f64 / height as f64);

        let bg_color: [u8; 4] = [30, 32, 40, 255];

        for py in 0..height {
            for px in 0..width {
                let mut r_acc = 0.0f64;
                let mut g_acc = 0.0f64;
                let mut b_acc = 0.0f64;
                let mut count = 0u32;

                for sy in 0..grid {
                    for sx in 0..grid {
                        // Stratified sub-pixel offset: center of each stratum.
                        let offset_x = (sx as f64 + 0.5) / grid as f64 - 0.5;
                        let offset_y = (sy as f64 + 0.5) / grid as f64 - 0.5;

                        let ndc_x = (px as f64 + 0.5 + offset_x) / width as f64;
                        let ndc_y = (py as f64 + 0.5 + offset_y) / height as f64;

                        let screen_x = (2.0 * ndc_x - 1.0) * half_width;
                        let screen_y = (1.0 - 2.0 * ndc_y) * half_height;

                        let ray_dir = Vec3::new(
                            forward.x + screen_x * right.x + screen_y * cam_up.x,
                            forward.y + screen_x * right.y + screen_y * cam_up.y,
                            forward.z + screen_x * right.z + screen_y * cam_up.z,
                        );

                        let ray = Ray::new(camera, ray_dir);
                        let color = if let Some(hit) = self.bvh.trace_closest(&ray) {
                            self.shade_hit(&ray, &hit)
                        } else {
                            bg_color
                        };

                        r_acc += color[0] as f64;
                        g_acc += color[1] as f64;
                        b_acc += color[2] as f64;
                        count += 1;
                    }
                }

                let n = count as f64;
                let idx = ((py * width + px) * 4) as usize;
                pixels[idx] = (r_acc / n) as u8;
                pixels[idx + 1] = (g_acc / n) as u8;
                pixels[idx + 2] = (b_acc / n) as u8;
                pixels[idx + 3] = 255;
            }
        }

        pixels
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

/// Render multiple solids with individual transforms and colors into an
/// opaque RGBA frame (fixed dark background). Scanlines render in parallel
/// (rayon); each solid's BVH is built once up front.
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
    render_scene_impl(
        solids,
        transforms,
        colors,
        camera,
        target,
        up,
        width,
        height,
        fov,
        [30, 32, 40, 255],
        Shading::Headlight,
    )
}

/// Like [`render_scene`], but misses stay fully transparent (RGBA 0) so the
/// frame can composite over a live viewport backdrop.
#[allow(clippy::too_many_arguments)]
pub fn render_scene_transparent(
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
    render_scene_impl(
        solids,
        transforms,
        colors,
        camera,
        target,
        up,
        width,
        height,
        fov,
        [0, 0, 0, 0],
        Shading::Studio,
    )
}

/// Shading model for the CPU tracer.
#[derive(Clone, Copy, PartialEq)]
enum Shading {
    /// Original camera-headlight lambert — byte-stable for CLI/termview.
    Headlight,
    /// Studio rig for the app's pixel-perfect mode: key/fill/rim lights,
    /// a hard key-light shadow ray, hemispherical ambient, Blinn specular,
    /// and gamma 2.2 output.
    Studio,
}

#[allow(clippy::too_many_arguments)]
fn render_scene_impl(
    solids: &[Arc<BRepSolid>],
    transforms: &[f64],
    colors: &[f32],
    camera: Point3,
    target: Point3,
    up: Dir3,
    width: u32,
    height: u32,
    fov: f64,
    background: [u8; 4],
    shading: Shading,
) -> Vec<u8> {
    use rayon::prelude::*;

    // Build every BVH once, up front (this used to happen per solid inside
    // the pixel loop's enclosing loop; the rays dominate, but rebuilds are
    // pure waste when parallelizing).
    let prepared: Vec<(Bvh, [f32; 3])> = solids
        .iter()
        .enumerate()
        .map(|(solid_idx, solid)| {
            let transformed = if transforms.len() >= (solid_idx + 1) * 16 {
                transform_brep(
                    solid.as_ref(),
                    &transforms[solid_idx * 16..(solid_idx + 1) * 16],
                )
            } else {
                solid.as_ref().clone()
            };
            let color = if colors.len() >= (solid_idx + 1) * 3 {
                [
                    colors[solid_idx * 3],
                    colors[solid_idx * 3 + 1],
                    colors[solid_idx * 3 + 2],
                ]
            } else {
                [0.6, 0.7, 0.8]
            };
            (Bvh::build(&transformed), color)
        })
        .collect();

    // Camera basis.
    let forward_vec = target - camera;
    let forward = Dir3::new_normalize(Vec3::new(forward_vec.x, forward_vec.y, forward_vec.z));
    let right = Dir3::new_normalize(forward.cross(up));
    let cam_up = Dir3::new_normalize(right.cross(forward));

    let fov_rad = fov.to_radians();
    let half_height = (fov_rad / 2.0).tan();
    let half_width = half_height * (width as f64 / height as f64);

    let mut pixels = vec![0u8; (width * height * 4) as usize];

    // Each scanline is independent: trace every solid, keep the nearest hit.
    pixels
        .par_chunks_mut(width as usize * 4)
        .enumerate()
        .for_each(|(py, row)| {
            for px in 0..width as usize {
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

                let mut best: Option<(f64, crate::RayHit, [f32; 3])> = None;
                for (bvh, color) in &prepared {
                    if let Some(hit) = bvh.trace_closest(&ray) {
                        if best.as_ref().is_none_or(|(t, _, _)| hit.t < *t) {
                            best = Some((hit.t, hit, *color));
                        }
                    }
                }
                let out = match best {
                    None => background,
                    Some((_, hit, color)) => match shading {
                        Shading::Headlight => {
                            let light_dir = Dir3::new_normalize(-ray.direction.into_inner());
                            let ndotl = hit.normal.dot(light_dir).abs();
                            let intensity = (0.2 + 0.8 * ndotl).min(1.0) as f32;
                            [
                                (color[0] * intensity * 255.0) as u8,
                                (color[1] * intensity * 255.0) as u8,
                                (color[2] * intensity * 255.0) as u8,
                                255,
                            ]
                        }
                        Shading::Studio => shade_studio(&prepared, &ray, &hit, color),
                    },
                };
                row[px * 4..px * 4 + 4].copy_from_slice(&out);
            }
        });

    pixels
}

/// Studio shading for the app's ray-traced still: a fixed Z-up light rig
/// (key with a hard shadow ray, cool fill, back rim), hemispherical
/// ambient so downward faces fall off naturally, Blinn specular for
/// machined sheen, and gamma-2.2 output.
fn shade_studio(
    prepared: &[(Bvh, [f32; 3])],
    ray: &Ray,
    hit: &crate::RayHit,
    color: [f32; 3],
) -> [u8; 4] {
    // Face-forward normal so interior faces (bore walls) shade correctly.
    let mut n = hit.normal.into_inner();
    if n.dot(ray.direction.into_inner()) > 0.0 {
        n = -n;
    }

    let key = Vec3::new(0.45, -0.35, 0.82).normalize();
    let fill = Vec3::new(-0.62, -0.25, 0.35).normalize();
    let rim = Vec3::new(0.15, 0.85, -0.25).normalize();

    // Hard shadow from the key light only: offset along the normal to dodge
    // self-intersection, any hit in any solid blocks it.
    let shadow_origin = hit.point + n * 1e-4;
    let shadow_ray = Ray::new(shadow_origin, key);
    let key_shadowed = prepared
        .iter()
        .any(|(bvh, _)| bvh.trace_closest(&shadow_ray).is_some());

    let key_l = if key_shadowed {
        0.0
    } else {
        n.dot(key).max(0.0)
    };
    let fill_l = n.dot(fill).max(0.0);
    let rim_l = n.dot(rim).max(0.0);
    // Hemispherical ambient: up-facing surfaces get a touch more sky.
    let ambient = 0.16 + 0.08 * (0.5 + 0.5 * n.z);

    let diffuse = (ambient + 0.68 * key_l + 0.22 * fill_l + 0.12 * rim_l).min(1.25);

    // Blinn specular on the key (skipped in shadow).
    let view = -ray.direction.into_inner();
    let half = (view + key).normalize();
    let spec = if key_shadowed {
        0.0
    } else {
        n.dot(half).max(0.0).powi(48) * 0.35
    };

    let shade = |c: f32| -> u8 {
        let linear = (c * diffuse as f32 + spec as f32).clamp(0.0, 1.0);
        (linear.powf(1.0 / 2.2) * 255.0) as u8
    };
    [shade(color[0]), shade(color[1]), shade(color[2]), 255]
}

/// Render multiple solids with individual transforms and colors, using stratified
/// multi-sampling for smoother silhouettes.
///
/// # Arguments
///
/// Same as `render_scene`, plus:
/// * `samples` - Samples per pixel (1 = single-sample, 4/9/16 = grid AA)
///
/// # Returns
///
/// RGBA pixel buffer.
#[allow(clippy::too_many_arguments)]
pub fn render_scene_samples(
    solids: &[Arc<BRepSolid>],
    transforms: &[f64],
    colors: &[f32],
    camera: Point3,
    target: Point3,
    up: Dir3,
    width: u32,
    height: u32,
    fov: f64,
    samples: u32,
) -> Vec<u8> {
    let samples = samples.max(1);
    let grid = (samples as f64).sqrt() as u32;
    let grid = grid.max(1);

    if grid == 1 {
        return render_scene(
            solids, transforms, colors, camera, target, up, width, height, fov,
        );
    }

    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let mut depth = vec![f64::INFINITY; (width * height) as usize];
    // Accumulate color sum per pixel (r, g, b, count) for averaging.
    let mut accum: Vec<[f64; 4]> = vec![[0.0; 4]; (width * height) as usize];

    // Set background.
    for i in 0..(width * height) as usize {
        pixels[i * 4] = 30;
        pixels[i * 4 + 1] = 32;
        pixels[i * 4 + 2] = 40;
        pixels[i * 4 + 3] = 255;
    }

    let forward_vec = target - camera;
    let forward = Dir3::new_normalize(Vec3::new(forward_vec.x, forward_vec.y, forward_vec.z));
    let right = Dir3::new_normalize(forward.cross(up));
    let cam_up = Dir3::new_normalize(right.cross(forward));

    let fov_rad = fov.to_radians();
    let half_height = (fov_rad / 2.0).tan();
    let half_width = half_height * (width as f64 / height as f64);

    for (solid_idx, solid) in solids.iter().enumerate() {
        let transform = if transforms.len() >= (solid_idx + 1) * 16 {
            Some(&transforms[solid_idx * 16..(solid_idx + 1) * 16])
        } else {
            None
        };

        let color = if colors.len() >= (solid_idx + 1) * 3 {
            [
                colors[solid_idx * 3],
                colors[solid_idx * 3 + 1],
                colors[solid_idx * 3 + 2],
            ]
        } else {
            [0.6, 0.7, 0.8]
        };

        let transformed_solid = if let Some(t) = transform {
            transform_brep(solid.as_ref(), t)
        } else {
            solid.as_ref().clone()
        };

        let bvh = Bvh::build(&transformed_solid);

        for py in 0..height {
            for px in 0..width {
                // Use only the pixel center for depth comparison; stratified samples
                // share the same depth slot (closest hit wins per solid, then averaged).
                let ndc_x_c = (px as f64 + 0.5) / width as f64;
                let ndc_y_c = (py as f64 + 0.5) / height as f64;
                let screen_x_c = (2.0 * ndc_x_c - 1.0) * half_width;
                let screen_y_c = (1.0 - 2.0 * ndc_y_c) * half_height;
                let ray_dir_c = Vec3::new(
                    forward.x + screen_x_c * right.x + screen_y_c * cam_up.x,
                    forward.y + screen_x_c * right.y + screen_y_c * cam_up.y,
                    forward.z + screen_x_c * right.z + screen_y_c * cam_up.z,
                );
                let center_ray = Ray::new(camera, ray_dir_c);
                let center_hit = bvh.trace_closest(&center_ray);

                let pixel_idx = (py * width + px) as usize;
                if let Some(ref hit) = center_hit {
                    if hit.t >= depth[pixel_idx] {
                        continue;
                    }
                    depth[pixel_idx] = hit.t;

                    // Fire stratified samples for this solid's contribution.
                    let mut r_acc = 0.0f64;
                    let mut g_acc = 0.0f64;
                    let mut b_acc = 0.0f64;

                    for sy in 0..grid {
                        for sx in 0..grid {
                            let offset_x = (sx as f64 + 0.5) / grid as f64 - 0.5;
                            let offset_y = (sy as f64 + 0.5) / grid as f64 - 0.5;
                            let ndc_x = (px as f64 + 0.5 + offset_x) / width as f64;
                            let ndc_y = (py as f64 + 0.5 + offset_y) / height as f64;
                            let screen_x = (2.0 * ndc_x - 1.0) * half_width;
                            let screen_y = (1.0 - 2.0 * ndc_y) * half_height;
                            let ray_dir = Vec3::new(
                                forward.x + screen_x * right.x + screen_y * cam_up.x,
                                forward.y + screen_x * right.y + screen_y * cam_up.y,
                                forward.z + screen_x * right.z + screen_y * cam_up.z,
                            );
                            let ray = Ray::new(camera, ray_dir);
                            let light_dir = Dir3::new_normalize(-ray.direction.into_inner());
                            let (r, g, b) = if let Some(h) = bvh.trace_closest(&ray) {
                                let ndotl = h.normal.dot(light_dir).abs();
                                let intensity = (0.2 + 0.8 * ndotl).min(1.0) as f32;
                                (
                                    (color[0] * intensity * 255.0) as f64,
                                    (color[1] * intensity * 255.0) as f64,
                                    (color[2] * intensity * 255.0) as f64,
                                )
                            } else {
                                // Missed on sub-sample — use center-hit shading
                                let ndotl = hit.normal.dot(light_dir).abs();
                                let intensity = (0.2 + 0.8 * ndotl).min(1.0) as f32;
                                (
                                    (color[0] * intensity * 255.0) as f64,
                                    (color[1] * intensity * 255.0) as f64,
                                    (color[2] * intensity * 255.0) as f64,
                                )
                            };
                            r_acc += r;
                            g_acc += g;
                            b_acc += b;
                        }
                    }

                    let n = (grid * grid) as f64;
                    accum[pixel_idx] = [r_acc / n, g_acc / n, b_acc / n, 1.0];
                }
            }
        }
    }

    // Write averaged colors.
    for (i, a) in accum.iter().enumerate() {
        if a[3] > 0.0 {
            let idx = i * 4;
            pixels[idx] = a[0] as u8;
            pixels[idx + 1] = a[1] as u8;
            pixels[idx + 2] = a[2] as u8;
            pixels[idx + 3] = 255;
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
            matrix[0], matrix[4], matrix[8], matrix[12], matrix[1], matrix[5], matrix[9],
            matrix[13], matrix[2], matrix[6], matrix[10], matrix[14], matrix[3], matrix[7],
            matrix[11], matrix[15],
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
        assert!(!renderer
            .brep()
            .expect("built from a BRep")
            .topology
            .faces
            .is_empty());
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

        assert!(bg_count < 32 * 32, "Should have some non-background pixels");
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
        let all_bg = pixels
            .chunks(4)
            .all(|p| p[0] == 30 && p[1] == 32 && p[2] == 40);
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
        let all_bg = pixels
            .chunks(4)
            .all(|p| p[0] == 30 && p[1] == 32 && p[2] == 40);
        assert!(all_bg, "Empty scene should be all background");
    }

    #[test]
    fn test_render_scene_single_solid() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let solids = vec![Arc::new(cube)];

        let pixels = render_scene(
            &solids,
            &[],              // No transform
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

#[cfg(test)]
mod closed_surface_trace {
    use super::*;
    use vcad_kernel_primitives::make_sphere;

    /// Regression: a full primitive sphere is one face whose outer loop is
    /// only the seam — a zero-area polygon in UV. The trim test used to
    /// reject every hit on it, so spheres never ray traced at all. A
    /// degenerate outer loop must read as "untrimmed".
    #[test]
    fn full_sphere_traces() {
        let s = make_sphere(10.0, 0);
        let bvh = Bvh::build(&s);
        let ray = Ray::new(Point3::new(0.0, -50.0, 0.0), Vec3::new(0.0, 1.0, 0.0));
        let hits = bvh.trace(&ray);
        assert_eq!(hits.len(), 2, "ray through center must enter and exit");
        let t = bvh.trace_closest(&ray).expect("must hit").t;
        assert!((t - 40.0).abs() < 1e-6, "front of sphere at t=40, got {t}");
    }
}
