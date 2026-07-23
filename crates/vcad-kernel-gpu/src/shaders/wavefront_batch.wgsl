// Batched multi-search wavefront relaxation (GPU-router charter M1).
//
// One dispatch relaxes N independent searches against ONE shared occupancy
// raster (the resident class raster: u8 cell states packed 4-per-u32,
// 0 = BLOCKED, 1 = TIGHT, 2 = WIDE — TIGHT/WIDE are both passable here;
// edge-exactness is the CPU validator's job, per the charter invariant).
//
// Each search adds a per-node OVERLAY bit (its own net's copper: passable
// and typically seeded at distance 0), stored as one bitset per search.
// Thread space is search-major: global id = search * total + node.
//
// Ping-pong dist buffers hold all searches contiguously; a per-search
// `changed` flag lets the host retire settled searches from the sweep loop
// without readback inside the loop (flags are read every K sweeps).

struct Params {
    nx: u32,
    ny: u32,
    nl: u32,
    n_searches: u32,
}

const INF: u32 = 0xffffffffu;

@group(0) @binding(0) var<storage, read> occupancy_words: array<u32>;
@group(0) @binding(1) var<storage, read> overlay_bits: array<u32>;
@group(0) @binding(2) var<storage, read> dist_in: array<u32>;
@group(0) @binding(3) var<storage, read_write> dist_out: array<u32>;
@group(0) @binding(4) var<storage, read_write> changed: array<atomic<u32>>;
@group(0) @binding(5) var<uniform> params: Params;
// (layer x 10-move) cost table compiled from the tang-expr cost model:
// moves [E, W, N, S, NE, NW, SE, SW, via_up, via_down].
@group(0) @binding(6) var<storage, read> move_costs: array<u32>;
// Per-node negotiated-congestion history (PathFinder pricing, charter M4):
// added to every arrival at the node, shared by all searches of the batch.
@group(0) @binding(7) var<storage, read> history: array<u32>;

fn move_cost(l: u32, mv: u32) -> u32 {
    return move_costs[l * 10u + mv];
}

fn total_nodes() -> u32 {
    return params.nx * params.ny * params.nl;
}

fn cell_state(node: u32) -> u32 {
    let word = occupancy_words[node / 4u];
    return (word >> ((node % 4u) * 8u)) & 0xffu;
}

fn overlay(search: u32, node: u32) -> bool {
    let bit_index = search * total_nodes() + node;
    let word = overlay_bits[bit_index / 32u];
    return ((word >> (bit_index % 32u)) & 1u) == 1u;
}

fn passable(search: u32, node: u32) -> bool {
    if cell_state(node) != 0u {
        return true;
    }
    return overlay(search, node);
}

fn candidate(search: u32, base: u32, x: i32, y: i32, l: u32, cost: u32) -> u32 {
    if x < 0 || y < 0 || u32(x) >= params.nx || u32(y) >= params.ny {
        return INF;
    }
    let idx = (l * params.ny + u32(y)) * params.nx + u32(x);
    if !passable(search, idx) {
        return INF;
    }
    let d = dist_in[base + idx];
    if d == INF {
        return INF;
    }
    let sum = d + cost + history[idx];
    if sum < d {
        return INF;
    }
    return sum;
}

@compute @workgroup_size(256)
fn relax_batch(@builtin(global_invocation_id) gid: vec3<u32>) {
    let total = total_nodes();
    let flat = gid.x;
    if flat >= total * params.n_searches {
        return;
    }
    let search = flat / total;
    let node = flat - search * total;
    let base = search * total;

    let cur = dist_in[base + node];
    var best = cur;

    if passable(search, node) {
        let l = node / (params.ny * params.nx);
        let rem = node - l * params.ny * params.nx;
        let y = i32(rem / params.nx);
        let x = i32(rem - u32(y) * params.nx);

        // 8-way in-plane; the arrival cost is the NEIGHBOUR's move toward
        // this node (E-from-west etc.), so index by the reverse move on the
        // same layer table.
        best = min(best, candidate(search, base, x - 1, y, l, move_cost(l, 0u)));
        best = min(best, candidate(search, base, x + 1, y, l, move_cost(l, 1u)));
        best = min(best, candidate(search, base, x, y - 1, l, move_cost(l, 3u)));
        best = min(best, candidate(search, base, x, y + 1, l, move_cost(l, 2u)));
        best = min(best, candidate(search, base, x - 1, y - 1, l, move_cost(l, 6u)));
        best = min(best, candidate(search, base, x + 1, y - 1, l, move_cost(l, 7u)));
        best = min(best, candidate(search, base, x - 1, y + 1, l, move_cost(l, 4u)));
        best = min(best, candidate(search, base, x + 1, y + 1, l, move_cost(l, 5u)));
        // Vias to adjacent layers.
        if l > 0u {
            best = min(best, candidate(search, base, x, y, l - 1u, move_cost(l - 1u, 8u)));
        }
        if l + 1u < params.nl {
            best = min(best, candidate(search, base, x, y, l + 1u, move_cost(l + 1u, 9u)));
        }
    }

    dist_out[base + node] = best;
    if best != cur {
        atomicStore(&changed[search], 1u);
    }
}
