// The BRep geometry module: kosm-render's geometry contract implemented over
// trimmed analytic faces.
//
// Not valid WGSL on its own — `kosm_render::gpu::shaders::compose` puts
// `bsdf.wgsl` and `prelude.wgsl` in front of it and the integrator after.
// It owns bind-group-0 bindings 1..=5; everything else belongs to the
// renderer. See `crate::gpu::BrepGeometry`.

// Structures matching Rust definitions

struct GpuFace {
    surface_idx: u32,
    orientation: u32,
    trim_start: u32,
    trim_count: u32,
    aabb_min: vec4<f32>,
    aabb_max: vec4<f32>,
    inner_start: u32,
    inner_count: u32,
    inner_loop_count: u32,
    inner_desc_start: u32,
    material_idx: u32,
    // Use explicit u32 padding instead of vec3<u32> to match Rust layout
    // vec3<u32> in WGSL has 16-byte alignment which would misalign struct
    _pad2_0: u32,
    _pad2_1: u32,
    _pad2_2: u32,
}

struct GpuBvhNode {
    aabb_min: vec4<f32>,
    aabb_max: vec4<f32>,
    left_or_first: u32,
    right_or_count: u32,
    is_leaf: u32,
    _pad: u32,
}

@group(0) @binding(1) var<storage, read> surfaces: array<GpuSurface>;
@group(0) @binding(2) var<storage, read> faces: array<GpuFace>;
@group(0) @binding(3) var<storage, read> bvh_nodes: array<GpuBvhNode>;
@group(0) @binding(4) var<storage, read> trim_verts: array<vec2<f32>>;
@group(0) @binding(5) var<storage, read> inner_loop_descs: array<u32>;

// Ray-surface intersection functions

// The ray parameter at the ray's closest approach to the origin of the frame
// `oc` is measured in, clamped to the ray's own start.
//
// Every analytic solve below re-origins the ray here before forming its
// polynomial, and this is the whole of why. `GpuScene::placed` bakes each
// placement into the packed surface frame, so a millimetre scene puts a
// basketball's seam — a torus of R 74 mm and r 2 mm — ten metres from the eye
// at a world coordinate of 1e4. Solved from the eye, the quartic's
// coefficients are built out of |o| ~ 1e4 terms (od, oo, and the depressed
// `p = b - 3a²/8` that cancels two of them against each other) while the
// answer they have to resolve is the 2 mm tube. In f32 that leaves nothing:
// the seam comes back as a burr of spurious hits and misses around the ball.
//
// Sliding the origin down the ray to the surface's own neighbourhood costs a
// dot product and makes every coefficient the size of the surface instead of
// the size of the scene. `t` is invariant under the shift — it is a rigid
// translation of the ray along itself — so the root simply comes back with
// `t0` added, and roots between the shifted origin and the true one are the
// (still valid) ones at negative local `t`, which is why each solve's
// acceptance test becomes `t >= -t0` rather than `t >= 0`.
fn closest_approach(oc: vec3<f32>, dir: vec3<f32>) -> f32 {
    return max(-dot(oc, dir) / dot(dir, dir), 0.0);
}

fn intersect_plane(origin: vec3<f32>, dir: vec3<f32>, params: array<f32, 32>) -> RayHit {
    var hit: RayHit;
    hit.t = MAX_T;
    hit.face_idx = 0xFFFFFFFFu;

    let plane_origin = vec3<f32>(params[0], params[1], params[2]);
    let plane_normal = vec3<f32>(params[9], params[10], params[11]);

    let denom = dot(dir, plane_normal);
    if abs(denom) < EPSILON {
        return hit;
    }

    // The ray's offset from the plane's own origin. A plane's solve is linear
    // and needs no re-origining, but its *parameterisation* does: computing
    // `to_p` as `origin + t*dir - plane_origin` differences two 1e4 world
    // coordinates and hands the trim test a uv that is a millimetre out.
    let q = origin - plane_origin;

    let t = -dot(q, plane_normal) / denom;
    if t < 0.0 {
        return hit;
    }

    hit.t = t;

    // Compute UV
    let x_dir = vec3<f32>(params[3], params[4], params[5]);
    let y_dir = vec3<f32>(params[6], params[7], params[8]);
    let to_p = q + t * dir;
    hit.uv = vec2<f32>(dot(to_p, x_dir), dot(to_p, y_dir));

    return hit;
}

