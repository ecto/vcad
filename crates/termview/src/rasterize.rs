//! Software 3D rasterizer.
//!
//! Renders triangle meshes to a pixel buffer with depth testing and flat shading.

use std::f32::consts::PI;

use crate::buffer::RenderBuffer;

/// 3D vector.
#[derive(Debug, Clone, Copy)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    pub fn cross(self, other: Self) -> Self {
        Self {
            x: self.y * other.z - self.z * other.y,
            y: self.z * other.x - self.x * other.z,
            z: self.x * other.y - self.y * other.x,
        }
    }

    pub fn normalize(self) -> Self {
        let len = (self.x * self.x + self.y * self.y + self.z * self.z).sqrt();
        if len < 1e-10 {
            Self::new(0.0, 0.0, 1.0)
        } else {
            Self::new(self.x / len, self.y / len, self.z / len)
        }
    }

    pub fn scale(self, s: f32) -> Self {
        Self::new(self.x * s, self.y * s, self.z * s)
    }

    #[allow(clippy::should_implement_trait)]
    pub fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }
}

/// A triangle with vertex positions and color.
#[derive(Debug, Clone)]
pub struct Triangle {
    pub v0: [f32; 3],
    pub v1: [f32; 3],
    pub v2: [f32; 3],
    pub color: [u8; 3],
    /// Object pick ID (0 = background, >0 = object).
    pub pick_id: u32,
}

/// Camera for 3D viewing.
#[derive(Debug, Clone)]
pub struct Camera {
    /// Camera position.
    pub position: Vec3,
    /// Look-at target.
    pub target: Vec3,
    /// Up vector.
    pub up: Vec3,
    /// Field of view in degrees.
    pub fov: f32,
    /// Distance from target (for orbit controls).
    pub distance: f32,
    /// Horizontal angle in degrees.
    pub azimuth: f32,
    /// Vertical angle in degrees.
    pub elevation: f32,
}

impl Default for Camera {
    fn default() -> Self {
        let distance = 100.0;
        let azimuth = 45.0f32;
        let elevation = 30.0f32;

        let az_rad = azimuth.to_radians();
        let el_rad = elevation.to_radians();

        let position = Vec3::new(
            distance * el_rad.cos() * az_rad.sin(),
            distance * el_rad.sin(),
            distance * el_rad.cos() * az_rad.cos(),
        );

        Self {
            position,
            target: Vec3::new(0.0, 0.0, 0.0),
            up: Vec3::new(0.0, 1.0, 0.0),
            fov: 60.0,
            distance,
            azimuth,
            elevation,
        }
    }
}

impl Camera {
    /// Rotate the camera horizontally (orbit around target).
    pub fn rotate_horizontal(&mut self, degrees: f32) {
        self.azimuth += degrees;
        self.update_position();
    }

    /// Rotate the camera vertically (orbit around target).
    pub fn rotate_vertical(&mut self, degrees: f32) {
        self.elevation = (self.elevation + degrees).clamp(-89.0, 89.0);
        self.update_position();
    }

    /// Zoom in/out.
    pub fn zoom(&mut self, factor: f32) {
        self.distance = (self.distance * factor).clamp(10.0, 1000.0);
        self.update_position();
    }

    /// Set camera orbit parameters directly.
    pub fn set_orbit(&mut self, azimuth: f32, elevation: f32, distance: f32, target: Vec3) {
        self.azimuth = azimuth;
        self.elevation = elevation.clamp(-89.0, 89.0);
        self.distance = distance.max(1.0);
        self.target = target;
        self.update_position();
    }

    /// Adjust distance so a 20-unit object fills ~40% of viewport height.
    #[allow(dead_code)]
    ///
    /// Perspective projection is resolution-independent: the apparent angular size
    /// depends only on FOV and distance, not pixel count.
    pub fn zoom_to_fit(&mut self, _viewport_width: u32, _viewport_height: u32) {
        // object_size / (2 * distance * tan(fov/2)) = fraction_of_screen
        // distance = object_size / (2 * fraction * tan(fov/2))
        let object_size = 20.0f32;
        let fraction = 0.4;
        let half_fov_rad = (self.fov / 2.0).to_radians();
        let distance = object_size / (2.0 * fraction * half_fov_rad.tan());
        self.distance = distance.clamp(10.0, 1000.0);
        self.update_position();
    }

