// Analytic surface parameterisation shared by every shader that shades.
//
// Prepended alongside `bsdf.wgsl` (which it depends on for `onb`), so the
// surface-tangent maths exists in exactly one place and the parity harness can
// drive it directly rather than through a full render.

const SURFACE_PLANE: u32 = 0u;
const SURFACE_CYLINDER: u32 = 1u;
const SURFACE_SPHERE: u32 = 2u;
const SURFACE_CONE: u32 = 3u;
const SURFACE_TORUS: u32 = 4u;
const SURFACE_BILINEAR: u32 = 5u;
// A mesh triangle, packed into a GpuSurface's params by
// `GpuSurface::triangle` in buffers.rs. Not a parametric surface: it has no
// dP/du, so `surface_dpdu` deliberately leaves it in the default arm and the
// shading frame falls back to an arbitrary orthonormal basis — which is what
// an isotropic BSDF wants anyway.
const SURFACE_TRIANGLE: u32 = 7u;

const MAX_T: f32 = 1e10;
const EPSILON: f32 = 1e-6;

// Layout must match GpuSurface in buffers.rs.
struct GpuSurface {
    surface_type: u32,
    // Use explicit u32 padding instead of vec3<u32> to match Rust layout
    // vec3<u32> in WGSL has 16-byte alignment which would misalign params
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    params: array<f32, 32>,
}

// Surface tangent dP/du, or a zero vector where the parameterisation is
// degenerate (sphere poles, cone apex).
//
// Ports the geom crate's `d_du` for each analytic surface — the same quantity
// `intersect::surface_tangent` hands the CPU path tracer — so an anisotropic
// highlight follows the SAME circumferential grain in both renderers.
//
// Every curved surface here shares the d/du direction (-sin u)·ref + (cos u)·y,
// differing only by a scalar. The scalar is kept because its magnitude is what
// detects degeneracy; its sign is irrelevant, since the GGX alpha ellipse is
// symmetric and t and -t give the same lobe.
fn surface_dpdu(surface_type: u32, params: array<f32, 32>, uv: vec2<f32>) -> vec3<f32> {
    let u = uv.x;
    let v = uv.y;
    let cos_u = cos(u);
    let sin_u = sin(u);

    switch surface_type {
        case SURFACE_PLANE: {
            // d/du is the constant x_dir.
            return vec3<f32>(params[3], params[4], params[5]);
        }
        case SURFACE_SPHERE: {
            // center (3), radius (1), ref_dir (3), axis (3)
            let radius = params[3];
            let ref_dir = vec3<f32>(params[4], params[5], params[6]);
            let axis = vec3<f32>(params[7], params[8], params[9]);
            let y_dir = cross(axis, ref_dir);
            return (ref_dir * (-sin_u) + y_dir * cos_u) * (radius * cos(v));
        }
        case SURFACE_CYLINDER: {
            // center (3), axis (3), ref_dir (3), radius (1)
            let axis = vec3<f32>(params[3], params[4], params[5]);
            let ref_dir = vec3<f32>(params[6], params[7], params[8]);
            let radius = params[9];
            let y_dir = cross(axis, ref_dir);
            return (ref_dir * (-sin_u) + y_dir * cos_u) * radius;
        }
        case SURFACE_CONE: {
            // apex (3), axis (3), ref_dir (3), half_angle (1)
            let axis = vec3<f32>(params[3], params[4], params[5]);
            let ref_dir = vec3<f32>(params[6], params[7], params[8]);
            let half_angle = params[9];
            let y_dir = cross(axis, ref_dir);
            return (ref_dir * (-sin_u) + y_dir * cos_u) * (v * sin(half_angle));
        }
        case SURFACE_TORUS: {
            // center (3), axis (3), ref_dir (3), R (1), r (1)
            let axis = vec3<f32>(params[3], params[4], params[5]);
            let ref_dir = vec3<f32>(params[6], params[7], params[8]);
            let major = params[9];
            let minor = params[10];
            let y_dir = cross(axis, ref_dir);
            return (ref_dir * (-sin_u) + y_dir * cos_u) * (major + minor * cos(v));
        }
        default: {
            // Bilinear / B-spline: no analytic tangent, same as the CPU.
            return vec3<f32>(0.0);
        }
    }
}

// Shading tangent frame around a unit normal.
//
// Mirrors `pathtrace::shading_frame`: when the hit carries a surface tangent it
// is Gram-Schmidt orthogonalised against the (face-forwarded) normal and used
// as the frame's x axis, so the anisotropic lobe follows the surface's own
// parameterisation. Otherwise fall back to the arbitrary `onb` basis — which is
// exactly what an isotropic material wants, since its BSDF is invariant to the
// choice.
fn shading_frame(n: vec3<f32>, dpdu: vec3<f32>) -> mat3x3<f32> {
    let t_raw = dpdu - n * dot(dpdu, n);
    // A tangent (numerically) parallel to the normal carries no direction;
    // fall back rather than normalising noise.
    if length(t_raw) > 1e-9 {
        let t = normalize(t_raw);
        return mat3x3<f32>(t, cross(n, t), n);
    }
    return onb(n);
}