fn intersect_sphere(origin: vec3<f32>, dir: vec3<f32>, params: array<f32, 32>) -> RayHit {
    var hit: RayHit;
    hit.t = MAX_T;
    hit.face_idx = 0xFFFFFFFFu;

    let center = vec3<f32>(params[0], params[1], params[2]);
    let radius = params[3];

    // Solve in the sphere's own frame, re-origined at the closest approach:
    // see `closest_approach`. `c = |oc|² - r²` is the cancellation this
    // spares — at a world coordinate of 2e4 it differences 4e8 against 1.4e4.
    let oc0 = origin - center;
    let t0 = closest_approach(oc0, dir);
    let oc = oc0 + t0 * dir;

    let a = dot(dir, dir);
    let b = 2.0 * dot(oc, dir);
    let c = dot(oc, oc) - radius * radius;

    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return hit;
    }

    let sqrt_disc = sqrt(disc);
    var t = (-b - sqrt_disc) / (2.0 * a);
    if t < -t0 {
        t = (-b + sqrt_disc) / (2.0 * a);
    }
    if t < -t0 {
        return hit;
    }

    hit.t = t + t0;

    // Compute UV (spherical coordinates)
    let ref_dir = vec3<f32>(params[4], params[5], params[6]);
    let axis = vec3<f32>(params[7], params[8], params[9]);
    let y_dir = cross(axis, ref_dir);

    let to_p = normalize((oc + t * dir) / radius);
    let z = clamp(dot(to_p, axis), -1.0, 1.0);
    let v = asin(z);

    let proj = to_p - z * axis;
    let proj_len = length(proj);
    var u = 0.0;
    if proj_len > EPSILON {
        let x = dot(proj, ref_dir) / proj_len;
        let y = dot(proj, y_dir) / proj_len;
        u = atan2(y, x);
        if u < 0.0 { u += 2.0 * PI; }
    }

    hit.uv = vec2<f32>(u, v);
    return hit;
}

fn intersect_cylinder(origin: vec3<f32>, dir: vec3<f32>, params: array<f32, 32>) -> RayHit {
    var hit: RayHit;
    hit.t = MAX_T;
    hit.face_idx = 0xFFFFFFFFu;

    let center = vec3<f32>(params[0], params[1], params[2]);
    let axis = vec3<f32>(params[3], params[4], params[5]);
    let ref_dir = vec3<f32>(params[6], params[7], params[8]);
    let radius = params[9];

    let oc0 = origin - center;

    // Project onto plane perpendicular to axis
    let d_perp = dir - dot(dir, axis) * axis;

    let a = dot(d_perp, d_perp);
    if a < EPSILON {
        return hit; // Ray parallel to axis
    }

    // Re-origin at the closest approach to the *axis*, not to the centre: a
    // cylinder's centre can sit arbitrarily far along its own axis from the
    // hit, and it is the perpendicular distance the quadratic is about. That
    // point is the quadratic's own vertex.
    let oc0_perp = oc0 - dot(oc0, axis) * axis;
    let t0 = max(-dot(oc0_perp, d_perp) / a, 0.0);
    let oc = oc0 + t0 * dir;
    let oc_perp = oc - dot(oc, axis) * axis;

    let b = 2.0 * dot(oc_perp, d_perp);
    let c = dot(oc_perp, oc_perp) - radius * radius;

    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return hit;
    }

    let sqrt_disc = sqrt(disc);
    var t = (-b - sqrt_disc) / (2.0 * a);
    if t < -t0 {
        t = (-b + sqrt_disc) / (2.0 * a);
    }
    if t < -t0 {
        return hit;
    }

    hit.t = t + t0;

    // Compute UV
    let y_dir = cross(axis, ref_dir);
    let to_p = oc + t * dir;
    let v = dot(to_p, axis);
    let proj = to_p - v * axis;
    let x = dot(proj, ref_dir);
    let y = dot(proj, y_dir);
    var u = atan2(y, x);
    if u < 0.0 { u += 2.0 * PI; }

    hit.uv = vec2<f32>(u, v);
    return hit;
}

