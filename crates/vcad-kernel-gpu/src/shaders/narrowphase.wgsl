// Batched copper narrowphase (GPU-router charter M2).
//
// One thread per candidate pair: exact edge-to-edge distance between two
// copper primitives (capsule = swept segment, disc), compared against the
// pair's required clearance. Output per pair: 1 when the pair CLEARS by
// more than `margin` beyond the requirement, else 0 ("needs exact CPU
// check"). The margin absorbs f32-vs-f64 drift so a GPU "clear" verdict is
// safe to trust and everything near the line re-runs on the exact oracle —
// the prefilter contract.

struct PairIn {
    // kind_a/kind_b: 0 = capsule, 1 = disc.
    kind_a: u32,
    kind_b: u32,
    required: f32,
    margin: f32,
    // capsule: (ax, ay) - (bx, by), radius r  |  disc: centre (ax, ay), radius r
    a_ax: f32,
    a_ay: f32,
    a_bx: f32,
    a_by: f32,
    a_r: f32,
    b_ax: f32,
    b_ay: f32,
    b_bx: f32,
    b_by: f32,
    b_r: f32,
    _pad0: f32,
    _pad1: f32,
}

@group(0) @binding(0) var<storage, read> pairs: array<PairIn>;
@group(0) @binding(1) var<storage, read_write> clears: array<u32>;

fn pt_seg_dist(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let ab = b - a;
    let l2 = dot(ab, ab);
    if l2 < 1e-12 {
        return distance(p, a);
    }
    let t = clamp(dot(p - a, ab) / l2, 0.0, 1.0);
    return distance(p, a + ab * t);
}

fn seg_seg_dist(a1: vec2<f32>, b1: vec2<f32>, a2: vec2<f32>, b2: vec2<f32>) -> f32 {
    // Segments intersect => 0; else min endpoint-to-segment distance.
    let d1 = b1 - a1;
    let d2 = b2 - a2;
    let denom = d1.x * d2.y - d1.y * d2.x;
    if abs(denom) > 1e-12 {
        let s = ((a2.x - a1.x) * d2.y - (a2.y - a1.y) * d2.x) / denom;
        let t = ((a2.x - a1.x) * d1.y - (a2.y - a1.y) * d1.x) / denom;
        if s >= 0.0 && s <= 1.0 && t >= 0.0 && t <= 1.0 {
            return 0.0;
        }
    }
    return min(
        min(pt_seg_dist(a1, a2, b2), pt_seg_dist(b1, a2, b2)),
        min(pt_seg_dist(a2, a1, b1), pt_seg_dist(b2, a1, b1)),
    );
}

@compute @workgroup_size(256)
fn narrowphase(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if i >= arrayLength(&pairs) {
        return;
    }
    let p = pairs[i];
    var centre_dist: f32;
    if p.kind_a == 0u && p.kind_b == 0u {
        centre_dist = seg_seg_dist(
            vec2(p.a_ax, p.a_ay), vec2(p.a_bx, p.a_by),
            vec2(p.b_ax, p.b_ay), vec2(p.b_bx, p.b_by),
        );
    } else if p.kind_a == 0u {
        centre_dist = pt_seg_dist(vec2(p.b_ax, p.b_ay), vec2(p.a_ax, p.a_ay), vec2(p.a_bx, p.a_by));
    } else if p.kind_b == 0u {
        centre_dist = pt_seg_dist(vec2(p.a_ax, p.a_ay), vec2(p.b_ax, p.b_ay), vec2(p.b_bx, p.b_by));
    } else {
        centre_dist = distance(vec2(p.a_ax, p.a_ay), vec2(p.b_ax, p.b_ay));
    }
    let edge = centre_dist - p.a_r - p.b_r;
    clears[i] = select(0u, 1u, edge >= p.required + p.margin);
}
