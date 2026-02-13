//! Orbit camera model.

use serde::{Deserialize, Serialize};

/// Orbit camera for 3D viewport.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Camera {
    /// Horizontal angle in degrees.
    pub azimuth: f64,
    /// Vertical angle in degrees, clamped to [-89, 89].
    pub elevation: f64,
    /// Distance from target.
    pub distance: f64,
    /// Look-at target point.
    pub target: [f64; 3],
    /// Field of view in degrees.
    pub fov: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            azimuth: 45.0,
            elevation: 30.0,
            distance: 100.0,
            target: [0.0, 0.0, 0.0],
            fov: 60.0,
        }
    }
}

impl Camera {
    /// Orbit the camera by delta angles.
    pub fn orbit(&mut self, d_azimuth: f64, d_elevation: f64) {
        self.azimuth += d_azimuth;
        self.elevation = (self.elevation + d_elevation).clamp(-89.0, 89.0);
    }

    /// Pan the camera target in screen-relative directions.
    pub fn pan(&mut self, dx: f64, dy: f64) {
        // Compute right and up vectors from current orientation
        let az = self.azimuth.to_radians();
        let el = self.elevation.to_radians();

        // Right vector (perpendicular to view direction in XZ plane)
        let right_x = az.cos();
        let right_z = -az.sin();

        // Up vector (perpendicular to view and right)
        let up_x = -el.sin() * az.sin();
        let up_y = el.cos();
        let up_z = -el.sin() * az.cos();

        // Scale by distance for consistent feel
        let scale = self.distance * 0.002;

        self.target[0] += (right_x * dx + up_x * dy) * scale;
        self.target[1] += up_y * dy * scale;
        self.target[2] += (right_z * dx + up_z * dy) * scale;
    }

    /// Zoom by a multiplicative factor (< 1 zooms in, > 1 zooms out).
    pub fn zoom(&mut self, factor: f64) {
        self.distance = (self.distance * factor).clamp(1.0, 10000.0);
    }

    /// Reset to default view.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Compute camera position from spherical coordinates.
    pub fn position(&self) -> [f64; 3] {
        let az = self.azimuth.to_radians();
        let el = self.elevation.to_radians();
        [
            self.target[0] + self.distance * el.cos() * az.sin(),
            self.target[1] + self.distance * el.sin(),
            self.target[2] + self.distance * el.cos() * az.cos(),
        ]
    }

    /// Compute 4x4 view matrix (column-major).
    pub fn view_matrix(&self) -> [[f64; 4]; 4] {
        let pos = self.position();
        let eye = [pos[0], pos[1], pos[2]];
        let target = self.target;
        let up = [0.0, 1.0, 0.0];

        // Forward (z-axis pointing from target to eye)
        let fwd = [
            eye[0] - target[0],
            eye[1] - target[1],
            eye[2] - target[2],
        ];
        let fwd_len = (fwd[0]*fwd[0] + fwd[1]*fwd[1] + fwd[2]*fwd[2]).sqrt();
        let z = [fwd[0]/fwd_len, fwd[1]/fwd_len, fwd[2]/fwd_len];

        // Right (x-axis)
        let rx = up[1]*z[2] - up[2]*z[1];
        let ry = up[2]*z[0] - up[0]*z[2];
        let rz = up[0]*z[1] - up[1]*z[0];
        let r_len = (rx*rx + ry*ry + rz*rz).sqrt();
        let x = [rx/r_len, ry/r_len, rz/r_len];

        // True up (y-axis)
        let y = [
            z[1]*x[2] - z[2]*x[1],
            z[2]*x[0] - z[0]*x[2],
            z[0]*x[1] - z[1]*x[0],
        ];

        let tx = -(x[0]*eye[0] + x[1]*eye[1] + x[2]*eye[2]);
        let ty = -(y[0]*eye[0] + y[1]*eye[1] + y[2]*eye[2]);
        let tz = -(z[0]*eye[0] + z[1]*eye[1] + z[2]*eye[2]);

        [
            [x[0], y[0], z[0], 0.0],
            [x[1], y[1], z[1], 0.0],
            [x[2], y[2], z[2], 0.0],
            [tx,   ty,   tz,   1.0],
        ]
    }

    /// Compute 4x4 perspective projection matrix (column-major).
    pub fn projection_matrix(&self, aspect: f64) -> [[f64; 4]; 4] {
        let fov_rad = self.fov.to_radians();
        let f = 1.0 / (fov_rad / 2.0).tan();
        let near = 0.1;
        let far = 10000.0;
        let nf = 1.0 / (near - far);

        [
            [f / aspect, 0.0, 0.0, 0.0],
            [0.0, f, 0.0, 0.0],
            [0.0, 0.0, (far + near) * nf, -1.0],
            [0.0, 0.0, 2.0 * far * near * nf, 0.0],
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_position() {
        let cam = Camera::default();
        let pos = cam.position();
        // At azimuth=45, elevation=30, distance=100
        assert!(pos[1] > 0.0); // above target
        assert!((pos[0] * pos[0] + pos[1] * pos[1] + pos[2] * pos[2]).sqrt() - 100.0 < 1.0);
    }

    #[test]
    fn test_orbit_clamp() {
        let mut cam = Camera::default();
        cam.orbit(0.0, 100.0);
        assert!(cam.elevation <= 89.0);
        cam.orbit(0.0, -200.0);
        assert!(cam.elevation >= -89.0);
    }

    #[test]
    fn test_zoom_clamp() {
        let mut cam = Camera::default();
        cam.zoom(0.001);
        assert!(cam.distance >= 1.0);
        cam.zoom(1e10);
        assert!(cam.distance <= 10000.0);
    }

    #[test]
    fn test_reset() {
        let mut cam = Camera::default();
        cam.orbit(30.0, 20.0);
        cam.zoom(2.0);
        cam.reset();
        assert_eq!(cam.azimuth, 45.0);
        assert_eq!(cam.elevation, 30.0);
        assert_eq!(cam.distance, 100.0);
    }
}