fn intersect_cone(origin: vec3<f32>, dir: vec3<f32>, params: array<f32, 32>) -> RayHit {
    var hit: RayHit;
    hit.t = MAX_T;
    hit.face_idx = 0xFFFFFFFFu;

    // Cone parameters: apex (3), axis (3), ref_dir (3), half_angle (1)
    let apex = vec3<f32>(params[0], params[1], params[2]);
    let axis = vec3<f32>(params[3], params[4], params[5]);
    let ref_dir = vec3<f32>(params[6], params[7], params[8]);
    let half_angle = params[9];

    let cos_a = cos(half_angle);
    let cos2 = cos_a * cos_a;

    let co0 = origin - apex;
    let t0 = closest_approach(co0, dir);
    let co = co0 + t0 * dir;
    let d_dot_a = dot(dir, axis);
    let co_dot_a = dot(co, axis);

    // Quadratic coefficients
    let a = d_dot_a * d_dot_a - cos2;
    let b = 2.0 * (d_dot_a * co_dot_a - cos2 * dot(dir, co));
    let c = co_dot_a * co_dot_a - cos2 * dot(co, co);

    if abs(a) < EPSILON {
        // Linear case
        if abs(b) > EPSILON {
            let t = -c / b;
            if t >= -t0 {
                let to_p = co + t * dir;
                let v = dot(to_p, axis) / cos_a;
                if v >= 0.0 {
                    hit.t = t + t0;
                    // Compute UV
                    let y_dir = cross(axis, ref_dir);
                    let height = dot(to_p, axis);
                    let proj = to_p - height * axis;
                    let proj_len = length(proj);
                    var u = 0.0;
                    if proj_len > EPSILON {
                        let x = dot(proj, ref_dir) / proj_len;
                        let y = dot(proj, y_dir) / proj_len;
                        u = atan2(y, x);
                        if u < 0.0 { u += 2.0 * PI; }
                    }
                    hit.uv = vec2<f32>(u, v);
                }
            }
        }
        return hit;
    }

    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return hit;
    }

    let sqrt_disc = sqrt(disc);
    var t1 = (-b - sqrt_disc) / (2.0 * a);
    var t2 = (-b + sqrt_disc) / (2.0 * a);

    // Try both solutions, take the closer valid one
    for (var i = 0; i < 2; i++) {
        let t = select(t2, t1, i == 0);
        if t < -t0 { continue; }

        let to_point = co + t * dir;
        let height_along_axis = dot(to_point, axis);
        let v = height_along_axis / cos_a;

        if v >= 0.0 {
            hit.t = t + t0;
            // Compute UV
            let y_dir = cross(axis, ref_dir);
            let proj = to_point - height_along_axis * axis;
            let proj_len = length(proj);
            var u = 0.0;
            if proj_len > EPSILON {
                let x = dot(proj, ref_dir) / proj_len;
                let y = dot(proj, y_dir) / proj_len;
                u = atan2(y, x);
                if u < 0.0 { u += 2.0 * PI; }
            }
            hit.uv = vec2<f32>(u, v);
            return hit;
        }
    }

    return hit;
}

// Solve cubic: x^3 + px^2 + qx + r = 0 (normalized form)
// Returns up to 3 roots
fn solve_cubic_normalized(p: f32, q: f32, r: f32) -> vec3<f32> {
    // Depressed cubic via substitution x = t - p/3
    let p2 = p * p;
    let aa = q - p2 / 3.0;
    let bb = r - p * q / 3.0 + 2.0 * p2 * p / 27.0;

    let delta = bb * bb / 4.0 + aa * aa * aa / 27.0;
    let shift = p / 3.0;

    if delta > EPSILON {
        // One real root
        let sqrt_delta = sqrt(delta);
        let u = sign(-bb / 2.0 + sqrt_delta) * pow(abs(-bb / 2.0 + sqrt_delta), 1.0 / 3.0);
        let v = sign(-bb / 2.0 - sqrt_delta) * pow(abs(-bb / 2.0 - sqrt_delta), 1.0 / 3.0);
        let root = u + v - shift;
        return vec3<f32>(root, root, root);
    } else if abs(delta) <= EPSILON {
        // Multiple roots
        if abs(aa) < EPSILON && abs(bb) < EPSILON {
            // Triple root
            return vec3<f32>(-shift, -shift, -shift);
        } else {
            // Double root
            let u = sign(-bb / 2.0) * pow(abs(-bb / 2.0), 1.0 / 3.0);
            return vec3<f32>(2.0 * u - shift, -u - shift, -u - shift);
        }
    } else {
        // Three real roots (Vieta's trigonometric solution)
        let m = 2.0 * sqrt(-aa / 3.0);
        // Clamped: rounding can push the cosine a hair outside [-1, 1], and an
        // acos of that is NaN, which propagates silently into every root.
        let theta = acos(clamp(3.0 * bb / (aa * m), -1.0, 1.0)) / 3.0;
        return vec3<f32>(
            m * cos(theta) - shift,
            m * cos(theta - 2.0 * PI / 3.0) - shift,
            m * cos(theta + 2.0 * PI / 3.0) - shift
        );
    }
}