    /// Create a bitwise snapshot for cheap change detection.
    pub fn snapshot(&self) -> CameraSnapshot {
        CameraSnapshot {
            azimuth: self.azimuth.to_bits(),
            elevation: self.elevation.to_bits(),
            distance: self.distance.to_bits(),
            target: [
                self.target.x.to_bits(),
                self.target.y.to_bits(),
                self.target.z.to_bits(),
            ],
        }
    }

    pub fn update_position(&mut self) {
        let az_rad = self.azimuth.to_radians();
        let el_rad = self.elevation.to_radians();

        self.position = Vec3::new(
            self.target.x + self.distance * el_rad.cos() * az_rad.sin(),
            self.target.y + self.distance * el_rad.sin(),
            self.target.z + self.distance * el_rad.cos() * az_rad.cos(),
        );
    }
}

/// Bitwise camera state for cheap equality comparison.
#[derive(Clone, PartialEq, Eq)]
pub struct CameraSnapshot {
    azimuth: u32,
    elevation: u32,
    distance: u32,
    target: [u32; 3],
}

/// 4x4 matrix for transformations.
struct Mat4 {
    data: [f32; 16],
}

impl Mat4 {
    fn look_at(eye: Vec3, target: Vec3, up: Vec3) -> Self {
        let z = eye.sub(target).normalize();
        let x = up.cross(z).normalize();
        let y = z.cross(x);

        Self {
            data: [
                x.x,
                y.x,
                z.x,
                0.0,
                x.y,
                y.y,
                z.y,
                0.0,
                x.z,
                y.z,
                z.z,
                0.0,
                -x.dot(eye),
                -y.dot(eye),
                -z.dot(eye),
                1.0,
            ],
        }
    }

    fn perspective(fov: f32, aspect: f32, near: f32, far: f32) -> Self {
        let f = 1.0 / (fov / 2.0).tan();
        let nf = 1.0 / (near - far);

        Self {
            data: [
                f / aspect,
                0.0,
                0.0,
                0.0,
                0.0,
                f,
                0.0,
                0.0,
                0.0,
                0.0,
                (far + near) * nf,
                -1.0,
                0.0,
                0.0,
                2.0 * far * near * nf,
                0.0,
            ],
        }
    }

    fn multiply(&self, other: &Mat4) -> Mat4 {
        let mut result = [0.0f32; 16];
        for i in 0..4 {
            for j in 0..4 {
                for k in 0..4 {
                    result[i * 4 + j] += self.data[i * 4 + k] * other.data[k * 4 + j];
                }
            }
        }
        Mat4 { data: result }
    }

    fn transform_point(&self, p: Vec3) -> (f32, f32, f32, f32) {
        let w = self.data[3] * p.x + self.data[7] * p.y + self.data[11] * p.z + self.data[15];
        let x = (self.data[0] * p.x + self.data[4] * p.y + self.data[8] * p.z + self.data[12]) / w;
        let y = (self.data[1] * p.x + self.data[5] * p.y + self.data[9] * p.z + self.data[13]) / w;
        let z = (self.data[2] * p.x + self.data[6] * p.y + self.data[10] * p.z + self.data[14]) / w;
        (x, y, z, w)
    }
}

