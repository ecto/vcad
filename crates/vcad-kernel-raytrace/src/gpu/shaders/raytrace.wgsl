// Ray tracing compute shader for direct BRep rendering.
//
// This shader traces rays against analytic surfaces without tessellation,
// achieving pixel-perfect silhouettes at any zoom level.

// Constants
const SURFACE_PLANE: u32 = 0u;
const SURFACE_CYLINDER: u32 = 1u;
const SURFACE_SPHERE: u32 = 2u;
const SURFACE_CONE: u32 = 3u;
const SURFACE_TORUS: u32 = 4u;
const SURFACE_BILINEAR: u32 = 5u;

const MAX_T: f32 = 1e10;
const EPSILON: f32 = 1e-6;
const PI: f32 = 3.14159265359;

// Structures matching Rust definitions

struct GpuSurface {
    surface_type: u32,
    // Use explicit u32 padding instead of vec3<u32> to match Rust layout
    // vec3<u32> in WGSL has 16-byte alignment which would misalign params
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    params: array<f32, 32>,
}

struct GpuMaterial {
    color: vec4<f32>,
    metallic: f32,
    roughness: f32,
    _pad: vec2<f32>,
}

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

struct Camera {
    position: vec4<f32>,
    look_at: vec4<f32>,
    up: vec4<f32>,
    fov: f32,
    width: u32,
    height: u32,
    _pad: u32,
}

struct RenderState {
    frame_index: u32,
    jitter_x: f32,
    jitter_y: f32,
    // Edge bit-flags: bit0=silhouette, bit1=crease, bit2=boundary. 0 = edges off.
    enable_edges: u32,
    edge_depth_threshold: f32,
    edge_normal_threshold: f32,
    // 0=normal, 1=normals RGB, 2=face_id, 3=n_dot_l, 4=orientation, 5=sample-count heatmap
    debug_mode: u32,
    // 0=dark, 1=light
    theme: u32,
    // Additional rays per edge pixel for adaptive refinement (0 = disabled).
    refine_sample_count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    // Edge style — layout must match GpuRenderState in buffers.rs
    silhouette_color: vec4<f32>,
    crease_color: vec4<f32>,
    boundary_color: vec4<f32>,
    silhouette_width: f32,
    crease_width: f32,
    boundary_width: f32,
    edge_softness: f32,
}

struct RayHit {
    t: f32,
    face_idx: u32,
    uv: vec2<f32>,
}

// Bind groups

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<storage, read> surfaces: array<GpuSurface>;
@group(0) @binding(2) var<storage, read> faces: array<GpuFace>;
@group(0) @binding(3) var<storage, read> bvh_nodes: array<GpuBvhNode>;
@group(0) @binding(4) var<storage, read> trim_verts: array<vec2<f32>>;
@group(0) @binding(5) var<storage, read> inner_loop_descs: array<u32>;
@group(0) @binding(6) var output: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(7) var<uniform> render_state: RenderState;
@group(0) @binding(8) var<storage, read_write> accum_buffer: array<vec4<f32>>;
@group(0) @binding(9) var<storage, read> materials: array<GpuMaterial>;
@group(0) @binding(10) var<storage, read_write> depth_normal_buffer: array<vec4<f32>>;
// Per-pixel face_idx for analytic crease detection (0xFFFFFFFF = background).
// Written at frame 1; read at frame 2+ by detect_edge_sobel.
@group(0) @binding(11) var<storage, read_write> feature_id_buffer: array<u32>;

// Helper functions for buffer indexing (2D coords to 1D index)
fn pixel_index(coord: vec2<u32>) -> u32 {
    return coord.y * camera.width + coord.x;
}

fn pixel_index_i32(coord: vec2<i32>) -> u32 {
    return u32(coord.y) * camera.width + u32(coord.x);
}

// Utility functions

// Core ray generation with an explicit sub-pixel offset.
// offset is in pixels, typically in [-0.5, 0.5].
fn ray_origin_and_direction_offset(pixel: vec2<u32>, offset: vec2<f32>) -> mat2x3<f32> {
    let aspect = f32(camera.width) / f32(camera.height);
    let fov_tan = tan(camera.fov * 0.5);

    // Compute normalized device coordinates with the given offset
    let ndc = vec2<f32>(
        (f32(pixel.x) + 0.5 + offset.x) / f32(camera.width) * 2.0 - 1.0,
        1.0 - (f32(pixel.y) + 0.5 + offset.y) / f32(camera.height) * 2.0
    );

    // Build camera coordinate system
    let forward = normalize(camera.look_at.xyz - camera.position.xyz);
    let right = normalize(cross(forward, camera.up.xyz));
    let up = cross(right, forward);

    // Compute ray direction
    let dir = normalize(
        forward +
        right * ndc.x * fov_tan * aspect +
        up * ndc.y * fov_tan
    );

    return mat2x3<f32>(camera.position.xyz, dir);
}

// Ray generation using the Halton-sequence jitter from render_state (main pass).
fn ray_origin_and_direction(pixel: vec2<u32>) -> mat2x3<f32> {
    let jitter = vec2<f32>(render_state.jitter_x, render_state.jitter_y);
    return ray_origin_and_direction_offset(pixel, jitter);
}

fn intersect_aabb(origin: vec3<f32>, inv_dir: vec3<f32>, aabb_min: vec3<f32>, aabb_max: vec3<f32>) -> vec2<f32> {
    let t1 = (aabb_min - origin) * inv_dir;
    let t2 = (aabb_max - origin) * inv_dir;

    let t_min = min(t1, t2);
    let t_max = max(t1, t2);

    let t_enter = max(max(t_min.x, t_min.y), t_min.z);
    let t_exit = min(min(t_max.x, t_max.y), t_max.z);

    return vec2<f32>(t_enter, t_exit);
}

// Ray-surface intersection functions

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

    let t = dot(plane_origin - origin, plane_normal) / denom;
    if t < 0.0 {
        return hit;
    }

    hit.t = t;

    // Compute UV
    let p = origin + t * dir;
    let x_dir = vec3<f32>(params[3], params[4], params[5]);
    let y_dir = vec3<f32>(params[6], params[7], params[8]);
    let to_p = p - plane_origin;
    hit.uv = vec2<f32>(dot(to_p, x_dir), dot(to_p, y_dir));

    return hit;
}