fn intersect_torus(origin: vec3<f32>, dir: vec3<f32>, params: array<f32, 32>) -> RayHit {
    var hit: RayHit;
    hit.t = MAX_T;
    hit.face_idx = 0xFFFFFFFFu;

    // Torus parameters: center (3), axis (3), ref_dir (3), major_radius (1), minor_radius (1)
    let center = vec3<f32>(params[0], params[1], params[2]);
    let axis = vec3<f32>(params[3], params[4], params[5]);
    let ref_dir = vec3<f32>(params[6], params[7], params[8]);
    let R = params[9];  // Major radius
    let r = params[10]; // Minor radius

    let R2 = R * R;
    let r2 = r * r;

    // The one that made this necessary: see `closest_approach`. Ferrari's
    // depressed coefficients are differences of |o|-scale quantities, so a
    // 2 mm tube solved from ten metres away is below the noise floor of f32.
    let o0 = origin - center;
    let t0 = closest_approach(o0, dir);
    let o = o0 + t0 * dir;

    let od = dot(o, dir);
    let oo = dot(o, o);
    let dd = dot(dir, dir);
    let oa = dot(o, axis);
    let da = dot(dir, axis);

    // Quartic coefficients
    let sum_r2_r2 = R2 + r2;
    let k = oo - sum_r2_r2;

    let c4 = dd * dd;
    let c3 = 4.0 * dd * od;
    let c2 = 2.0 * dd * k + 4.0 * od * od + 4.0 * R2 * da * da;
    let c1 = 4.0 * k * od + 8.0 * R2 * oa * da;
    let c0 = k * k - 4.0 * R2 * (r2 - oa * oa);

    // Normalize to monic quartic: t^4 + at^3 + bt^2 + ct + d = 0
    let a_norm = c3 / c4;
    let b_norm = c2 / c4;
    let c_norm = c1 / c4;
    let d_norm = c0 / c4;

    // Depressed quartic via substitution t = y - a/4
    let a2 = a_norm * a_norm;
    let a3 = a2 * a_norm;
    let a4 = a2 * a2;

    let p = b_norm - 3.0 * a2 / 8.0;
    let q = c_norm - a_norm * b_norm / 2.0 + a3 / 8.0;
    let rr = d_norm - a_norm * c_norm / 4.0 + a2 * b_norm / 16.0 - 3.0 * a4 / 256.0;

    // Ferrari's resolvent cubic, 8u^3 + 8p*u^2 + (2p^2 - 8rr)*u - q^2 = 0,
    // divided through by 8 for the monic solver. The reconstruction below
    // reads `u` on this scale — a resolvent solved on any other scale hands
    // back four numbers that satisfy nothing.
    let cubic_roots = solve_cubic_normalized(
        p,
        (p * p - 4.0 * rr) / 4.0,
        -q * q / 8.0
    );

    // Find positive root
    var u = 0.0;
    if cubic_roots.x > EPSILON { u = cubic_roots.x; }
    else if cubic_roots.y > EPSILON { u = cubic_roots.y; }
    else if cubic_roots.z > EPSILON { u = cubic_roots.z; }

    let sqrt_2u = sqrt(max(2.0 * u, 0.0));

    // Two quadratics
    // Local roots run from -t0 (the true ray origin) upwards.
    var best_t = MAX_T;
    var best_uv = vec2<f32>(0.0, 0.0);

    if sqrt_2u > EPSILON {
        // The quartic splits as (y^2 + p/2 + u)^2 - 2u*(y - q/(4u))^2, giving
        // the two quadratics below. Their signs are *not* interchangeable: the
        // `+ beta` constant belongs to the `- sqrt(2u)*y` factor and `- beta`
        // to the `+ sqrt(2u)*y` one. Pairing them the other way still yields
        // four plausible numbers that are not roots — and it stays invisible
        // in an axis-aligned test, where q vanishes and the two pairings agree.
        let alpha = p + 2.0 * u;
        let beta = q / sqrt_2u;

        // First quadratic: y^2 - sqrt_2u*y + (alpha + beta)/2 = 0
        let disc1 = sqrt_2u * sqrt_2u - 2.0 * (alpha + beta);
        if disc1 >= 0.0 {
            let sqrt_disc1 = sqrt(disc1);
            let y1 = (sqrt_2u + sqrt_disc1) / 2.0;
            let y2 = (sqrt_2u - sqrt_disc1) / 2.0;
            let t1 = y1 - a_norm / 4.0;
            let t2 = y2 - a_norm / 4.0;
            if t1 >= -t0 && t1 < best_t { best_t = t1; }
            if t2 >= -t0 && t2 < best_t { best_t = t2; }
        }

        // Second quadratic: y^2 + sqrt_2u*y + (alpha - beta)/2 = 0
        let disc2 = sqrt_2u * sqrt_2u - 2.0 * (alpha - beta);
        if disc2 >= 0.0 {
            let sqrt_disc2 = sqrt(disc2);
            let y3 = (-sqrt_2u + sqrt_disc2) / 2.0;
            let y4 = (-sqrt_2u - sqrt_disc2) / 2.0;
            let t3 = y3 - a_norm / 4.0;
            let t4 = y4 - a_norm / 4.0;
            if t3 >= -t0 && t3 < best_t { best_t = t3; }
            if t4 >= -t0 && t4 < best_t { best_t = t4; }
        }
    } else {
        // Biquadratic case: y^4 + p*y^2 + rr = 0
        let disc = p * p - 4.0 * rr;
        if disc >= 0.0 {
            let sqrt_disc = sqrt(disc);
            let y2_1 = (-p + sqrt_disc) / 2.0;
            let y2_2 = (-p - sqrt_disc) / 2.0;

            if y2_1 >= 0.0 {
                let y = sqrt(y2_1);
                let t1 = y - a_norm / 4.0;
                let t2 = -y - a_norm / 4.0;
                if t1 >= -t0 && t1 < best_t { best_t = t1; }
                if t2 >= -t0 && t2 < best_t { best_t = t2; }
            }
            if y2_2 >= 0.0 {
                let y = sqrt(y2_2);
                let t3 = y - a_norm / 4.0;
                let t4 = -y - a_norm / 4.0;
                if t3 >= -t0 && t3 < best_t { best_t = t3; }
                if t4 >= -t0 && t4 < best_t { best_t = t4; }
            }
        }
    }

    if best_t < MAX_T {
        hit.t = best_t + t0;
        // Compute UV
        let y_dir = cross(axis, ref_dir);
        let to_point = o + best_t * dir;
        let h = dot(to_point, axis);
        let proj = to_point - h * axis;
        let proj_len = length(proj);

        // u = toroidal angle
        var u_angle = 0.0;
        if proj_len > EPSILON {
            let x = dot(proj, ref_dir) / proj_len;
            let y = dot(proj, y_dir) / proj_len;
            u_angle = atan2(y, x);
            if u_angle < 0.0 { u_angle += 2.0 * PI; }
        }

        // v = poloidal angle
        let tube_center_dist = proj_len - R;
        var v_angle = atan2(h, tube_center_dist);
        if v_angle < 0.0 { v_angle += 2.0 * PI; }

        hit.uv = vec2<f32>(u_angle, v_angle);
    }

    return hit;
}