fn edge_function(a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> f32 {
    (c.0 - a.0) * (b.1 - a.1) - (c.1 - a.1) * (b.0 - a.0)
}

/// Render triangles to the buffer.
pub fn render_scene(buffer: &mut RenderBuffer, triangles: &[Triangle], camera: &Camera) {
    buffer.clear(30, 30, 35);

    let aspect = buffer.width as f32 / buffer.height as f32;
    let view = Mat4::look_at(camera.position, camera.target, camera.up);
    let proj = Mat4::perspective(camera.fov * PI / 180.0, aspect, 0.1, 1000.0);
    let mvp = view.multiply(&proj);

    // Ground grid first (behind objects)
    render_ground_grid(buffer, camera, &mvp);

    if triangles.is_empty() {
        return;
    }

    // Light direction (from top-right-front)
    let light_dir = Vec3::new(0.5, 0.8, 0.3).normalize();

    for tri in triangles {
        let v0 = Vec3::new(tri.v0[0], tri.v0[1], tri.v0[2]);
        let v1 = Vec3::new(tri.v1[0], tri.v1[1], tri.v1[2]);
        let v2 = Vec3::new(tri.v2[0], tri.v2[1], tri.v2[2]);

        let (p0x, p0y, p0z, p0w) = mvp.transform_point(v0);
        let (p1x, p1y, p1z, p1w) = mvp.transform_point(v1);
        let (p2x, p2y, p2z, p2w) = mvp.transform_point(v2);

        // Clip triangles behind camera
        if p0w < 0.1 || p1w < 0.1 || p2w < 0.1 {
            continue;
        }

        // Convert to screen coordinates
        let w = buffer.width as f32;
        let h = buffer.height as f32;
        let s0 = ((p0x + 1.0) * 0.5 * w, (1.0 - p0y) * 0.5 * h, p0z);
        let s1 = ((p1x + 1.0) * 0.5 * w, (1.0 - p1y) * 0.5 * h, p1z);
        let s2 = ((p2x + 1.0) * 0.5 * w, (1.0 - p2y) * 0.5 * h, p2z);

        // Compute face normal for lighting
        let edge1 = v1.sub(v0);
        let edge2 = v2.sub(v0);
        let normal = edge1.cross(edge2).normalize();

        // Screen area for winding check
        let screen_area = edge_function((s0.0, s0.1), (s1.0, s1.1), (s2.0, s2.1));
        if screen_area.abs() < 0.001 {
            continue;
        }

        // Two-sided lighting
        let view_dir = camera.position.sub(v0).normalize();
        let ndotv = normal.dot(view_dir);
        let shading_normal = if ndotv < 0.0 {
            normal.scale(-1.0)
        } else {
            normal
        };
        let ndotl = shading_normal.dot(light_dir).max(0.0);
        let ambient = 0.3;
        let diffuse = 0.7;
        let intensity = ambient + diffuse * ndotl;

        let lit_r = ((tri.color[0] as f32) * intensity).min(255.0) as u8;
        let lit_g = ((tri.color[1] as f32) * intensity).min(255.0) as u8;
        let lit_b = ((tri.color[2] as f32) * intensity).min(255.0) as u8;

        // Bounding box
        let min_x = s0.0.min(s1.0).min(s2.0).max(0.0) as u32;
        let max_x = s0.0.max(s1.0).max(s2.0).min(w - 1.0) as u32;
        let min_y = s0.1.min(s1.1).min(s2.1).max(0.0) as u32;
        let max_y = s0.1.max(s1.1).max(s2.1).min(h - 1.0) as u32;

        // Rasterize
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let p = (x as f32 + 0.5, y as f32 + 0.5);

                let w0 = edge_function((s1.0, s1.1), (s2.0, s2.1), p);
                let w1 = edge_function((s2.0, s2.1), (s0.0, s0.1), p);
                let w2 = edge_function((s0.0, s0.1), (s1.0, s1.1), p);

                // Check if point is inside triangle (handle both windings)
                let inside =
                    (w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0) || (w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0);

                if inside {
                    // Interpolate depth
                    let z = (w0 * s0.2 + w1 * s1.2 + w2 * s2.2) / screen_area;
                    buffer.set_pixel_with_id(x, y, z, [lit_r, lit_g, lit_b], tri.pick_id);
                }
            }
        }
    }

    // Axis indicator (bottom-left corner, rendered last so it's on top)
    render_axis_indicator(buffer, camera, 4, buffer.height.saturating_sub(34), 30);
}

