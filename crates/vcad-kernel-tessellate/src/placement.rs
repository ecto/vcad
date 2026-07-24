//! Rigid placement of baked render meshes.
//!
//! Applies the standard translate/rotate/scale placement the IR's
//! `Translate`/`Rotate`/`Scale` wrapper chain describes to flat `f32`
//! position/normal buffers. Rotation order is **Rz·Ry·Rx** (X applied
//! first), matching the document evaluator's transform chain and the app's
//! three.js "ZYX" euler order. Scale is applied before rotation; normals get
//! the rotation only (no scale, no renormalization) — bit-for-bit the
//! contract of the web engine's `transformMesh`, which this replaces.

/// Apply `scale → rotate (Rz·Ry·Rx, degrees) → translate` to `positions`,
/// and the rotation alone to `normals` when present. Buffers are flat
/// `[x0, y0, z0, x1, ...]`; math runs in `f64` and is truncated to `f32` on
/// store, mirroring JS `Float32Array` semantics.
pub fn apply_placement(
    positions: &mut [f32],
    normals: Option<&mut [f32]>,
    translate: [f64; 3],
    rotate_deg: [f64; 3],
    scale: [f64; 3],
) {
    let rx = rotate_deg[0].to_radians();
    let ry = rotate_deg[1].to_radians();
    let rz = rotate_deg[2].to_radians();

    let (sx, cx) = rx.sin_cos();
    let (sy, cy) = ry.sin_cos();
    let (sz, cz) = rz.sin_cos();

    let m00 = cy * cz;
    let m01 = sx * sy * cz - cx * sz;
    let m02 = cx * sy * cz + sx * sz;
    let m10 = cy * sz;
    let m11 = sx * sy * sz + cx * cz;
    let m12 = cx * sy * sz - sx * cz;
    let m20 = -sy;
    let m21 = sx * cy;
    let m22 = cx * cy;

    for p in positions.chunks_exact_mut(3) {
        let x = p[0] as f64 * scale[0];
        let y = p[1] as f64 * scale[1];
        let z = p[2] as f64 * scale[2];

        p[0] = (m00 * x + m01 * y + m02 * z + translate[0]) as f32;
        p[1] = (m10 * x + m11 * y + m12 * z + translate[1]) as f32;
        p[2] = (m20 * x + m21 * y + m22 * z + translate[2]) as f32;
    }

    if let Some(normals) = normals {
        for n in normals.chunks_exact_mut(3) {
            let nx = n[0] as f64;
            let ny = n[1] as f64;
            let nz = n[2] as f64;

            n[0] = (m00 * nx + m01 * ny + m02 * nz) as f32;
            n[1] = (m10 * nx + m11 * ny + m12 * nz) as f32;
            n[2] = (m20 * nx + m21 * ny + m22 * nz) as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const I3: [f64; 3] = [1.0, 1.0, 1.0];
    const Z3: [f64; 3] = [0.0, 0.0, 0.0];

    fn close(a: &[f32], b: &[f32]) {
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b) {
            assert!((x - y).abs() < 1e-5, "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn translate_and_scale() {
        let mut p = vec![1.0, 2.0, 3.0];
        apply_placement(&mut p, None, [10.0, 20.0, 30.0], Z3, [2.0, 3.0, 4.0]);
        close(&p, &[12.0, 26.0, 42.0]);
    }

    #[test]
    fn rotation_order_is_rz_ry_rx() {
        // Rotate +X 90° about X, then 90° about Z: Rx sends x̂→x̂,
        // Rz then sends x̂→ŷ. (X applied first ⇒ result ŷ; the reverse
        // order would land on ẑ.)
        let mut p = vec![1.0, 0.0, 0.0];
        apply_placement(&mut p, None, Z3, [90.0, 0.0, 90.0], I3);
        close(&p, &[0.0, 1.0, 0.0]);

        // ŷ under the same rotation: Rx sends ŷ→ẑ, Rz keeps ẑ.
        let mut p = vec![0.0, 1.0, 0.0];
        apply_placement(&mut p, None, Z3, [90.0, 0.0, 90.0], I3);
        close(&p, &[0.0, 0.0, 1.0]);
    }

    #[test]
    fn normals_get_rotation_only() {
        let mut p = vec![0.0f32; 3];
        let mut n = vec![0.0, 0.0, 1.0];
        apply_placement(
            &mut p,
            Some(&mut n),
            [5.0, 6.0, 7.0],
            [90.0, 0.0, 0.0],
            [2.0, 2.0, 2.0],
        );
        // Rx(90°): ẑ→-ŷ. No translate, no scale on normals.
        close(&n, &[0.0, -1.0, 0.0]);
        close(&p, &[5.0, 6.0, 7.0]);
    }

    #[test]
    fn general_rotation_matches_composed_transform() {
        // Cross-check against the placement euler convention — extrinsic
        // X→Y→Z, matrix Rz·Ry·Rx — the one vcad-eval's kinematics
        // `euler_to_matrix`, the engine's `transformMesh`, and the app's
        // three.js "ZYX" euler all share. (`Transform::then` composes
        // self·other with column vectors, so `other` acts on the point
        // first: Rz.then(Ry).then(Rx) = Rz·Ry·Rx = X applied first.)
        use vcad_kernel_math::Transform;
        let (rx, ry, rz): (f64, f64, f64) = (30.0, 45.0, 60.0);
        let t = Transform::rotation_z(rz.to_radians())
            .then(&Transform::rotation_y(ry.to_radians()))
            .then(&Transform::rotation_x(rx.to_radians()));
        let pt = t.apply_point(&vcad_kernel_math::Point3::new(1.0, 2.0, 3.0));

        let mut p = vec![1.0, 2.0, 3.0];
        apply_placement(&mut p, None, Z3, [rx, ry, rz], I3);
        close(&p, &[pt.x as f32, pt.y as f32, pt.z as f32]);
    }
}