// Möller-Trumbore ray/triangle test.
//
// Ports `kosm_render::intersect_triangle`, the CPU tracer's intersector, with
// two deliberate differences forced by f32:
//
//   * the degeneracy guard is 1e-8 relative rather than 1e-12 — at f32
//     precision 1e-12 is below the noise floor of the determinant itself, so
//     it would admit near-parallel rays whose barycentrics are pure rounding
//     error;
//   * the barycentric slack is 1e-6 rather than 1e-12, for the same reason.
//     Slack matters here: a shared edge must be inclusive from both sides, or
//     a mesh grows a lace of single-pixel holes along every triangle border.
//
// Returns barycentrics in `uv`, which `compute_normal` then interpolates the
// vertex normals with. `hit.uv` is (u, v); the third weight is 1 - u - v.
fn intersect_triangle(origin: vec3<f32>, dir: vec3<f32>, params: array<f32, 32>) -> RayHit {
    var hit: RayHit;
    hit.t = MAX_T;
    hit.face_idx = 0xFFFFFFFFu;

    let v0 = vec3<f32>(params[0], params[1], params[2]);
    let v1 = vec3<f32>(params[3], params[4], params[5]);
    let v2 = vec3<f32>(params[6], params[7], params[8]);

    let e1 = v1 - v0;
    let e2 = v2 - v0;

    let pvec = cross(dir, e2);
    let det = dot(e1, pvec);

    // Size-relative: an absolute epsilon is meaningless when the model might
    // be dimensioned in millimetres or in metres.
    let scale = length(e1) * length(e2);
    if abs(det) <= 1e-8 * max(scale, 1.0) {
        return hit;
    }

    let inv_det = 1.0 / det;
    let tvec = origin - v0;

    let u = dot(tvec, pvec) * inv_det;
    if u < -1e-6 || u > 1.0 + 1e-6 {
        return hit;
    }

    let qvec = cross(tvec, e1);
    let v = dot(dir, qvec) * inv_det;
    if v < -1e-6 || u + v > 1.0 + 1e-6 {
        return hit;
    }

    let t = dot(e2, qvec) * inv_det;
    if t <= 0.0 {
        return hit;
    }

    hit.t = t;
    hit.uv = vec2<f32>(u, v);
    return hit;
}