/// Render a ground plane grid on the XZ plane (Y=0) with adaptive spacing.
fn render_ground_grid(buffer: &mut RenderBuffer, camera: &Camera, mvp: &Mat4) {
    let dist = camera.distance;
    let w = buffer.width as f32;
    let h = buffer.height as f32;

    // Pixels per world unit at the target distance
    let pixels_per_unit = h / (2.0 * dist * (camera.fov * PI / 360.0).tan());

    // Base spacing from camera distance
    let base_spacing = if dist < 50.0 {
        5.0f32
    } else if dist < 200.0 {
        10.0
    } else if dist < 500.0 {
        50.0
    } else {
        100.0
    };

    // Ensure grid lines are at least 10 pixels apart on screen to avoid visual noise
    let min_spacing = 10.0 / pixels_per_unit.max(0.001);
    let spacing = base_spacing.max(min_spacing);

    let major_every = 5;
    // Cap line count to avoid excessive rendering at low resolution
    let half_extent = ((dist * 1.5 / spacing).ceil() as i32).min(15);
    // Adaptive line width: ensure lines are at least ~1.5px wide on screen
    let min_world_width = 1.5 / pixels_per_unit.max(0.001);
    let line_half = (0.15 * spacing).max(min_world_width * 0.5);

    for i in -half_extent..=half_extent {
        let coord = i as f32 * spacing;
        let is_major = i % major_every == 0;
        let color: [u8; 3] = if is_major { [60, 60, 65] } else { [40, 40, 44] };

        // X-aligned line at z=coord: thin quad from (lo, 0, coord-hw) to (hi, 0, coord+hw)
        let lo = -half_extent as f32 * spacing;
        let hi = half_extent as f32 * spacing;

        // Z-line (parallel to X axis)
        rasterize_grid_line(
            buffer,
            mvp,
            w,
            h,
            Vec3::new(lo, 0.0, coord - line_half),
            Vec3::new(hi, 0.0, coord - line_half),
            Vec3::new(hi, 0.0, coord + line_half),
            Vec3::new(lo, 0.0, coord + line_half),
            color,
        );

        // X-line (parallel to Z axis)
        rasterize_grid_line(
            buffer,
            mvp,
            w,
            h,
            Vec3::new(coord - line_half, 0.0, lo),
            Vec3::new(coord + line_half, 0.0, lo),
            Vec3::new(coord + line_half, 0.0, hi),
            Vec3::new(coord - line_half, 0.0, hi),
            color,
        );
    }
}

/// Rasterize a thin grid-line quad (two triangles, no lighting, pick_id=0).
#[allow(clippy::too_many_arguments)]
fn rasterize_grid_line(
    buffer: &mut RenderBuffer,
    mvp: &Mat4,
    w: f32,
    h: f32,
    v0: Vec3,
    v1: Vec3,
    v2: Vec3,
    v3: Vec3,
    color: [u8; 3],
) {
    let verts = [v0, v1, v2, v3];
    let tris = [(0, 1, 2), (0, 2, 3)];

    for (a, b, c) in tris {
        let (p0x, p0y, p0z, p0w) = mvp.transform_point(verts[a]);
        let (p1x, p1y, p1z, p1w) = mvp.transform_point(verts[b]);
        let (p2x, p2y, p2z, p2w) = mvp.transform_point(verts[c]);

        if p0w < 0.1 || p1w < 0.1 || p2w < 0.1 {
            continue;
        }

        let s0 = ((p0x + 1.0) * 0.5 * w, (1.0 - p0y) * 0.5 * h, p0z);
        let s1 = ((p1x + 1.0) * 0.5 * w, (1.0 - p1y) * 0.5 * h, p1z);
        let s2 = ((p2x + 1.0) * 0.5 * w, (1.0 - p2y) * 0.5 * h, p2z);

        let screen_area = edge_function((s0.0, s0.1), (s1.0, s1.1), (s2.0, s2.1));
        if screen_area.abs() < 0.001 {
            continue;
        }

        let min_x = s0.0.min(s1.0).min(s2.0).max(0.0) as u32;
        let max_x = s0.0.max(s1.0).max(s2.0).min(w - 1.0) as u32;
        let min_y = s0.1.min(s1.1).min(s2.1).max(0.0) as u32;
        let max_y = s0.1.max(s1.1).max(s2.1).min(h - 1.0) as u32;

        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let p = (x as f32 + 0.5, y as f32 + 0.5);
                let w0 = edge_function((s1.0, s1.1), (s2.0, s2.1), p);
                let w1 = edge_function((s2.0, s2.1), (s0.0, s0.1), p);
                let w2 = edge_function((s0.0, s0.1), (s1.0, s1.1), p);

                let inside =
                    (w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0) || (w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0);

                if inside {
                    let z = (w0 * s0.2 + w1 * s1.2 + w2 * s2.2) / screen_area;
                    buffer.set_pixel_with_id(x, y, z, color, 0);
                }
            }
        }
    }
}