fn intersect_sphere(origin: vec3<f32>, dir: vec3<f32>, params: array<f32, 32>) -> RayHit {
    var hit: RayHit;
    hit.t = MAX_T;
    hit.face_idx = 0xFFFFFFFFu;

    let center = vec3<f32>(params[0], params[1], params[2]);
    let radius = params[3];

    let oc = origin - center;
    let a = dot(dir, dir);
    let b = 2.0 * dot(oc, dir);
    let c = dot(oc, oc) - radius * radius;

    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return hit;
    }

    let sqrt_disc = sqrt(disc);
    var t = (-b - sqrt_disc) / (2.0 * a);
    if t < 0.0 {
        t = (-b + sqrt_disc) / (2.0 * a);
    }
    if t < 0.0 {
        return hit;
    }

    hit.t = t;

    // Compute UV (spherical coordinates)
    let p = origin + t * dir;
    let ref_dir = vec3<f32>(params[4], params[5], params[6]);
    let axis = vec3<f32>(params[7], params[8], params[9]);
    let y_dir = cross(axis, ref_dir);

    let to_p = normalize((p - center) / radius);
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

    let oc = origin - center;

    // Project onto plane perpendicular to axis
    let d_perp = dir - dot(dir, axis) * axis;
    let oc_perp = oc - dot(oc, axis) * axis;

    let a = dot(d_perp, d_perp);
    if a < EPSILON {
        return hit; // Ray parallel to axis
    }

    let b = 2.0 * dot(oc_perp, d_perp);
    let c = dot(oc_perp, oc_perp) - radius * radius;

    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return hit;
    }

    let sqrt_disc = sqrt(disc);
    var t = (-b - sqrt_disc) / (2.0 * a);
    if t < 0.0 {
        t = (-b + sqrt_disc) / (2.0 * a);
    }
    if t < 0.0 {
        return hit;
    }

    hit.t = t;

    // Compute UV
    let p = origin + t * dir;
    let y_dir = cross(axis, ref_dir);
    let to_p = p - center;
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

    let co = origin - apex;
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
            if t >= 0.0 {
                let point = origin + t * dir;
                let v = dot(point - apex, axis) / cos_a;
                if v >= 0.0 {
                    hit.t = t;
                    // Compute UV
                    let y_dir = cross(axis, ref_dir);
                    let to_p = point - apex;
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
        if t < 0.0 { continue; }

        let point = origin + t * dir;
        let to_point = point - apex;
        let height_along_axis = dot(to_point, axis);
        let v = height_along_axis / cos_a;

        if v >= 0.0 {
            hit.t = t;
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
        let theta = acos(3.0 * bb / (aa * m)) / 3.0;
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

    let o = origin - center;
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

    // Solve resolvent cubic: u^3 + (p/2)*u^2 + ((p^2 - 4*rr)/16)*u - q^2/64 = 0
    let cubic_roots = solve_cubic_normalized(
        p / 2.0,
        (p * p - 4.0 * rr) / 16.0,
        -q * q / 64.0
    );

    // Find positive root
    var u = 0.0;
    if cubic_roots.x > EPSILON { u = cubic_roots.x; }
    else if cubic_roots.y > EPSILON { u = cubic_roots.y; }
    else if cubic_roots.z > EPSILON { u = cubic_roots.z; }

    let sqrt_2u = sqrt(max(2.0 * u, 0.0));

    // Two quadratics
    var best_t = MAX_T;
    var best_uv = vec2<f32>(0.0, 0.0);

    if sqrt_2u > EPSILON {
        let alpha = p + 2.0 * u;
        let beta = q / sqrt_2u;

        // First quadratic: y^2 + sqrt_2u*y + (alpha + beta)/2 = 0
        let disc1 = sqrt_2u * sqrt_2u - 2.0 * (alpha + beta);
        if disc1 >= 0.0 {
            let sqrt_disc1 = sqrt(disc1);
            let y1 = (-sqrt_2u + sqrt_disc1) / 2.0;
            let y2 = (-sqrt_2u - sqrt_disc1) / 2.0;
            let t1 = y1 - a_norm / 4.0;
            let t2 = y2 - a_norm / 4.0;
            if t1 >= 0.0 && t1 < best_t { best_t = t1; }
            if t2 >= 0.0 && t2 < best_t { best_t = t2; }
        }

        // Second quadratic: y^2 - sqrt_2u*y + (alpha - beta)/2 = 0
        let disc2 = sqrt_2u * sqrt_2u - 2.0 * (alpha - beta);
        if disc2 >= 0.0 {
            let sqrt_disc2 = sqrt(disc2);
            let y3 = (sqrt_2u + sqrt_disc2) / 2.0;
            let y4 = (sqrt_2u - sqrt_disc2) / 2.0;
            let t3 = y3 - a_norm / 4.0;
            let t4 = y4 - a_norm / 4.0;
            if t3 >= 0.0 && t3 < best_t { best_t = t3; }
            if t4 >= 0.0 && t4 < best_t { best_t = t4; }
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
                if t1 >= 0.0 && t1 < best_t { best_t = t1; }
                if t2 >= 0.0 && t2 < best_t { best_t = t2; }
            }
            if y2_2 >= 0.0 {
                let y = sqrt(y2_2);
                let t3 = y - a_norm / 4.0;
                let t4 = -y - a_norm / 4.0;
                if t3 >= 0.0 && t3 < best_t { best_t = t3; }
                if t4 >= 0.0 && t4 < best_t { best_t = t4; }
            }
        }
    }

    if best_t < MAX_T {
        hit.t = best_t;
        // Compute UV
        let point = origin + best_t * dir;
        let y_dir = cross(axis, ref_dir);
        let to_point = point - center;
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

    // Quick AABB rejection before expensive winding number test
    if !uv_in_trim_bounds(uv, face.trim_start, face.trim_count) {
        return false;
    }

    // Winding number test for proper polygon boundary
    let outer_winding = winding_number_polygon(uv, face.trim_start, face.trim_count);
    if outer_winding == 0 {
        return false; // Outside outer boundary
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
                if hit.t < best_hit.t && hit.t > 0.0 {
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
// 0xFFFFFFFFu = ray miss (background), 0xFFFFFFFEu = ground plane hit.
const FACE_IDX_GROUND: u32 = 0xFFFFFFFEu;

// Procedural HDR environment in Z-up world space. Returns radiance for a
// given ray direction — used both for ray misses (visible background) and
// for ambient + IBL specular sampling.
//
// Modeled as a "studio environment": dim atmospheric backdrop plus a few
// high-luminance soft panels at fixed directions (key, fill, rim, top
// fill). The panels carry HDR values (luminance > 1) so reflections on
// metals get hot specular highlights that ACES rolls off into clean
// blown-white spots — the same look you get sampling a real HDRI.
//
// Stays in shader code rather than uploading a baked HDR texture; the
// binding plumbing isn't worth it for a single environment, and tuning
// in WGSL is faster than re-baking an exr.
fn sky_color(dir: vec3<f32>) -> vec3<f32> {
    let z = dir.z;

    // Atmospheric backdrop. Two palettes — dark and light — selected by
    // render_state.theme. The IBL panels below stay the same in both so
    // the model's lighting is theme-independent.
    var zenith: vec3<f32>;
    var horizon: vec3<f32>;
    var below: vec3<f32>;
    if render_state.theme == 1u {
        // Light theme — bright neutral with the faintest cool tint at the
        // top so it doesn't read as flat paper-white.
        zenith = vec3<f32>(0.93, 0.95, 1.00);
        horizon = vec3<f32>(0.96, 0.97, 0.99);
        below = vec3<f32>(0.86, 0.86, 0.88);
    } else {
        // Dark theme — moody studio backdrop, cool blues fading into a
        // dim "below horizon" band.
        zenith = vec3<f32>(0.35, 0.55, 0.95);
        horizon = vec3<f32>(0.78, 0.84, 0.92);
        below = vec3<f32>(0.18, 0.18, 0.20);
    }

    var col: vec3<f32>;
    if z >= 0.0 {
        col = mix(horizon, zenith, smoothstep(0.0, 0.55, z));
    } else {
        col = mix(horizon, below, smoothstep(0.0, -0.4, z));
    }

    // Helper to add a soft directional panel. `tightness` controls disc
    // size (higher = tighter, more sun-like; lower = broader, softer
    // panel). HDR-valued so reflections on shiny surfaces blow out
    // through ACES into clean specular highlights.
    // Using inline pow() since WGSL has no closures or generics.

    // Primary key — sun. Tight & hot.
    let sun = sun_direction();
    let sun_dot = dot(dir, sun);
    let sun_disk = smoothstep(0.998, 0.9995, sun_dot) * 35.0;
    let sun_glow = pow(max(sun_dot, 0.0), 96.0) * 1.0;
    col += vec3<f32>(1.00, 0.94, 0.82) * (sun_disk + sun_glow);

    // Warm fill from upper-front-right. Broader, lower luminance —
    // simulates a softbox.
    let fill = normalize(vec3<f32>(0.55, -0.3, 0.7));
    let fill_dot = max(dot(dir, fill), 0.0);
    col += vec3<f32>(1.00, 0.88, 0.72) * pow(fill_dot, 18.0) * 6.0;

    // Cool rim from behind/below — gives metals a clean blue-tinged
    // back-rim highlight.
    let rim = normalize(vec3<f32>(0.2, 0.85, -0.15));
    let rim_dot = max(dot(dir, rim), 0.0);
    col += vec3<f32>(0.55, 0.72, 1.00) * pow(rim_dot, 28.0) * 4.5;

    // Top diffuse panel — broad cool light from straight up. Acts like
    // a studio ceiling and provides the dominant ambient term for the
    // tops of objects.
    let top = vec3<f32>(0.0, 0.0, 1.0);
    let top_dot = max(dot(dir, top), 0.0);
    col += vec3<f32>(0.95, 0.97, 1.00) * pow(top_dot, 6.0) * 1.8;

    return col;
}

// Primary key-light direction in kernel (Z-up) space. Upper-back-left so
// the camera, which typically sits in the +x/-y/+z octant, sees a clear
// lit/shadow split rather than backlight.
fn sun_direction() -> vec3<f32> {
    return normalize(vec3<f32>(-0.35, 0.55, 0.75));
}

// Implicit ground plane at z=0 (kernel space). The plane is always full
// opacity — fade-to-sky is applied by `shade_ground` based on horizontal
// distance from the world origin (where models typically sit), so that
// the model stays grounded regardless of camera distance.
struct GroundHit {
    t: f32,
    point: vec3<f32>,
    fade: f32,
}

fn intersect_ground(origin: vec3<f32>, dir: vec3<f32>) -> GroundHit {
    var hit: GroundHit;
    hit.t = MAX_T;
    hit.fade = 0.0;

    if abs(dir.z) < EPSILON {
        return hit;
    }
    let t = -origin.z / dir.z;
    if t < 0.001 {
        return hit;
    }
    let p = origin + dir * t;

    // fade is computed against the world origin (where the model usually
    // sits) so the ground reads as "platform under the model" rather than
    // "puddle around the camera".
    let horizontal_from_origin = length(p.xy);
    let fade = 1.0 - smoothstep(100.0, 1500.0, horizontal_from_origin);
    if fade <= 0.0 {
        return hit;
    }

    hit.t = t;
    hit.point = p;
    hit.fade = fade;
    return hit;
}

// Cheap shadow test: reuse trace_bvh and check if the closest hit lies
// before the light. WGSL doesn't allow recursion or function pointers, so
// this is the simplest correct path; an "any-hit" early-exit traversal
// would be ~30% faster but isn't needed for current scene complexity.
fn in_shadow(p: vec3<f32>, light_dir: vec3<f32>, max_t: f32) -> bool {
    let bias = 0.0015;
    let origin = p + light_dir * bias;
    let hit = trace_bvh(origin, light_dir);
    return hit.face_idx != 0xFFFFFFFFu && hit.t < max_t;
}

// PCG hash → uniform [0, 1) noise. Per-pixel + per-frame seed so the noise
// decorrelates across pixels (prevents banding) and animates per frame
// (so progressive accumulation averages out).
fn rand_uniform(pixel: vec2<u32>, sample_idx: u32) -> f32 {
    var state = pixel.x * 1973u + pixel.y * 9277u + sample_idx * 26699u + render_state.frame_index * 12345u + 1u;
    state = state * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    let r = (word >> 22u) ^ word;
    return f32(r) / 4294967296.0;
}

fn rand_uniform2(pixel: vec2<u32>, sample_idx: u32) -> vec2<f32> {
    return vec2<f32>(rand_uniform(pixel, sample_idx * 2u), rand_uniform(pixel, sample_idx * 2u + 1u));
}

// Build a tangent frame around a normal so we can sample directions in its
// hemisphere. Choose a stable tangent reference axis based on the normal's
// dominant component.
fn build_tangent_frame(normal: vec3<f32>) -> mat3x3<f32> {
    let up_ref = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(normal.y) > 0.95);
    let tangent = normalize(cross(up_ref, normal));
    let bitangent = cross(normal, tangent);
    return mat3x3<f32>(tangent, bitangent, normal);
}

// Cosine-weighted hemisphere sample around `normal`. Uses Malley's method
// (sample disc, project to hemisphere) for cosine weighting.
fn sample_hemisphere_cosine(normal: vec3<f32>, u: vec2<f32>) -> vec3<f32> {
    let r = sqrt(u.x);
    let theta = 2.0 * PI * u.y;
    let local = vec3<f32>(r * cos(theta), r * sin(theta), sqrt(max(0.0, 1.0 - u.x)));
    return build_tangent_frame(normal) * local;
}

// Jitter a direction within a small cone for soft area-light shadows.
// `cone_radius` is the tangent of the cone half-angle (small ≈ small angle).
fn jitter_direction(dir: vec3<f32>, cone_radius: f32, u: vec2<f32>) -> vec3<f32> {
    let angle = 2.0 * PI * u.x;
    let r = cone_radius * sqrt(u.y);
    let offset = vec3<f32>(cos(angle) * r, sin(angle) * r, 0.0);
    return normalize(build_tangent_frame(dir) * vec3<f32>(offset.x, offset.y, 1.0));
}

// Ambient occlusion via a single short hemisphere ray per pixel. Accumulation
// across frames averages out the noise — at 1 sample per frame and ~8-16
// frames at HIGH quality, the result settles into clean soft contact
// shadows.
fn ambient_occlusion(p: vec3<f32>, normal: vec3<f32>, pixel: vec2<u32>) -> f32 {
    let bias = 0.001;
    let max_dist = 6.0;
    let u = rand_uniform2(pixel, 7u);
    let dir = sample_hemisphere_cosine(normal, u);
    let origin = p + normal * bias;
    let hit = trace_bvh(origin, dir);
    if hit.face_idx == 0xFFFFFFFFu || hit.t > max_dist {
        return 1.0;
    }
    // Linear falloff with hit distance — close hits darken more than far hits.
    return clamp(hit.t / max_dist, 0.0, 1.0);
}

// Compute surface normal at hit point
fn compute_normal(hit: RayHit) -> vec3<f32> {
    let face = faces[hit.face_idx];
    let surface = surfaces[face.surface_idx];

    var normal: vec3<f32>;

    switch surface.surface_type {
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

// Fresnel-Schlick approximation
fn fresnel_schlick(cos_theta: f32, f0: vec3<f32>) -> vec3<f32> {
    return f0 + (1.0 - f0) * pow(1.0 - cos_theta, 5.0);
}

// GGX/Trowbridge-Reitz normal distribution
fn distribution_ggx(n_dot_h: f32, roughness: f32) -> f32 {
    let a = roughness * roughness;
    let a2 = a * a;
    let denom = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / (PI * denom * denom);
}

// Smith geometry function (GGX)
fn geometry_smith(n_dot_v: f32, n_dot_l: f32, roughness: f32) -> f32 {
    let r = roughness + 1.0;
    let k = (r * r) / 8.0;
    let ggx_v = n_dot_v / (n_dot_v * (1.0 - k) + k);
    let ggx_l = n_dot_l / (n_dot_l * (1.0 - k) + k);
    return ggx_v * ggx_l;
}

// ACES Narkowicz tonemap. Cleaner highlights and richer mids than Reinhard.
fn tonemap_aces(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

// Evaluate Cook-Torrance BRDF for one light direction. Returns the
// radiance contribution (already weighted by n_dot_l).
fn brdf_direct(
    normal: vec3<f32>,
    view_dir: vec3<f32>,
    light_dir: vec3<f32>,
    albedo: vec3<f32>,
    metallic: f32,
    roughness: f32,
    f0: vec3<f32>,
) -> vec3<f32> {
    let n_dot_l = max(dot(normal, light_dir), 0.0);
    if n_dot_l <= 0.0 {
        return vec3<f32>(0.0);
    }
    let halfway = normalize(view_dir + light_dir);
    let n_dot_v = max(dot(normal, view_dir), 0.001);
    let n_dot_h = max(dot(normal, halfway), 0.0);
    let h_dot_v = max(dot(halfway, view_dir), 0.0);

    let d = distribution_ggx(n_dot_h, roughness);
    let g = geometry_smith(n_dot_v, n_dot_l, roughness);
    let f = fresnel_schlick(h_dot_v, f0);

    let specular = (d * g * f) / (4.0 * n_dot_v * n_dot_l + 0.001);
    let kd = (1.0 - f) * (1.0 - metallic);
    let diffuse = kd * albedo / PI;
    return (diffuse + specular) * n_dot_l;
}

// Shade the implicit ground plane. Warm-tinted matte so models read as
// sitting on a platform rather than floating in sky. Casts soft contact
// shadows (jittered shadow ray accumulating across frames) and gets AO
// for proper grounding under the model.
fn shade_ground(p: vec3<f32>, dir: vec3<f32>, fade: f32, pixel: vec2<u32>) -> vec4<f32> {
    let normal = vec3<f32>(0.0, 0.0, 1.0);
    let view_dir = -dir;
    // Ground albedo follows theme: warm gray on dark, near-white on light.
    var albedo: vec3<f32>;
    var ambient_factor: vec3<f32>;
    var fade_target: vec3<f32>;
    if render_state.theme == 1u {
        albedo = vec3<f32>(0.78, 0.78, 0.79);
        ambient_factor = vec3<f32>(0.55, 0.55, 0.57);
        fade_target = vec3<f32>(0.96, 0.97, 0.99);
    } else {
        albedo = vec3<f32>(0.22, 0.21, 0.20);
        ambient_factor = vec3<f32>(0.22, 0.22, 0.23);
        fade_target = vec3<f32>(0.80, 0.85, 0.92);
    }
    let roughness = 0.92;
    let metallic = 0.0;
    let f0 = vec3<f32>(0.04);

    let ambient_base = ambient_factor * albedo;
    let ao = ambient_occlusion(p, normal, pixel);
    let ambient = ambient_base * ao;

    var lo = vec3<f32>(0.0);
    let sun = sun_direction();
    let sun_color = vec3<f32>(1.0, 0.96, 0.88) * 2.4;
    let shadow_jitter = rand_uniform2(pixel, 13u);
    let sun_jittered = jitter_direction(sun, 0.025, shadow_jitter);
    let shadowed = in_shadow(p, sun_jittered, MAX_T);
    if !shadowed {
        lo += brdf_direct(normal, view_dir, sun, albedo, metallic, roughness, f0) * sun_color;
    }

    var color = ambient + lo;
    color = tonemap_aces(color);
    color = pow(color, vec3<f32>(1.0 / 2.2));

    let sky_tonemapped = pow(tonemap_aces(fade_target), vec3<f32>(1.0 / 2.2));
    color = mix(sky_tonemapped, color, fade);
    return vec4<f32>(color, 1.0);
}

// Direct-only shading used as the *bounce hit* color for one-bounce GI.
// Returns LINEAR radiance (no tonemap, no gamma) so it can be added to
// the primary hit's lo before the final tonemap. Skips IBL, AO, and
// recursive bounces — those are the things that would cause cost
// blow-up or recursion (which WGSL forbids).
fn shade_direct(hit: RayHit, origin: vec3<f32>, dir: vec3<f32>, pixel: vec2<u32>) -> vec3<f32> {
    if hit.face_idx == 0xFFFFFFFFu {
        return sky_color(dir);
    }

    let sun = sun_direction();
    let view_dir = -dir;

    if hit.face_idx == FACE_IDX_GROUND {
        let p = origin + dir * hit.t;
        let normal = vec3<f32>(0.0, 0.0, 1.0);
        let albedo = vec3<f32>(0.22, 0.21, 0.20);
        let ambient = vec3<f32>(0.22, 0.22, 0.23) * albedo;
        var lo = vec3<f32>(0.0);
        if !in_shadow(p, sun, MAX_T) {
            // Lambertian — single light, no fresnel/specular.
            let n_dot_l = max(dot(normal, sun), 0.0);
            lo += albedo / PI * vec3<f32>(1.0, 0.96, 0.88) * 2.4 * n_dot_l;
        }
        return ambient + lo;
    }

    let face = faces[hit.face_idx];
    let mat = materials[face.material_idx];
    let albedo = mat.color.rgb;
    let metallic = mat.metallic;
    let roughness = max(mat.roughness, 0.04);
    let normal = compute_normal(hit);
    let p = origin + dir * hit.t;
    let f0 = mix(vec3<f32>(0.04, 0.04, 0.04), albedo, metallic);

    var lo = vec3<f32>(0.0);
    let sun_color = vec3<f32>(1.0, 0.96, 0.88) * 2.6;
    if !in_shadow(p, sun, MAX_T) {
        lo += brdf_direct(normal, view_dir, sun, albedo, metallic, roughness, f0) * sun_color;
    }
    let fill_dir = normalize(vec3<f32>(-0.4, 0.5, 0.6));
    let fill_color = vec3<f32>(0.7, 0.8, 1.0) * 0.45;
    if !in_shadow(p, fill_dir, MAX_T) {
        lo += brdf_direct(normal, view_dir, fill_dir, albedo, metallic, roughness, f0) * fill_color;
    }
    // Cheap hemisphere ambient — no AO/sky-blend, just a flat tint.
    lo += albedo * 0.08;
    return lo;
}

// PBR shading with Cook-Torrance BRDF, soft shadow rays, AO, one-bounce
// indirect (GI), and HDR env IBL. Z-up world (kernel space).
//
// Stochastic terms (soft shadows, AO, env-spec jitter, GI bounce) are
// decorrelated per-pixel and per-frame so progressive accumulation
// converges to a noise-free image.
fn shade(hit: RayHit, origin: vec3<f32>, dir: vec3<f32>, pixel: vec2<u32>) -> vec4<f32> {
    if hit.face_idx == 0xFFFFFFFFu {
        // Background — render the sky directly.
        let sky = sky_color(dir);
        let mapped = tonemap_aces(sky);
        return vec4<f32>(pow(mapped, vec3<f32>(1.0 / 2.2)), 1.0);
    }

    if hit.face_idx == FACE_IDX_GROUND {
        let p = origin + dir * hit.t;
        return shade_ground(p, dir, hit.uv.x, pixel);
    }

    let face = faces[hit.face_idx];
    let mat = materials[face.material_idx];
    let albedo = mat.color.rgb;
    let metallic = mat.metallic;
    let roughness = max(mat.roughness, 0.04);

    let normal = compute_normal(hit);
    let view_dir = -dir;
    let p = origin + dir * hit.t;

    let f0 = mix(vec3<f32>(0.04, 0.04, 0.04), albedo, metallic);

    // Hemisphere ambient blend, modulated by AO for clean contact darkening.
    let sky_up = mix(vec3<f32>(0.40, 0.42, 0.46), sky_color(normal), 0.3);
    let sky_down = vec3<f32>(0.30, 0.27, 0.24);
    let hemi_factor = max(normal.z, 0.0) * 0.5 + 0.5;
    let ao = ambient_occlusion(p, normal, pixel);
    let ambient = mix(sky_down, sky_up, hemi_factor) * albedo * 0.30 * ao;

    var lo = vec3<f32>(0.0);

    // Soft sun shadow — single jittered ray per frame, smoothed by accumulation.
    let sun = sun_direction();
    let sun_color = vec3<f32>(1.0, 0.96, 0.88) * 2.6;
    let sun_jitter = rand_uniform2(pixel, 17u);
    let sun_jittered = jitter_direction(sun, 0.025, sun_jitter);
    if !in_shadow(p, sun_jittered, MAX_T) {
        lo += brdf_direct(normal, view_dir, sun, albedo, metallic, roughness, f0) * sun_color;
    }

    // Soft fill from upper-back. Wider cone since fill lights are physically
    // larger / softer than the sun.
    let fill_dir = normalize(vec3<f32>(-0.4, 0.5, 0.6));
    let fill_color = vec3<f32>(0.7, 0.8, 1.0) * 0.45;
    let fill_jitter = rand_uniform2(pixel, 23u);
    let fill_jittered = jitter_direction(fill_dir, 0.08, fill_jitter);
    if !in_shadow(p, fill_jittered, MAX_T) {
        lo += brdf_direct(normal, view_dir, fill_dir, albedo, metallic, roughness, f0) * fill_color;
    }

    // Camera-relative wrap fill: cheap, no shadow test, keeps near-camera
    // faces readable when both directional lights are occluded.
    {
        let n_dot_v = max(dot(normal, view_dir), 0.0);
        let kd = (1.0 - metallic);
        let diffuse = kd * albedo / PI;
        lo += diffuse * vec3<f32>(0.85, 0.88, 0.95) * 0.18 * n_dot_v;
    }

    // Specular IBL: jitter the reflection direction within a roughness-sized
    // cone so glossy reflections soften over accumulation rather than ringing.
    let n_dot_v_amb = max(dot(normal, view_dir), 0.0);
    let f_ambient = fresnel_schlick(n_dot_v_amb, f0);
    let reflect_dir = reflect(dir, normal);
    let env_jitter = rand_uniform2(pixel, 31u);
    let env_dir = jitter_direction(reflect_dir, roughness * 0.4, env_jitter);
    let env_specular = sky_color(env_dir) * f_ambient * (1.0 - roughness * 0.85);

    // One-bounce indirect (GI). Cosine-weighted hemisphere sample, traced
    // against BVH + ground; the bounce hit is shaded with shade_direct
    // (no recursion). For a Lambertian + cosine-weighted sample, the brdf
    // and pdf factors cancel to just `albedo`, so the contribution is
    // `albedo * incoming_radiance`. Skipped for metals (they have no
    // Lambertian diffuse bounce — their indirect comes through env_specular).
    let bounce_jitter = rand_uniform2(pixel, 41u);
    let bounce_dir = sample_hemisphere_cosine(normal, bounce_jitter);
    let bounce_origin = p + normal * 0.001;
    var bounce_hit = trace_bvh(bounce_origin, bounce_dir);
    let bounce_ground = intersect_ground(bounce_origin, bounce_dir);
    if bounce_ground.t < bounce_hit.t {
        bounce_hit.t = bounce_ground.t;
        bounce_hit.face_idx = FACE_IDX_GROUND;
        bounce_hit.uv = vec2<f32>(bounce_ground.fade, 0.0);
    }
    let indirect_radiance = shade_direct(bounce_hit, bounce_origin, bounce_dir, pixel);
    let indirect = albedo * (1.0 - metallic) * indirect_radiance * 0.7;

    var color = ambient + lo + env_specular + indirect;

    color = tonemap_aces(color);
    color = pow(color, vec3<f32>(1.0 / 2.2));

    return vec4<f32>(color, 1.0);
}

// HSV to RGB conversion for debug visualization
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> vec3<f32> {
    let c = v * s;
    let h6 = h * 6.0;
    let x = c * (1.0 - abs(fract(h6 / 2.0) * 2.0 - 1.0));
    let m = v - c;

    var rgb: vec3<f32>;
    if h6 < 1.0 {
        rgb = vec3<f32>(c, x, 0.0);
    } else if h6 < 2.0 {
        rgb = vec3<f32>(x, c, 0.0);
    } else if h6 < 3.0 {
        rgb = vec3<f32>(0.0, c, x);
    } else if h6 < 4.0 {
        rgb = vec3<f32>(0.0, x, c);
    } else if h6 < 5.0 {
        rgb = vec3<f32>(x, 0.0, c);
    } else {
        rgb = vec3<f32>(c, 0.0, x);
    }
    return rgb + vec3<f32>(m, m, m);
}

// Compute depth and normal for a pixel
fn trace_depth_normal(pixel: vec2<u32>) -> vec4<f32> {
    let ray = ray_origin_and_direction(pixel);
    let origin = ray[0];
    let dir = ray[1];

    let hit = trace_bvh(origin, dir);

    if hit.face_idx == 0xFFFFFFFFu {
        // Background: max depth, zero normal
        return vec4<f32>(0.0, 0.0, 0.0, MAX_T);
    }

    let normal = compute_normal(hit);
    return vec4<f32>(normal, hit.t);
}

// Edge-aware bilateral spatial filter for denoising stochastic noise
// (soft shadows, AO, GI bounce). Reads accum_buffer at neighbor pixels
// weighted by depth + normal similarity, returning a smoothed color
// that respects geometric edges.
//
// The filter strength scales DOWN with accumulation: at frame 1 (DRAFT)
// it provides aggressive smoothing of single-sample noise; at frame 24+
// (HIGH after settle) it has nearly no effect since the temporal average
// is already clean.
//
// Note: reads accum_buffer at neighbor positions which may have either
// "this frame's value" or "previous frame's value" depending on workgroup
// scheduling. In practice the neighbors' running averages are close
// enough that the bilateral converges correctly.
fn denoise(pixel_coord: vec2<i32>, center_color: vec4<f32>, center_depth_normal: vec4<f32>) -> vec4<f32> {
    let frame_idx = render_state.frame_index;
    if frame_idx > 32u {
        return center_color;
    }
    // Strength: 1.0 at frame 1, decays to ~0 by frame 32.
    let strength = clamp(1.0 - f32(frame_idx) / 32.0, 0.0, 1.0);

    let center_normal = center_depth_normal.xyz;
    let center_depth = center_depth_normal.w;
    let is_background = center_depth >= MAX_T - 1.0;

    // 5x5 box of neighbors (skip center, included separately).
    var sum = center_color.rgb;
    var weight_sum = 1.0;

    for (var dy: i32 = -2; dy <= 2; dy++) {
        for (var dx: i32 = -2; dx <= 2; dx++) {
            if dx == 0 && dy == 0 { continue; }

            let n_coord = pixel_coord + vec2<i32>(dx, dy);
            if n_coord.x < 0 || n_coord.x >= i32(camera.width) ||
               n_coord.y < 0 || n_coord.y >= i32(camera.height) {
                continue;
            }

            let n_dn = depth_normal_buffer[pixel_index_i32(n_coord)];
            let n_normal = n_dn.xyz;
            let n_depth = n_dn.w;
            let n_is_bg = n_depth >= MAX_T - 1.0;

            // Hard reject across silhouettes — never blur foreground into
            // background or vice versa.
            if is_background != n_is_bg { continue; }

            var w = 1.0;
            if !is_background {
                // Depth weight: similar depth → high weight.
                let depth_diff = abs(center_depth - n_depth) / max(center_depth, 0.1);
                w *= exp(-depth_diff * 8.0);

                // Normal weight: similar normal → high weight.
                if length(center_normal) > 0.5 && length(n_normal) > 0.5 {
                    let n_dot = max(dot(normalize(center_normal), normalize(n_normal)), 0.0);
                    w *= pow(n_dot, 6.0);
                }
            }

            // Spatial falloff (Gaussian-ish).
            let r2 = f32(dx * dx + dy * dy);
            w *= exp(-r2 * 0.25);

            let n_color = accum_buffer[pixel_index_i32(n_coord)].rgb;
            sum += n_color * w;
            weight_sum += w;
        }
    }

    let blurred = sum / weight_sum;
    return vec4<f32>(mix(center_color.rgb, blurred, strength), center_color.a);
}

// Sample depth_normal_buffer with coordinate clamped to image bounds.
fn sample_dn(offset: vec2<i32>, center: vec2<i32>) -> vec4<f32> {
    let c = clamp(center + offset,
                  vec2<i32>(0, 0),
                  vec2<i32>(i32(camera.width) - 1, i32(camera.height) - 1));
    return depth_normal_buffer[pixel_index_i32(c)];
}

// Sample feature_id_buffer with coordinate clamped to image bounds.
fn sample_fid(offset: vec2<i32>, center: vec2<i32>) -> u32 {
    let c = clamp(center + offset,
                  vec2<i32>(0, 0),
                  vec2<i32>(i32(camera.width) - 1, i32(camera.height) - 1));
    return feature_id_buffer[pixel_index_i32(c)];
}

// Detect Fusion-style edge lines using 3×3 Sobel on depth+normal plus analytic
// face-ID creases.  Returns vec3(silhouette, crease, boundary) strengths in [0,1].
//
// silhouette — large depth gradient (Sobel), catches diagonal edges without stair-stepping
// crease     — face_id changes between neighbours without a silhouette-level depth jump
// boundary   — foreground pixel adjacent to background (rendered both sides)
fn detect_edge_sobel(pixel_coord: vec2<i32>) -> vec3<f32> {
    // 3×3 neighbourhood samples (clamped at image borders)
    let p00 = sample_dn(vec2<i32>(-1,-1), pixel_coord);
    let p10 = sample_dn(vec2<i32>( 0,-1), pixel_coord);
    let p20 = sample_dn(vec2<i32>( 1,-1), pixel_coord);
    let p01 = sample_dn(vec2<i32>(-1, 0), pixel_coord);
    let p11 = sample_dn(vec2<i32>( 0, 0), pixel_coord); // center
    let p21 = sample_dn(vec2<i32>( 1, 0), pixel_coord);
    let p02 = sample_dn(vec2<i32>(-1, 1), pixel_coord);
    let p12 = sample_dn(vec2<i32>( 0, 1), pixel_coord);
    let p22 = sample_dn(vec2<i32>( 1, 1), pixel_coord);

    let center_depth = p11.w;
    let is_fg = center_depth < MAX_T - 1.0;

    // ---------- Sobel depth gradient (perspective-normalised) ----------
    let d00 = p00.w; let d10 = p10.w; let d20 = p20.w;
    let d01 = p01.w; let d21 = p21.w;
    let d02 = p02.w; let d12 = p12.w; let d22 = p22.w;

    let gx_d = -d00 + d20 - 2.0*d01 + 2.0*d21 - d02 + d22;
    let gy_d = -d00 - 2.0*d10 - d20 + d02 + 2.0*d12 + d22;
    let depth_grad = sqrt(gx_d*gx_d + gy_d*gy_d) / max(center_depth, 0.1);

    // ---------- Sobel normal gradient (sum of all three channels) ----------
    let n00 = p00.xyz; let n10 = p10.xyz; let n20 = p20.xyz;
    let n01 = p01.xyz; let n21 = p21.xyz;
    let n02 = p02.xyz; let n12 = p12.xyz; let n22 = p22.xyz;

    var normal_grad2 = 0.0;
    // Sobel on x channel
    let gx_nx = -n00.x + n20.x - 2.0*n01.x + 2.0*n21.x - n02.x + n22.x;
    let gy_nx = -n00.x - 2.0*n10.x - n20.x + n02.x + 2.0*n12.x + n22.x;
    // Sobel on y channel
    let gx_ny = -n00.y + n20.y - 2.0*n01.y + 2.0*n21.y - n02.y + n22.y;
    let gy_ny = -n00.y - 2.0*n10.y - n20.y + n02.y + 2.0*n12.y + n22.y;
    // Sobel on z channel
    let gx_nz = -n00.z + n20.z - 2.0*n01.z + 2.0*n21.z - n02.z + n22.z;
    let gy_nz = -n00.z - 2.0*n10.z - n20.z + n02.z + 2.0*n12.z + n22.z;
    normal_grad2 = gx_nx*gx_nx + gy_nx*gy_nx
                 + gx_ny*gx_ny + gy_ny*gy_ny
                 + gx_nz*gx_nz + gy_nz*gy_nz;
    let normal_grad = sqrt(normal_grad2);

    // ---------- Silhouette strength ----------
    let depth_threshold = render_state.edge_depth_threshold;
    var silhouette = 0.0;
    if depth_grad > depth_threshold {
        // Use the raw Sobel magnitude (relative to threshold) as AA sub-pixel distance.
        silhouette = clamp((depth_grad - depth_threshold) / depth_threshold, 0.0, 1.0);
    }
    // Add normal Sobel contribution for sharp curvature changes on a single surface.
    let normal_threshold_cos = cos(radians(render_state.edge_normal_threshold));
    let normal_edge_thresh = (1.0 - normal_threshold_cos) * 8.0; // scale to comparable range
    if normal_grad > normal_edge_thresh {
        let n_strength = clamp((normal_grad - normal_edge_thresh) / normal_edge_thresh, 0.0, 1.0);
        silhouette = max(silhouette, n_strength);
    }

    // ---------- Boundary (foreground ↔ background) ----------
    // Check 4-connected neighbours only; boundary pixels get full strength.
    var boundary = 0.0;
    let f01_d = p01.w; let f21_d = p21.w; let f10_d = p10.w; let f12_d = p12.w;
    let bg_t = MAX_T - 1.0;
    if is_fg {
        if f01_d > bg_t || f21_d > bg_t || f10_d > bg_t || f12_d > bg_t {
            boundary = 1.0;
        }
    } else {
        if f01_d < bg_t || f21_d < bg_t || f10_d < bg_t || f12_d < bg_t {
            boundary = 1.0;
        }
    }

    // ---------- Crease (analytic face-ID discontinuity) ----------
    // A crease exists when two adjacent foreground pixels belong to different faces
    // and there is no silhouette-level depth jump between them.
    var crease = 0.0;
    if is_fg {
        let center_fid = feature_id_buffer[pixel_index_i32(pixel_coord)];
        if center_fid != 0xFFFFFFFFu {
            let fid01 = sample_fid(vec2<i32>(-1, 0), pixel_coord);
            let fid21 = sample_fid(vec2<i32>( 1, 0), pixel_coord);
            let fid10 = sample_fid(vec2<i32>( 0,-1), pixel_coord);
            let fid12 = sample_fid(vec2<i32>( 0, 1), pixel_coord);
            let dn01 = p01.w; let dn21 = p21.w;
            let dn10 = p10.w; let dn12 = p12.w;
            let dd_max = depth_threshold * 2.0;

            if fid01 != 0xFFFFFFFFu && fid01 != center_fid
               && abs(center_depth - dn01) / max(center_depth, 0.1) < dd_max {
                crease = 1.0;
            }
            if fid21 != 0xFFFFFFFFu && fid21 != center_fid
               && abs(center_depth - dn21) / max(center_depth, 0.1) < dd_max {
                crease = max(crease, 1.0);
            }
            if fid10 != 0xFFFFFFFFu && fid10 != center_fid
               && abs(center_depth - dn10) / max(center_depth, 0.1) < dd_max {
                crease = max(crease, 1.0);
            }
            if fid12 != 0xFFFFFFFFu && fid12 != center_fid
               && abs(center_depth - dn12) / max(center_depth, 0.1) < dd_max {
                crease = max(crease, 1.0);
            }
        }
    }

    return vec3<f32>(silhouette, crease, boundary);
}

// Sample-count heatmap: t=0.0 → blue (1 sample/cold), t=1.0 → red (max samples/hot).
fn heat_color(t: f32) -> vec3<f32> {
    let t = clamp(t, 0.0, 1.0);
    let r = clamp(t * 2.0 - 1.0, 0.0, 1.0);
    let g = clamp(1.0 - abs(t * 2.0 - 1.0), 0.0, 1.0);
    let b = clamp(1.0 - t * 2.0, 0.0, 1.0);
    return vec3<f32>(r, g, b);
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let pixel = global_id.xy;

    if pixel.x >= camera.width || pixel.y >= camera.height {
        return;
    }

    let ray = ray_origin_and_direction(pixel);
    let origin = ray[0];
    let dir = ray[1];

    // Trace ray using BVH acceleration, then test the implicit ground
    // plane and pick whichever is closer.
    var hit = trace_bvh(origin, dir);
    let ground = intersect_ground(origin, dir);
    if ground.t < hit.t {
        hit.t = ground.t;
        hit.face_idx = FACE_IDX_GROUND;
        hit.uv = vec2<f32>(ground.fade, 0.0);
    }
    let new_color = shade(hit, origin, dir, pixel);

    // Store depth and normal for edge detection. Ground hits get a normal
    // so silhouettes against the ground get drawn just like real faces.
    let pixel_coord = vec2<i32>(pixel);
    var depth_normal: vec4<f32>;
    if hit.face_idx == 0xFFFFFFFFu {
        depth_normal = vec4<f32>(0.0, 0.0, 0.0, MAX_T);
    } else if hit.face_idx == FACE_IDX_GROUND {
        depth_normal = vec4<f32>(0.0, 0.0, 1.0, hit.t);
    } else {
        let normal = compute_normal(hit);
        depth_normal = vec4<f32>(normal, hit.t);
    }

    // Write stable geometry data on the first frame so edge detection has
    // coherent neighbours on frame 2+ (same condition as depth_normal_buffer).
    if render_state.frame_index <= 1u {
        depth_normal_buffer[pixel_index_i32(pixel_coord)] = depth_normal;
        feature_id_buffer[pixel_index_i32(pixel_coord)] = hit.face_idx;
    }

    // Progressive accumulation
    var accumulated: vec4<f32>;

    if render_state.frame_index <= 1u {
        // First frame: start fresh
        accumulated = new_color;
    } else {
        // Blend with previous samples using running average
        let prev = accum_buffer[pixel_index_i32(pixel_coord)];
        let weight = 1.0 / f32(render_state.frame_index);
        accumulated = mix(prev, new_color, weight);
    }

    // Spatial denoise — bilateral filter that smooths within similar
    // depth/normal regions, scaled down with accumulation count so it
    // mostly affects DRAFT/STANDARD tiers and fades out as HIGH settles.
    // Has to run before edge detection so edges are drawn on the
    // denoised image.
    var final_color = accumulated;
    if render_state.frame_index >= 2u {
        let stored_dn = depth_normal_buffer[pixel_index_i32(pixel_coord)];
        final_color = denoise(pixel_coord, accumulated, stored_dn);
    }

    // Apply Fusion-style edge lines on later frames (stable depth/normal/face-ID data).
    // enable_edges is a bit-mask: bit0=silhouette, bit1=crease, bit2=boundary.
    if render_state.enable_edges != 0u && render_state.frame_index >= 2u {
        let strengths = detect_edge_sobel(pixel_coord);

        // Silhouette lines (large depth/normal gradient, bit 0)
        if (render_state.enable_edges & 1u) != 0u && strengths.x > 0.001 {
            let s = clamp(strengths.x * render_state.silhouette_width * render_state.edge_softness,
                          0.0, 1.0);
            final_color = mix(final_color, render_state.silhouette_color, s);
        }

        // Crease lines (analytic face-ID boundary, bit 1)
        if (render_state.enable_edges & 2u) != 0u && strengths.y > 0.001 {
            let s = clamp(render_state.crease_width * render_state.edge_softness, 0.0, 1.0);
            final_color = mix(final_color, render_state.crease_color, s * strengths.y);
        }

        // Boundary lines (foreground↔background, bit 2 — highest priority)
        if (render_state.enable_edges & 4u) != 0u && strengths.z > 0.001 {
            let s = clamp(render_state.boundary_width * render_state.edge_softness, 0.0, 1.0);
            final_color = mix(final_color, render_state.boundary_color, s * strengths.z);
        }
    }

    // Apply debug visualization if enabled. Skip ground/miss sentinels
    // because compute_normal() would index a real face buffer.
    if render_state.debug_mode > 0u && render_state.debug_mode != 5u
        && hit.face_idx != 0xFFFFFFFFu
        && hit.face_idx != FACE_IDX_GROUND {
        let normal = compute_normal(hit);

        if render_state.debug_mode == 1u {
            // Normal visualization: map (-1,1) to (0,1) as RGB
            final_color = vec4<f32>((normal + 1.0) * 0.5, 1.0);
        } else if render_state.debug_mode == 2u {
            // Face ID as color (use HSV for distinct colors)
            let hue = fract(f32(hit.face_idx) * 0.15);
            let face_color = hsv_to_rgb(hue, 1.0, 1.0);
            final_color = vec4<f32>(face_color, 1.0);
        } else if render_state.debug_mode == 3u {
            // N dot L visualization (grayscale) using primary light direction
            let light_dir = normalize(vec3<f32>(0.5, 0.8, 0.3));
            let ndl = max(dot(normal, light_dir), 0.0);
            final_color = vec4<f32>(ndl, ndl, ndl, 1.0);
        } else if render_state.debug_mode == 4u {
            // Face orientation visualization: green=forward(0), red=reversed(1)
            let face = faces[hit.face_idx];
            if face.orientation == 0u {
                final_color = vec4<f32>(0.2, 1.0, 0.2, 1.0);  // Green for forward
            } else {
                final_color = vec4<f32>(1.0, 0.2, 0.2, 1.0);  // Red for reversed
            }
        }
    }

    // Debug mode 5: sample-count heatmap. Main pass fires 1 ray per pixel
    // (blue = cold = minimum). The refine pass overwrites edge pixels with
    // the actual sample count after it runs.
    if render_state.debug_mode == 5u {
        final_color = vec4<f32>(heat_color(0.0), 1.0);
    }

    // Store accumulated color with sample count in alpha (1.0 from main pass).
    // The refine pass may update this for edge pixels.
    accumulated.a = 1.0;

    // Store to accumulation buffer and output
    accum_buffer[pixel_index_i32(pixel_coord)] = accumulated;
    textureStore(output, pixel_coord, final_color);
}

// Adaptive refinement pass: fires additional stratified rays for edge pixels,
// blends with the coarse main-pass sample, and updates the output texture.
// Only runs when render_state.refine_sample_count > 0.
@compute @workgroup_size(8, 8)
fn refine(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let pixel = global_id.xy;

    if pixel.x >= camera.width || pixel.y >= camera.height {
        return;
    }
    if render_state.refine_sample_count == 0u {
        return;
    }

    let pixel_coord = vec2<i32>(pixel);
    let idx = pixel_index(pixel);

    // Detect edge strength from the depth/normal buffer (written by main pass).
    let dn = depth_normal_buffer[idx];
    let edge = detect_edge(pixel_coord, dn);

    // Only refine pixels on silhouettes / creases.
    if edge <= 0.1 {
        return;
    }

    // Stratified sub-pixel grid: grid_size x grid_size additional samples.
    let grid_size = u32(sqrt(f32(render_state.refine_sample_count)));
    if grid_size == 0u {
        return;
    }

    var color_sum = vec3<f32>(0.0);
    var fired = 0u;

    for (var sy = 0u; sy < grid_size; sy++) {
        for (var sx = 0u; sx < grid_size; sx++) {
            // Uniform stratified offset within the pixel, range [-0.5, 0.5].
            let offset = (vec2<f32>(f32(sx), f32(sy)) + 0.5) / f32(grid_size) - 0.5;
            let ray = ray_origin_and_direction_offset(pixel, offset);
            let origin = ray[0];
            let dir = ray[1];

            var hit = trace_bvh(origin, dir);
            let ground = intersect_ground(origin, dir);
            if ground.t < hit.t {
                hit.t = ground.t;
                hit.face_idx = FACE_IDX_GROUND;
                hit.uv = vec2<f32>(ground.fade, 0.0);
            }

            color_sum += shade(hit, origin, dir, pixel).rgb;
            fired += 1u;
        }
    }

    // Blend: existing coarse sample (1 ray, alpha=1.0) + fired new rays.
    let existing = accum_buffer[idx];
    let total = f32(1u + fired);
    let blended_rgb = (existing.rgb + color_sum) / total;

    // Store back with sample count in alpha for heatmap debug mode.
    accum_buffer[idx] = vec4<f32>(blended_rgb, total);

    // Compose the refined output pixel.
    var final_color = vec4<f32>(blended_rgb, 1.0);

    // Re-apply edge overlay (only on later frames, consistent with main pass).
    if render_state.enable_edges == 1u && render_state.frame_index >= 2u {
        let edge_color = vec4<f32>(0.1, 0.1, 0.12, 1.0);
        final_color = mix(final_color, edge_color, edge * 0.8);
    }

    // Debug mode 5: show actual sample count as heatmap (refine overwrites main-pass blue).
    if render_state.debug_mode == 5u {
        let t = clamp((total - 1.0) / f32(render_state.refine_sample_count), 0.0, 1.0);
        final_color = vec4<f32>(heat_color(t), 1.0);
    }

    textureStore(output, pixel_coord, final_color);
}