fn intersect_surface(origin: vec3<f32>, dir: vec3<f32>, surface_idx: u32) -> RayHit {
    let surface = surfaces[surface_idx];

    switch surface.surface_type {
        case SURFACE_PLANE: {
            return intersect_plane(origin, dir, surface.params);
        }
        case SURFACE_SPHERE: {
            return intersect_sphere(origin, dir, surface.params);
        }
        case SURFACE_CYLINDER: {
            return intersect_cylinder(origin, dir, surface.params);
        }
        case SURFACE_CONE: {
            return intersect_cone(origin, dir, surface.params);
        }
        case SURFACE_TORUS: {
            return intersect_torus(origin, dir, surface.params);
        }
        case SURFACE_TRIANGLE: {
            return intersect_triangle(origin, dir, surface.params);
        }
        default: {
            var hit: RayHit;
            hit.t = MAX_T;
            hit.face_idx = 0xFFFFFFFFu;
            return hit;
        }
    }
}

// Compute winding number for a single polygon
fn winding_number_polygon(uv: vec2<f32>, start: u32, count: u32) -> i32 {
    if count < 3u {
        return 0;
    }

    var winding: i32 = 0;

    for (var i = 0u; i < count; i++) {
        let p1 = trim_verts[start + i];
        let p2 = trim_verts[start + ((i + 1u) % count)];

        if p1.y <= uv.y {
            if p2.y > uv.y {
                let cross_val = (p2.x - p1.x) * (uv.y - p1.y) - (uv.x - p1.x) * (p2.y - p1.y);
                if cross_val > 0.0 {
                    winding++;
                }
            }
        } else {
            if p2.y <= uv.y {
                let cross_val = (p2.x - p1.x) * (uv.y - p1.y) - (uv.x - p1.x) * (p2.y - p1.y);
                if cross_val < 0.0 {
                    winding--;
                }
            }
        }
    }

    return winding;
}

// Twice the signed shoelace area of a polygon in UV space.
// Used to detect degenerate (near-zero-area) trim loops.
fn polygon_signed_area(start: u32, count: u32) -> f32 {
    if count < 3u {
        return 0.0;
    }

    var area: f32 = 0.0;
    for (var i = 0u; i < count; i++) {
        let p1 = trim_verts[start + i];
        let p2 = trim_verts[start + ((i + 1u) % count)];
        area += p1.x * p2.y - p2.x * p1.y;
    }

    return area;
}

// Simple AABB check for outer loop (for debugging)
fn uv_in_trim_bounds(uv: vec2<f32>, start: u32, count: u32) -> bool {
    if count == 0u {
        return false;
    }

    var min_uv = trim_verts[start];
    var max_uv = trim_verts[start];

    for (var i = 1u; i < count; i++) {
        let v = trim_verts[start + i];
        min_uv = min(min_uv, v);
        max_uv = max(max_uv, v);
    }

    // Add small epsilon for numerical tolerance
    let eps = 0.001;
    return uv.x >= min_uv.x - eps && uv.x <= max_uv.x + eps &&
           uv.y >= min_uv.y - eps && uv.y <= max_uv.y + eps;
}

// Point-in-polygon test with inner loops (holes)
fn point_in_face(uv: vec2<f32>, face_idx: u32) -> bool {
    let face = faces[face_idx];

    // A triangle carries no trim loops: `intersect_triangle` already answered
    // the containment question that trimming answers for an analytic surface,
    // so a mesh hit is in-face by construction. Without this bypass the
    // winding test below would reject every mesh hit (trim_count is 0).
    if surfaces[face.surface_idx].surface_type == SURFACE_TRIANGLE {
        return true;
    }

    // Check outer loop - point must be inside
    if face.trim_count < 3u {
        // For faces with < 3 trim vertices (e.g., full cylinder walls),
        // the 2 vertices define a v-range (height bounds).
        // The u-coordinate wraps around 0 to 2π.
        if face.trim_count == 2u {
            let v1 = trim_verts[face.trim_start];
            let v2 = trim_verts[face.trim_start + 1u];
            let v_min = min(v1.y, v2.y);
            let v_max = max(v1.y, v2.y);
            // Check v is in range (u is assumed valid for full wrap-around)
            return uv.y >= v_min && uv.y <= v_max;
        }
        // For 0 or 1 vertices, reject
        return false;
    }

    // A primitive sphere (or full torus) face is bounded only by its seam,
    // which projects to a zero-area UV polygon. Treat such a degenerate outer
    // loop as untrimmed so every ray hit isn't spuriously rejected — inner
    // loops (holes) are still honored below.
    let outer_area = polygon_signed_area(face.trim_start, face.trim_count);
    if abs(outer_area) >= 1e-9 {
        // Quick AABB rejection before expensive winding number test
        if !uv_in_trim_bounds(uv, face.trim_start, face.trim_count) {
            return false;
        }

        // Winding number test for proper polygon boundary
        let outer_winding = winding_number_polygon(uv, face.trim_start, face.trim_count);
        if outer_winding == 0 {
            return false; // Outside outer boundary
        }
    }

    // Check inner loops (holes) - point must be outside all holes
    if face.inner_loop_count > 0u {
        var inner_offset = face.inner_start;
        for (var loop_idx = 0u; loop_idx < face.inner_loop_count; loop_idx++) {
            let loop_size = inner_loop_descs[face.inner_desc_start + loop_idx];
            if loop_size >= 3u {
                let inner_winding = winding_number_polygon(uv, inner_offset, loop_size);
                if inner_winding != 0 {
                    return false; // Inside a hole
                }
            }
            inner_offset += loop_size;
        }
    }

    return true;
}