/// Render an axis orientation indicator (X=red, Y=green, Z=blue) in a sub-region.
fn render_axis_indicator(
    buffer: &mut RenderBuffer,
    camera: &Camera,
    corner_x: u32,
    corner_y: u32,
    size: u32,
) {
    if size < 8 || corner_x + size > buffer.width || corner_y + size > buffer.height {
        return;
    }

    // Use the same view rotation as the main camera, but fixed orthographic
    let view_rot = Mat4::look_at(
        camera.position.sub(camera.target).normalize(),
        Vec3::new(0.0, 0.0, 0.0),
        camera.up,
    );

    let axes: [([f32; 3], [u8; 3], char); 3] = [
        ([1.0, 0.0, 0.0], [220, 60, 60], 'X'),  // X = red
        ([0.0, 1.0, 0.0], [60, 200, 60], 'Y'),  // Y = green
        ([0.0, 0.0, 1.0], [60, 100, 220], 'Z'), // Z = blue
    ];

    let center_x = corner_x + size / 2;
    let center_y = corner_y + size / 2;
    let axis_len = (size as f32) * 0.38;

    // Sort axes back-to-front for correct overlap
    let mut sorted_axes: Vec<(usize, f32)> = (0..3)
        .map(|i| {
            let a = axes[i].0;
            let v = Vec3::new(a[0], a[1], a[2]);
            let (_, _, z, _) = view_rot.transform_point(v);
            (i, z)
        })
        .collect();
    sorted_axes.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    for (idx, _) in sorted_axes {
        let (dir, color, label) = &axes[idx];
        let v = Vec3::new(dir[0], dir[1], dir[2]);
        let (sx, sy, _, _) = view_rot.transform_point(v);

        // Screen endpoint
        let ex = center_x as f32 + sx * axis_len;
        let ey = center_y as f32 - sy * axis_len; // Y flipped

        // Draw line from center to endpoint (Bresenham)
        draw_line(
            buffer,
            center_x as i32,
            center_y as i32,
            ex as i32,
            ey as i32,
            *color,
        );

        // Draw a small circle/dot at the tip
        let tip_x = ex as i32;
        let tip_y = ey as i32;
        for dy in -1..=1i32 {
            for dx in -1..=1i32 {
                let px = (tip_x + dx) as u32;
                let py = (tip_y + dy) as u32;
                if px < buffer.width && py < buffer.height {
                    let idx = (py * buffer.width + px) as usize;
                    buffer.pixels[idx * 4] = color[0];
                    buffer.pixels[idx * 4 + 1] = color[1];
                    buffer.pixels[idx * 4 + 2] = color[2];
                    buffer.pick_ids[idx] = 0;
                }
            }
        }

        // Draw label 4px past tip
        let label_x = (ex + sx * 5.0) as i32;
        let label_y = (ey - sy * 5.0) as i32;
        draw_tiny_char(buffer, label_x - 2, label_y - 3, *label, *color);
    }
}

