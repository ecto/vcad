// Multi-layer wavefront distance relaxation shader.
//
// Iterative Bellman-Ford-style relaxation over a 3D routing grid
// (nx x ny x nl). Each thread owns one node and relaxes its distance
// from the 8 in-plane neighbors (orthogonal cost `step`, diagonal cost
// `diag`) and the same cell on adjacent layers (via cost `via`).
//
// Ping-pong buffering: reads `dist_in`, writes `dist_out`. The host
// swaps the buffers between iterations and stops when a sweep makes
// no improvement (the `changed` flag stays 0).

struct Params {
    nx: u32,
    ny: u32,
    nl: u32,
    step_cost: u32,
    diag_cost: u32,
    via_cost: u32,
    _pad0: u32,
    _pad1: u32,
}

const INF: u32 = 0xffffffffu;

@group(0) @binding(0) var<storage, read> occupancy: array<u32>;
@group(0) @binding(1) var<storage, read> dist_in: array<u32>;
@group(0) @binding(2) var<storage, read_write> dist_out: array<u32>;
@group(0) @binding(3) var<storage, read_write> changed: atomic<u32>;
@group(0) @binding(4) var<uniform> params: Params;

fn node_index(x: u32, y: u32, l: u32) -> u32 {
    return (l * params.ny + y) * params.nx + x;
}

fn candidate(x: i32, y: i32, l: u32, cost: u32) -> u32 {
    if x < 0 || y < 0 || u32(x) >= params.nx || u32(y) >= params.ny {
        return INF;
    }
    let idx = node_index(u32(x), u32(y), l);
    let d = dist_in[idx];
    if d == INF {
        return INF;
    }
    // Costs are small integers; d + cost cannot overflow before hitting
    // INF in any grid we can allocate, but saturate defensively.
    let sum = d + cost;
    if sum < d {
        return INF;
    }
    return sum;
}

@compute @workgroup_size(256)
fn relax(@builtin(global_invocation_id) gid: vec3<u32>) {
    let total = params.nx * params.ny * params.nl;
    let idx = gid.x;
    if idx >= total {
        return;
    }

    let cur = dist_in[idx];

    // Blocked nodes never improve.
    if occupancy[idx] != 0u {
        dist_out[idx] = cur;
        return;
    }

    let x = i32(idx % params.nx);
    let y = i32((idx / params.nx) % params.ny);
    let l = idx / (params.nx * params.ny);

    var best = cur;

    // 4 orthogonal in-plane neighbors.
    best = min(best, candidate(x - 1, y, l, params.step_cost));
    best = min(best, candidate(x + 1, y, l, params.step_cost));
    best = min(best, candidate(x, y - 1, l, params.step_cost));
    best = min(best, candidate(x, y + 1, l, params.step_cost));

    // 4 diagonal in-plane neighbors.
    best = min(best, candidate(x - 1, y - 1, l, params.diag_cost));
    best = min(best, candidate(x + 1, y - 1, l, params.diag_cost));
    best = min(best, candidate(x - 1, y + 1, l, params.diag_cost));
    best = min(best, candidate(x + 1, y + 1, l, params.diag_cost));

    // Same cell on adjacent layers (via).
    if l > 0u {
        best = min(best, candidate(x, y, l - 1u, params.via_cost));
    }
    if l + 1u < params.nl {
        best = min(best, candidate(x, y, l + 1u, params.via_cost));
    }

    dist_out[idx] = best;
    if best < cur {
        atomicStore(&changed, 1u);
    }
}