// Debug: trace with bounds checking but without BVH
fn trace_debug(origin: vec3<f32>, dir: vec3<f32>) -> RayHit {
    var best_hit: RayHit;
    best_hit.t = MAX_T;
    best_hit.face_idx = 0xFFFFFFFFu;

    let num_faces = arrayLength(&faces);
    for (var i = 0u; i < num_faces; i++) {
        let face = faces[i];
        let hit = intersect_surface(origin, dir, face.surface_idx);
        if hit.t > EPSILON && hit.t < best_hit.t {
            // Apply bounds checking to reject hits outside face boundary
            if point_in_face(hit.uv, i) {
                best_hit = hit;
                best_hit.face_idx = i;
            }
        }
    }

    return best_hit;
}

// BVH traversal
fn trace_bvh(origin: vec3<f32>, dir: vec3<f32>) -> RayHit {
    var best_hit: RayHit;
    best_hit.t = MAX_T;
    best_hit.face_idx = 0xFFFFFFFFu;

    let inv_dir = 1.0 / dir;
    // Reject anything the origin's own float grid cannot separate from the
    // origin: at millimetre scale `t > 0.0` alone re-finds the surface the
    // ray just left.
    let t_floor = ray_eps(origin);

    // Stack-based traversal
    var stack: array<u32, 32>;
    var stack_ptr = 0;
    stack[0] = 0u; // Root node
    stack_ptr = 1;

    while stack_ptr > 0 {
        stack_ptr--;
        let node_idx = stack[stack_ptr];
        let node = bvh_nodes[node_idx];

        // Test AABB
        let t_range = intersect_aabb(origin, inv_dir, node.aabb_min.xyz, node.aabb_max.xyz);
        if t_range.y < 0.0 || t_range.x > t_range.y || t_range.x > best_hit.t {
            continue;
        }

        if node.is_leaf == 1u {
            // Leaf node: test faces
            for (var i = 0u; i < node.right_or_count; i++) {
                let face_idx = node.left_or_first + i;
                let face = faces[face_idx];

                let hit = intersect_surface(origin, dir, face.surface_idx);
                if hit.t < best_hit.t && hit.t > t_floor {
                    // Use proper UV-based point-in-polygon test
                    if point_in_face(hit.uv, face_idx) {
                        best_hit = hit;
                        best_hit.face_idx = face_idx;
                    }
                }
            }
        } else {
            // Internal node: push children
            if stack_ptr < 31 {
                stack[stack_ptr] = node.left_or_first;
                stack_ptr++;
            }
            if stack_ptr < 31 {
                stack[stack_ptr] = node.right_or_count;
                stack_ptr++;
            }
        }
    }

    return best_hit;
}

// Sentinel face_idx values used by the shader to tag non-BRep hits.