/// Bresenham line drawing.
fn draw_line(buffer: &mut RenderBuffer, x0: i32, y0: i32, x1: i32, y1: i32, color: [u8; 3]) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut x = x0;
    let mut y = y0;

    loop {
        if x >= 0 && y >= 0 {
            let px = x as u32;
            let py = y as u32;
            if px < buffer.width && py < buffer.height {
                let idx = (py * buffer.width + px) as usize;
                buffer.pixels[idx * 4] = color[0];
                buffer.pixels[idx * 4 + 1] = color[1];
                buffer.pixels[idx * 4 + 2] = color[2];
                buffer.depth[idx] = -1.0; // Always on top
                buffer.pick_ids[idx] = 0;
            }
        }
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

/// Minimal 5x7 bitmap font for axis labels (X, Y, Z only).
fn draw_tiny_char(buffer: &mut RenderBuffer, x: i32, y: i32, ch: char, color: [u8; 3]) {
    // 5-wide bitmaps, 7 rows each
    let bitmap: &[u8] = match ch {
        'X' => &[
            0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b01010, 0b10001,
        ],
        'Y' => &[
            0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'Z' => &[
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        _ => return,
    };

    for (row, &bits) in bitmap.iter().enumerate() {
        for col in 0..5 {
            if bits & (1 << (4 - col)) != 0 {
                let px = x + col;
                let py = y + row as i32;
                if px >= 0 && py >= 0 {
                    let px = px as u32;
                    let py = py as u32;
                    if px < buffer.width && py < buffer.height {
                        let idx = (py * buffer.width + px) as usize;
                        buffer.pixels[idx * 4] = color[0];
                        buffer.pixels[idx * 4 + 1] = color[1];
                        buffer.pixels[idx * 4 + 2] = color[2];
                        buffer.pick_ids[idx] = 0;
                    }
                }
            }
        }
    }
}

#[allow(dead_code)]
fn draw_grid(buffer: &mut RenderBuffer) {
    let w = buffer.width;
    let h = buffer.height;
    let cx = w / 2;
    let cy = h / 2;

    // Draw crosshair
    for x in 0..w {
        let idx = (cy * w + x) as usize;
        if idx < buffer.pixels.len() / 4 {
            buffer.pixels[idx * 4] = 60;
            buffer.pixels[idx * 4 + 1] = 60;
            buffer.pixels[idx * 4 + 2] = 70;
        }
    }
    for y in 0..h {
        let idx = (y * w + cx) as usize;
        if idx < buffer.pixels.len() / 4 {
            buffer.pixels[idx * 4] = 60;
            buffer.pixels[idx * 4 + 1] = 60;
            buffer.pixels[idx * 4 + 2] = 70;
        }
    }

    // Draw border
    for x in 0..w {
        buffer.pixels[(x * 4) as usize] = 80;
        buffer.pixels[(x * 4 + 1) as usize] = 80;
        buffer.pixels[(x * 4 + 2) as usize] = 90;
        let bot = ((h - 1) * w + x) as usize * 4;
        if bot + 2 < buffer.pixels.len() {
            buffer.pixels[bot] = 80;
            buffer.pixels[bot + 1] = 80;
            buffer.pixels[bot + 2] = 90;
        }
    }
    for y in 0..h {
        let left = (y * w) as usize * 4;
        buffer.pixels[left] = 80;
        buffer.pixels[left + 1] = 80;
        buffer.pixels[left + 2] = 90;
        let right = (y * w + w - 1) as usize * 4;
        if right + 2 < buffer.pixels.len() {
            buffer.pixels[right] = 80;
            buffer.pixels[right + 1] = 80;
            buffer.pixels[right + 2] = 90;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vec3_operations() {
        let a = Vec3::new(1.0, 2.0, 3.0);
        let b = Vec3::new(4.0, 5.0, 6.0);

        assert!((a.dot(b) - 32.0).abs() < 0.001);

        let cross = a.cross(b);
        assert!((cross.x - (-3.0)).abs() < 0.001);
        assert!((cross.y - 6.0).abs() < 0.001);
        assert!((cross.z - (-3.0)).abs() < 0.001);
    }

    #[test]
    fn test_camera_default() {
        let camera = Camera::default();
        assert!(camera.distance > 0.0);
        assert_eq!(camera.fov, 60.0);
    }

    #[test]
    fn test_camera_rotation() {
        let mut camera = Camera::default();
        let initial_pos = camera.position;

        camera.rotate_horizontal(45.0);
        assert!((camera.position.x - initial_pos.x).abs() > 0.1);
    }

    #[test]
    fn test_camera_zoom() {
        let mut camera = Camera::default();
        let initial_dist = camera.distance;

        camera.zoom(0.5);
        assert!(camera.distance < initial_dist);

        camera.zoom(2.0);
        assert!((camera.distance - initial_dist).abs() < 0.1);
    }

    #[test]
    fn test_render_empty_scene() {
        let mut buffer = RenderBuffer::new(40, 20);
        let camera = Camera::default();
        let triangles: Vec<Triangle> = vec![];

        render_scene(&mut buffer, &triangles, &camera);

        // Should have drawn the grid
        assert!(buffer.pixels.iter().any(|&p| p > 0));
    }

    #[test]
    fn test_render_with_triangles() {
        // Just test that rendering triangles doesn't panic and produces output
        let mut buffer = RenderBuffer::new(100, 100);
        let camera = Camera::default();

        // Create triangles that should be visible from the default camera position
        // Default camera is at ~(61, 50, 75) looking at origin
        let triangles = vec![Triangle {
            v0: [-10.0, -10.0, 0.0],
            v1: [10.0, -10.0, 0.0],
            v2: [0.0, 10.0, 0.0],
            color: [180, 180, 190],
            pick_id: 1,
        }];

        // This should not panic
        render_scene(&mut buffer, &triangles, &camera);

        // Buffer should be modified (at least cleared)
        assert!(buffer.pixels.iter().any(|&p| p > 0));
    }
}
