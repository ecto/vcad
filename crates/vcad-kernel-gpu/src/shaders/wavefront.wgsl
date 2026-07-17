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
//
// A second, frontier-compacted path lives in the same module
// (`frontier_prep` + `relax_compacted`): instead of sweeping every node,
// each iteration relaxes only the nodes touched last iteration. The
// frontier is a compacted list of node indices with an atomic counter;
// duplicates are suppressed with an epoch-stamped `stamp` buffer
// (a node is appended at most once per iteration: the first thread to
// atomicMax the stamp up to the current epoch wins the append).
// `frontier_prep` runs single-threaded before each iteration to turn the
// counter into DispatchIndirect workgroup counts, reset the outgoing
// counter, and bump the epoch — so the host never reads counters back
// inside the loop.

struct Params {
    nx: u32,
    ny: u32,
    nl: u32,
    step_cost: u32,
    diag_cost: u32,
    via_cost: u32,
    // Frontier parity for the compacted path: 0 = frontier A is input,
    // 1 = frontier B is input. Unused by the full-sweep `relax` kernel.
    parity: u32,
    _pad1: u32,
}

// Layout of the compacted-path `meta` buffer (all atomic u32):
//   [0..3) DispatchIndirect args (x, y, z) for `relax_compacted`
//   [3]    epoch (bumped once per iteration by `frontier_prep`)
//   [4]    frontier A length
//   [5]    frontier B length
const META_DISPATCH_X: u32 = 0u;
const META_EPOCH: u32 = 3u;
const META_COUNT_A: u32 = 4u;

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

// ---- Frontier-compacted path ----------------------------------------

// Distances, updated in place with atomicMin (deterministic final field
// regardless of relaxation order).
@group(0) @binding(5) var<storage, read_write> dist: array<atomic<u32>>;
// Compacted frontier node lists (ping-pong; parity picks which is input).
@group(0) @binding(6) var<storage, read> frontier_in: array<u32>;
@group(0) @binding(7) var<storage, read_write> frontier_out: array<u32>;
// Per-node epoch stamp for duplicate suppression.
@group(0) @binding(8) var<storage, read_write> stamp: array<atomic<u32>>;
// Dispatch args + epoch + frontier lengths; see layout above.
@group(0) @binding(9) var<storage, read_write> fmeta: array<atomic<u32>>;

// Single-threaded per-iteration bookkeeping: publish the input frontier
// length as indirect workgroup counts, clear the output counter, and
// advance the epoch.
@compute @workgroup_size(1)
fn frontier_prep() {
    let n = atomicLoad(&fmeta[META_COUNT_A + params.parity]);
    atomicStore(&fmeta[META_DISPATCH_X], (n + 255u) / 256u);
    atomicStore(&fmeta[META_DISPATCH_X + 1u], 1u);
    atomicStore(&fmeta[META_DISPATCH_X + 2u], 1u);
    atomicStore(&fmeta[META_COUNT_A + (1u - params.parity)], 0u);
    atomicAdd(&fmeta[META_EPOCH], 1u);
}

// Relax one neighbor: lower its distance with atomicMin and, if this is
// the first improvement of the node this iteration (epoch stamp), append
// it to the outgoing frontier.
fn try_relax(idx: u32, nd: u32, epoch: u32) {
    if occupancy[idx] != 0u {
        return;
    }
    let old = atomicMin(&dist[idx], nd);
    if nd < old {
        if atomicMax(&stamp[idx], epoch) < epoch {
            let slot = atomicAdd(&fmeta[META_COUNT_A + (1u - params.parity)], 1u);
            frontier_out[slot] = idx;
        }
    }
}

// One thread per entry of the input frontier: relax all neighbors of the
// frontier node. Improved neighbors join the next frontier (deduped).
@compute @workgroup_size(256)
fn relax_compacted(@builtin(global_invocation_id) gid: vec3<u32>) {
    let n = atomicLoad(&fmeta[META_COUNT_A + params.parity]);
    if gid.x >= n {
        return;
    }
    let idx = frontier_in[gid.x];
    let d = atomicLoad(&dist[idx]);
    if d == INF {
        return;
    }
    let epoch = atomicLoad(&fmeta[META_EPOCH]);

    let x = i32(idx % params.nx);
    let y = i32((idx / params.nx) % params.ny);
    let l = idx / (params.nx * params.ny);

    for (var dy = -1; dy <= 1; dy += 1) {
        for (var dx = -1; dx <= 1; dx += 1) {
            if dx == 0 && dy == 0 {
                continue;
            }
            let px = x + dx;
            let py = y + dy;
            if px < 0 || py < 0 || u32(px) >= params.nx || u32(py) >= params.ny {
                continue;
            }
            var cost = params.step_cost;
            if dx != 0 && dy != 0 {
                cost = params.diag_cost;
            }
            let nd = d + cost;
            if nd < d {
                continue; // overflow guard
            }
            try_relax(node_index(u32(px), u32(py), l), nd, epoch);
        }
    }

    let nd_via = d + params.via_cost;
    if nd_via >= d {
        let cell = idx % (params.nx * params.ny);
        if l > 0u {
            try_relax((l - 1u) * params.nx * params.ny + cell, nd_via, epoch);
        }
        if l + 1u < params.nl {
            try_relax((l + 1u) * params.nx * params.ny + cell, nd_via, epoch);
        }
    }
}