// Compute surface normal at hit point
fn compute_normal(hit: RayHit) -> vec3<f32> {
    let face = faces[hit.face_idx];
    let surface = surfaces[face.surface_idx];

    var normal: vec3<f32>;

    switch surface.surface_type {
        case SURFACE_TRIANGLE: {
            let v0 = vec3<f32>(surface.params[0], surface.params[1], surface.params[2]);
            let v1 = vec3<f32>(surface.params[3], surface.params[4], surface.params[5]);
            let v2 = vec3<f32>(surface.params[6], surface.params[7], surface.params[8]);

            // Geometric normal. Non-zero by construction: `Bvh::build_mesh`
            // drops zero-area triangles, so this is always a usable fallback.
            let geometric = cross(v1 - v0, v2 - v0);

            // Smooth shading: barycentric blend of the vertex normals, so a
            // mesh part doesn't read as faceted next to an analytic one.
            // Mirrors `TriMesh::intersect` on the CPU, including both of its
            // fallbacks — no normal array (params[18] == 0), and a blend that
            // cancels (opposed vertex normals across a degenerate crease).
            normal = geometric;
            if surface.params[18] > 0.5 {
                let n0 = vec3<f32>(surface.params[9], surface.params[10], surface.params[11]);
                let n1 = vec3<f32>(surface.params[12], surface.params[13], surface.params[14]);
                let n2 = vec3<f32>(surface.params[15], surface.params[16], surface.params[17]);
                let u = hit.uv.x;
                let v = hit.uv.y;
                let blended = n0 * (1.0 - u - v) + n1 * u + n2 * v;
                if length(blended) > 1e-6 {
                    normal = blended;
                }
            }
        }
        case SURFACE_PLANE: {
            normal = vec3<f32>(surface.params[9], surface.params[10], surface.params[11]);
        }
        case SURFACE_SPHERE: {
            let center = vec3<f32>(surface.params[0], surface.params[1], surface.params[2]);
            let ref_dir = vec3<f32>(surface.params[4], surface.params[5], surface.params[6]);
            let axis = vec3<f32>(surface.params[7], surface.params[8], surface.params[9]);
            let y_dir = cross(axis, ref_dir);

            let u = hit.uv.x;
            let v = hit.uv.y;
            let cos_v = cos(v);
            let sin_v = sin(v);
            let cos_u = cos(u);
            let sin_u = sin(u);

            normal = cos_v * (cos_u * ref_dir + sin_u * y_dir) + sin_v * axis;
        }
        case SURFACE_CYLINDER: {
            let ref_dir = vec3<f32>(surface.params[6], surface.params[7], surface.params[8]);
            let axis = vec3<f32>(surface.params[3], surface.params[4], surface.params[5]);
            let y_dir = cross(axis, ref_dir);

            let u = hit.uv.x;
            let cos_u = cos(u);
            let sin_u = sin(u);

            normal = cos_u * ref_dir + sin_u * y_dir;
        }
        case SURFACE_CONE: {
            // Cone: apex (3), axis (3), ref_dir (3), half_angle (1)
            let axis = vec3<f32>(surface.params[3], surface.params[4], surface.params[5]);
            let ref_dir = vec3<f32>(surface.params[6], surface.params[7], surface.params[8]);
            let half_angle = surface.params[9];
            let y_dir = cross(axis, ref_dir);

            let u = hit.uv.x;
            let cos_u = cos(u);
            let sin_u = sin(u);
            let cos_a = cos(half_angle);
            let sin_a = sin(half_angle);

            // Normal points outward from cone surface
            // Radial direction at angle u
            let radial = cos_u * ref_dir + sin_u * y_dir;
            // Normal = radial * cos(half_angle) - axis * sin(half_angle)
            normal = radial * cos_a - axis * sin_a;
        }
        case SURFACE_TORUS: {
            // Torus: center (3), axis (3), ref_dir (3), R (1), r (1)
            let center = vec3<f32>(surface.params[0], surface.params[1], surface.params[2]);
            let axis = vec3<f32>(surface.params[3], surface.params[4], surface.params[5]);
            let ref_dir = vec3<f32>(surface.params[6], surface.params[7], surface.params[8]);
            let R = surface.params[9];
            let y_dir = cross(axis, ref_dir);

            let u = hit.uv.x;
            let v = hit.uv.y;
            let cos_u = cos(u);
            let sin_u = sin(u);
            let cos_v = cos(v);
            let sin_v = sin(v);

            // Direction from center to tube center at angle u
            let tube_dir = cos_u * ref_dir + sin_u * y_dir;
            // Normal at poloidal angle v
            normal = tube_dir * cos_v + axis * sin_v;
        }
        default: {
            normal = vec3<f32>(0.0, 0.0, 1.0);
        }
    }

    // Apply face orientation
    if face.orientation == 1u {
        normal = -normal;
    }

    return normalize(normal);
}


// Surface tangent dP/du at a hit, or a zero vector where the parameterisation
// is degenerate. Thin wrapper: the maths lives in `surface_dpdu` (surface.wgsl)
// so it can be driven directly by the parity harness.
fn compute_tangent(hit: RayHit) -> vec3<f32> {
    let face = faces[hit.face_idx];
    let surface = surfaces[face.surface_idx];
    return surface_dpdu(surface.surface_type, surface.params, hit.uv);
}

// ─── the rest of the contract ─────────────────────────────────────────────

// The renderer's names for the four accessors it calls on a hit.
fn trace_scene(origin: vec3<f32>, dir: vec3<f32>) -> RayHit {
    return trace_bvh(origin, dir);
}

fn hit_normal(hit: RayHit) -> vec3<f32> {
    return compute_normal(hit);
}

fn hit_tangent(hit: RayHit) -> vec3<f32> {
    return compute_tangent(hit);
}

// Index into the renderer's `materials` binding for the face that was hit.
fn hit_material_index(hit: RayHit) -> u32 {
    return faces[hit.face_idx].material_idx;
}

// 0 = forward, 1 = reversed. Only the orientation debug mode reads it.
fn hit_orientation(hit: RayHit) -> u32 {
    return faces[hit.face_idx].orientation;
}
