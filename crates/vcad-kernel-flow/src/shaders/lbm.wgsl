// D3Q19 BGK lattice-Boltzmann step, split into collide + stream passes.
// Mirrors the CPU reference in solve.rs (isothermal path); the CPU
// solver is the claim-grade oracle, this kernel is the preview lattice.
// Cell kinds: 0 = solid, 1 = fluid, 2 = inlet, 3 = outlet.

struct Params {
    dims: vec4<u32>,      // nx, ny, nz, unused
    fluid: vec4<f32>,     // omega, rho_out_lat, unused, unused
    u_in: vec4<f32>,      // ramped inlet velocity (lattice units)
    a_body: vec4<f32>,    // body acceleration (lattice units)
    periodic: vec4<u32>,  // per-axis wrap flags
};

@group(0) @binding(0) var<storage, read> cells: array<u32>;
@group(0) @binding(1) var<storage, read_write> f_in: array<f32>;
@group(0) @binding(2) var<storage, read_write> f_out: array<f32>;
@group(0) @binding(3) var<storage, read_write> vel: array<vec4<f32>>;
@group(0) @binding(4) var<uniform> params: Params;

const CX = array<i32, 19>(0, 1,-1, 0, 0, 0, 0, 1,-1, 1,-1, 1,-1, 1,-1, 0, 0, 0, 0);
const CY = array<i32, 19>(0, 0, 0, 1,-1, 0, 0, 1,-1,-1, 1, 0, 0, 0, 0, 1,-1, 1,-1);
const CZ = array<i32, 19>(0, 0, 0, 0, 0, 1,-1, 0, 0, 0, 0, 1,-1,-1, 1, 1,-1,-1, 1);
const WQ = array<f32, 19>(
    0.33333333333333333,
    0.05555555555555555, 0.05555555555555555, 0.05555555555555555,
    0.05555555555555555, 0.05555555555555555, 0.05555555555555555,
    0.027777777777777776, 0.027777777777777776, 0.027777777777777776,
    0.027777777777777776, 0.027777777777777776, 0.027777777777777776,
    0.027777777777777776, 0.027777777777777776, 0.027777777777777776,
    0.027777777777777776, 0.027777777777777776, 0.027777777777777776
);
const OPPQ = array<u32, 19>(0u, 2u, 1u, 4u, 3u, 6u, 5u, 8u, 7u, 10u, 9u, 12u, 11u, 14u, 13u, 16u, 15u, 18u, 17u);

fn cell_count() -> u32 {
    return params.dims.x * params.dims.y * params.dims.z;
}

@compute @workgroup_size(256)
fn collide(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    if (x >= cell_count() || cells[x] != 1u) {
        return;
    }
    var cx = CX; var cy = CY; var cz = CZ; var wq = WQ;
    var rho = 0.0;
    var m = vec3<f32>(0.0);
    for (var q = 0u; q < 19u; q++) {
        let fi = f_in[x * 19u + q];
        rho += fi;
        m += fi * vec3<f32>(f32(cx[q]), f32(cy[q]), f32(cz[q]));
    }
    let a = params.a_body.xyz;
    let u = (m + 0.5 * a * rho) / rho;
    vel[x] = vec4<f32>(u, rho);
    let omega = params.fluid.x;
    let phi = 1.0 - 0.5 * omega;
    let uu = dot(u, u);
    for (var q = 0u; q < 19u; q++) {
        let c = vec3<f32>(f32(cx[q]), f32(cy[q]), f32(cz[q]));
        let cu = dot(c, u);
        let feq = wq[q] * rho * (1.0 + 3.0 * cu + 4.5 * cu * cu - 1.5 * uu);
        let ca = dot(c, a);
        let ua = dot(u, a);
        let guo = phi * wq[q] * rho * (3.0 * (ca - ua) + 9.0 * cu * ca);
        let i = x * 19u + q;
        f_in[i] = f_in[i] - omega * (f_in[i] - feq) + guo;
    }
}

@compute @workgroup_size(256)
fn stream(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    if (x >= cell_count() || cells[x] != 1u) {
        return;
    }
    let nx = i32(params.dims.x);
    let ny = i32(params.dims.y);
    let nz = i32(params.dims.z);
    let i0 = i32(x) % nx;
    let j0 = (i32(x) / nx) % ny;
    let k0 = i32(x) / (nx * ny);
    var cx = CX; var cy = CY; var cz = CZ; var wq = WQ; var oppq = OPPQ;
    for (var q = 0u; q < 19u; q++) {
        var si = i0 - cx[q];
        var sj = j0 - cy[q];
        var sk = k0 - cz[q];
        var outside = false;
        if (si < 0 || si >= nx) {
            if (params.periodic.x != 0u) { si = (si + nx) % nx; } else { outside = true; }
        }
        if (sj < 0 || sj >= ny) {
            if (params.periodic.y != 0u) { sj = (sj + ny) % ny; } else { outside = true; }
        }
        if (sk < 0 || sk >= nz) {
            if (params.periodic.z != 0u) { sk = (sk + nz) % nz; } else { outside = true; }
        }
        var kind = 0u;
        var sx = 0u;
        if (!outside) {
            sx = u32((sk * ny + sj) * nx + si);
            kind = cells[sx];
        }
        var value: f32;
        if (kind == 1u) {
            value = f_in[sx * 19u + q];
        } else if (kind == 2u) {
            let c = vec3<f32>(f32(cx[q]), f32(cy[q]), f32(cz[q]));
            let cu = dot(c, params.u_in.xyz);
            value = f_in[x * 19u + oppq[q]] + 6.0 * wq[q] * cu;
        } else if (kind == 3u) {
            let u = vel[x].xyz;
            let c = vec3<f32>(f32(cx[q]), f32(cy[q]), f32(cz[q]));
            let cu = dot(c, u);
            let uu = dot(u, u);
            value = -f_in[x * 19u + oppq[q]]
                + 2.0 * wq[q] * params.fluid.y * (1.0 + 4.5 * cu * cu - 1.5 * uu);
        } else {
            // Solid or out-of-domain wall: half-way bounce-back.
            value = f_in[x * 19u + oppq[q]];
        }
        f_out[x * 19u + q] = value;
    }
}
