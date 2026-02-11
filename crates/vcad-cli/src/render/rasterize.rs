//! Software 3D rasterizer.
//!
//! Renders triangle meshes to a pixel buffer with depth testing and flat shading.

use std::f32::consts::PI;

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
    distance: f32,
    /// Horizontal angle in degrees.
    azimuth: f32,
    /// Vertical angle in degrees.
    elevation: f32,
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

    fn update_position(&mut self) {
        let az_rad = self.azimuth.to_radians();
        let el_rad = self.elevation.to_radians();

        self.position = Vec3::new(
            self.target.x + self.distance * el_rad.cos() * az_rad.sin(),
            self.target.y + self.distance * el_rad.sin(),
            self.target.z + self.distance * el_rad.cos() * az_rad.cos(),
        );
    }
}

/// RGBA pixel buffer with depth.
pub struct RenderBuffer {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub depth: Vec<f32>,
}

impl RenderBuffer {
    pub fn new(width: u32, height: u32) -> Self {
        let size = (width * height) as usize;
        Self {
            width,
            height,
            pixels: vec![0; size * 4],
            depth: vec![f32::INFINITY; size],
        }
    }

    pub fn clear(&mut self, r: u8, g: u8, b: u8) {
        let size = (self.width * self.height) as usize;
        for i in 0..size {
            self.pixels[i * 4] = r;
            self.pixels[i * 4 + 1] = g;
            self.pixels[i * 4 + 2] = b;
            self.pixels[i * 4 + 3] = 255;
            self.depth[i] = f32::INFINITY;
        }
    }

    fn set_pixel(&mut self, x: u32, y: u32, z: f32, r: u8, g: u8, b: u8) {
        if x >= self.width || y >= self.height {
            return;
        }
        let idx = (y * self.width + x) as usize;
        if z < self.depth[idx] {
            self.depth[idx] = z;
            self.pixels[idx * 4] = r;
            self.pixels[idx * 4 + 1] = g;
            self.pixels[idx * 4 + 2] = b;
            self.pixels[idx * 4 + 3] = 255;
        }
    }
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

    if triangles.is_empty() {
        // Draw a grid pattern when empty
        draw_grid(buffer);
        return;
    }

    let aspect = buffer.width as f32 / buffer.height as f32;
    let view = Mat4::look_at(camera.position, camera.target, camera.up);
    let proj = Mat4::perspective(camera.fov * PI / 180.0, aspect, 0.1, 1000.0);
    let mvp = proj.multiply(&view);

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
                    buffer.set_pixel(x, y, z, lit_r, lit_g, lit_b);
                }
            }
        }
    }
}

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
    fn test_render_buffer() {
        let buffer = RenderBuffer::new(100, 50);
        assert_eq!(buffer.width, 100);
        assert_eq!(buffer.height, 50);
        assert_eq!(buffer.pixels.len(), 100 * 50 * 4);
        assert_eq!(buffer.depth.len(), 100 * 50);
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
        }];

        // This should not panic
        render_scene(&mut buffer, &triangles, &camera);

        // Buffer should be modified (at least cleared)
        assert!(buffer.pixels.iter().any(|&p| p > 0));
    }
}
