//! World-space raycasting for the viewport cursor readout.
//!
//! Given the current camera and a mouse cell position, build a pinhole
//! ray and intersect it with the ground plane so the status bar can
//! show live XYZ coordinates under the cursor.
//!
//! Coordinate frames: the camera carries its own [`UpAxis`], and vcad
//! drives termview Z-up (grid in XY, Z vertical) to match the kernel's
//! stored frame per CLAUDE.md. The ray is intersected with the plane
//! `up == 0` in whichever frame the camera is using, so the hit point is
//! already world space — no axis swap.

use crate::render::Camera;

/// Intersect a camera ray built from cell `(col, row)` with the ground
/// plane. Returns the hit point in Z-up world space (mm) so the status
/// bar can render `x / y / z` directly.
///
/// - `col`, `row`: mouse cell position within `area`
/// - `area_x`, `area_y`: top-left of the viewport area in cells
/// - `area_w`, `area_h`: size of the viewport area in cells
///
/// Returns `None` when the cell is outside `area`, the ray is parallel
/// to the ground, or the hit point is behind the camera.
pub fn raycast_ground_plane(
    camera: &Camera,
    col: u16,
    row: u16,
    area_x: u16,
    area_y: u16,
    area_w: u16,
    area_h: u16,
) -> Option<(f64, f64, f64)> {
    if area_w == 0 || area_h == 0 {
        return None;
    }
    if col < area_x || row < area_y {
        return None;
    }
    let local_col = col.saturating_sub(area_x) as f32;
    let local_row = row.saturating_sub(area_y) as f32;
    if local_col >= area_w as f32 || local_row >= area_h as f32 {
        return None;
    }

    // Cell aspect ≈ 2:1 (height:width) — half-block rendering means each
    // cell is one pixel wide but represents two pixels vertically, and
    // terminal cells themselves are usually around 2× taller than wide
    // even for the top/bottom halves. So the effective aspect is w:h ≈
    // (cols) : (rows × 0.5).
    let effective_h = (area_h as f32 * 0.5).max(1.0);
    let aspect = area_w as f32 / effective_h;

    // NDC-ish: u, v in [-1, 1]. v is flipped because row grows downward.
    let u = (local_col + 0.5) / area_w as f32 * 2.0 - 1.0;
    let v = 1.0 - (local_row + 0.5) / area_h as f32 * 2.0;

    // Camera basis vectors, all in plain [f32; 3] so the math stays
    // local to this file without leaking termview::Vec3 everywhere.
    let eye = as_arr(camera.position);
    let target = as_arr(camera.target);
    let world_up = as_arr(camera.up);

    let forward = normalize(sub(target, eye))?;
    let right = normalize(cross(forward, world_up))?;
    let cam_up = cross(right, forward);

    // Perspective view-plane direction.
    let tan_half_fov = (camera.fov.to_radians() / 2.0).tan();
    let ru = u * aspect * tan_half_fov;
    let vu = v * tan_half_fov;
    let dir = normalize([
        forward[0] + right[0] * ru + cam_up[0] * vu,
        forward[1] + right[1] * ru + cam_up[1] * vu,
        forward[2] + right[2] * ru + cam_up[2] * vu,
    ])?;

    // Ground plane: the coordinate along the camera's up axis is 0.
    let axis = match camera.up_axis {
        crate::render::UpAxis::Z => 2,
        crate::render::UpAxis::Y => 1,
    };
    if dir[axis].abs() < 1e-6 {
        return None;
    }
    let t = -eye[axis] / dir[axis];
    if t <= 0.0 {
        return None;
    }
    let mut hit = [
        eye[0] + dir[0] * t,
        eye[1] + dir[1] * t,
        eye[2] + dir[2] * t,
    ];
    hit[axis] = 0.0;

    Some((hit[0] as f64, hit[1] as f64, hit[2] as f64))
}

// ---------------------------------------------------------------------------
// Tiny vector helpers operating on [f32; 3]. termview::Vec3 lives in a
// separate crate and has its own methods — we convert once at the top
// of `raycast_ground_plane` and stay in arrays from there.
// ---------------------------------------------------------------------------

fn as_arr(v: crate::render::Vec3) -> [f32; 3] {
    [v.x, v.y, v.z]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(v: [f32; 3]) -> Option<[f32; 3]> {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len < 1e-6 {
        None
    } else {
        Some([v[0] / len, v[1] / len, v[2] / len])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{Camera, Vec3};

    /// Build a Z-up camera at (0, -100, 100) looking at the origin,
    /// 60° fov. Mirrors termview::Camera's structure.
    fn test_camera() -> Camera {
        let mut c = Camera::default();
        c.position = Vec3::new(0.0, -100.0, 100.0);
        c.target = Vec3::new(0.0, 0.0, 0.0);
        c.up = Vec3::new(0.0, 0.0, 1.0);
        c.fov = 60.0;
        c
    }

    #[test]
    fn center_cell_hits_ground_near_origin_xy() {
        // The camera looks roughly at the origin, so a cell at the
        // center of a 100x40 viewport should raycast to something
        // approximately on the ground near (0, ?, 0) in Z-up.
        let cam = test_camera();
        let hit = raycast_ground_plane(&cam, 50, 20, 0, 0, 100, 40);
        assert!(hit.is_some(), "center cell should hit ground");
        let (x, _y, z) = hit.unwrap();
        assert!(
            x.abs() < 5.0,
            "center cell's X should be near origin, got {x}"
        );
        assert!(z.abs() < 1e-4, "ground hit's Z should be ~0, got {z}");
    }

    #[test]
    fn out_of_area_returns_none() {
        let cam = test_camera();
        assert!(raycast_ground_plane(&cam, 200, 200, 0, 0, 100, 40).is_none());
        assert!(raycast_ground_plane(&cam, 50, 20, 60, 25, 100, 40).is_none());
    }

    #[test]
    fn zero_area_returns_none() {
        let cam = test_camera();
        assert!(raycast_ground_plane(&cam, 10, 10, 0, 0, 0, 40).is_none());
        assert!(raycast_ground_plane(&cam, 10, 10, 0, 0, 100, 0).is_none());
    }

    #[test]
    fn right_of_center_has_positive_x() {
        let cam = test_camera();
        let left = raycast_ground_plane(&cam, 20, 20, 0, 0, 100, 40).unwrap();
        let right = raycast_ground_plane(&cam, 80, 20, 0, 0, 100, 40).unwrap();
        assert!(
            right.0 > left.0,
            "rightward cursor should increase X (left={}, right={})",
            left.0,
            right.0
        );
    }
}
