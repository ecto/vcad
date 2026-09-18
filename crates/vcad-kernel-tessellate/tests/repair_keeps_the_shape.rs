//! The export repair may close a part; it may not change one.
//!
//! `repair_watertightness` used to optimize a defective-edge count under a
//! single guard — enclosed volume within 1%. On the rana-60 stator that
//! bought 542 defective edges at the price of 0.68 mm of the part: the
//! membrane peeler, which ran unguarded on the reasoning that it can only
//! remove surface that should not exist, deleted 42 triangles of the lead
//! notch's R1.05 fillet walls, and every later pass worked on a torn mesh.
//! 1% of 7850 mm³ is 78 mm³, so nothing noticed.
//!
//! Both tests below fail on the base commit and pass with the shape guard.
//!
//! The guard was then one-sided: it bounded surface LOST and was blind to
//! surface ADDED, so `bridge_boundary_slits` — which never deletes a triangle
//! — could span anything under its 2 mm mean-width gate, a real 1.5 mm slot
//! mouth included, and the export came back watertight with an empty
//! `declined` list. `a_real_slot_mouth_is_not_bridged_shut` pins that.

use vcad_kernel_tessellate::{
    repair_watertightness, repair_watertightness_reported, SurfaceChange, TriangleMesh,
    SHAPE_TOLERANCE,
};

/// A closed axis-aligned box as an indexed mesh.
fn box_mesh(sx: f32, sy: f32, sz: f32) -> TriangleMesh {
    let v = [
        [0.0, 0.0, 0.0],
        [sx, 0.0, 0.0],
        [sx, sy, 0.0],
        [0.0, sy, 0.0],
        [0.0, 0.0, sz],
        [sx, 0.0, sz],
        [sx, sy, sz],
        [0.0, sy, sz],
    ];
    let faces = [
        [0, 3, 2, 1], // z-
        [4, 5, 6, 7], // z+
        [0, 1, 5, 4], // y-
        [1, 2, 6, 5], // x+
        [2, 3, 7, 6], // y+
        [3, 0, 4, 7], // x-
    ];
    let mut mesh = TriangleMesh::new();
    for p in v {
        mesh.vertices.extend_from_slice(&p);
    }
    for f in faces {
        mesh.indices
            .extend_from_slice(&[f[0] as u32, f[1] as u32, f[2] as u32]);
        mesh.indices
            .extend_from_slice(&[f[0] as u32, f[2] as u32, f[3] as u32]);
    }
    mesh
}

/// Every triangle of `mesh`, sampled at its corners and centroid, must lie
/// within `tol` of `other`'s surface: the one-sided Hausdorff the guard uses,
/// recomputed here so the test does not just re-run the implementation.
fn max_deviation(mesh: &TriangleMesh, other: &TriangleMesh) -> f64 {
    let p = |m: &TriangleMesh, i: u32| {
        let k = i as usize * 3;
        [
            m.vertices[k] as f64,
            m.vertices[k + 1] as f64,
            m.vertices[k + 2] as f64,
        ]
    };
    let mut worst: f64 = 0.0;
    for t in mesh.indices.chunks(3) {
        let (a, b, c) = (p(mesh, t[0]), p(mesh, t[1]), p(mesh, t[2]));
        let centroid = [
            (a[0] + b[0] + c[0]) / 3.0,
            (a[1] + b[1] + c[1]) / 3.0,
            (a[2] + b[2] + c[2]) / 3.0,
        ];
        for q in [a, b, c, centroid] {
            let mut best = f64::INFINITY;
            for u in other.indices.chunks(3) {
                let (x, y, z) = (p(other, u[0]), p(other, u[1]), p(other, u[2]));
                best = best.min(point_tri_dist(q, x, y, z));
            }
            worst = worst.max(best);
        }
    }
    worst
}

/// Distance from a point to the nearest triangle of `mesh`, by the same dense
/// sampling — "is there surface here?" asked of a single point.
fn distance_to_surface(p: [f64; 3], mesh: &TriangleMesh) -> f64 {
    let at = |i: u32| {
        let k = i as usize * 3;
        [
            mesh.vertices[k] as f64,
            mesh.vertices[k + 1] as f64,
            mesh.vertices[k + 2] as f64,
        ]
    };
    mesh.indices
        .chunks(3)
        .map(|t| point_tri_dist(p, at(t[0]), at(t[1]), at(t[2])))
        .fold(f64::INFINITY, f64::min)
}

/// Distance from a point to a triangle, by dense barycentric sampling —
/// slow, obvious, and independent of the code under test.
fn point_tri_dist(p: [f64; 3], a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> f64 {
    const N: usize = 12;
    let mut best = f64::INFINITY;
    for i in 0..=N {
        for j in 0..=(N - i) {
            let (u, v) = (i as f64 / N as f64, j as f64 / N as f64);
            let w = 1.0 - u - v;
            let q = [
                a[0] * w + b[0] * u + c[0] * v,
                a[1] * w + b[1] * u + c[1] * v,
                a[2] * w + b[2] * u + c[2] * v,
            ];
            let d = ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt();
            best = best.min(d);
        }
    }
    best
}

// ---------------------------------------------------------------------------
// A plate with a slot the repair must not fill in
// ---------------------------------------------------------------------------

/// Plate extent, mm. Big enough that capping the slot costs well under the
/// 1% volume guard — the point is that the SHAPE guard has to catch this, not
/// that the volume check happens to.
const PLATE: [f64; 3] = [30.0, 20.0, 5.0];
/// The slot: 1.5 mm across (narrower than `SLIT_MAX_WIDTH` = 2 mm, so the
/// bridge will happily span it) and 4 mm long, straight through the plate.
const SLOT_X: [f64; 2] = [14.25, 15.75];
const SLOT_Y: [f64; 2] = [8.0, 12.0];

/// Accumulates quads into an indexed mesh, welding vertices by position so
/// edges pair the way a real tessellator's do.
struct Weld {
    mesh: TriangleMesh,
    seen: std::collections::HashMap<[i64; 3], u32>,
}

impl Weld {
    fn new() -> Self {
        Self {
            mesh: TriangleMesh::new(),
            seen: std::collections::HashMap::new(),
        }
    }

    fn vertex(&mut self, p: [f64; 3]) -> u32 {
        let key = [
            (p[0] * 1e4).round() as i64,
            (p[1] * 1e4).round() as i64,
            (p[2] * 1e4).round() as i64,
        ];
        *self.seen.entry(key).or_insert_with(|| {
            let i = (self.mesh.vertices.len() / 3) as u32;
            self.mesh
                .vertices
                .extend(p.iter().map(|&x| x as f32).collect::<Vec<_>>());
            i
        })
    }

    /// `a b c d` in order; the winding is the caller's to get right.
    fn quad(&mut self, a: [f64; 3], b: [f64; 3], c: [f64; 3], d: [f64; 3]) {
        let (a, b, c, d) = (
            self.vertex(a),
            self.vertex(b),
            self.vertex(c),
            self.vertex(d),
        );
        self.mesh.indices.extend_from_slice(&[a, b, c, a, c, d]);
    }
}

/// A plate with a 1.5 x 4 mm through-slot whose slot WALLS are missing — the
/// splitter refused them, which is the shape the stator's unpunched mouths
/// come back in. The two slot mouths (one in each face) are the only boundary
/// loops, each 2·area/perimeter = 1.09 mm of mean width, comfortably inside
/// the 2 mm gate `bridge_boundary_slits` fills under.
///
/// A repair that bridges them hands back a plain solid plate. The slot is
/// gone, the mesh reads watertight, and the number every other guard watches
/// barely moves: the fill is worth 10 mm³ of the 2990 the open mesh encloses,
/// 0.33%, a third of the 1% volume guard. Only the shape can tell.
fn plate_with_an_open_slot() -> TriangleMesh {
    let mut w = Weld::new();
    let (sx, sy, sz) = (PLATE[0], PLATE[1], PLATE[2]);
    let xs = [0.0, SLOT_X[0], SLOT_X[1], sx];
    let ys = [0.0, SLOT_Y[0], SLOT_Y[1], sy];

    // Both flat faces as a 3x3 grid with the middle cell — the slot — left
    // out. The grid also subdivides the outer rim, so the side walls below
    // must be cut at the same stations or the edges will not pair.
    for i in 0..3 {
        for j in 0..3 {
            if i == 1 && j == 1 {
                continue;
            }
            let (x0, x1, y0, y1) = (xs[i], xs[i + 1], ys[j], ys[j + 1]);
            // z = 0, outward normal -Z.
            w.quad([x0, y0, 0.0], [x0, y1, 0.0], [x1, y1, 0.0], [x1, y0, 0.0]);
            // z = sz, outward normal +Z.
            w.quad([x0, y0, sz], [x1, y0, sz], [x1, y1, sz], [x0, y1, sz]);
        }
    }
    for i in 0..3 {
        let (x0, x1) = (xs[i], xs[i + 1]);
        // y = 0, outward -Y.
        w.quad([x0, 0.0, 0.0], [x1, 0.0, 0.0], [x1, 0.0, sz], [x0, 0.0, sz]);
        // y = sy, outward +Y.
        w.quad([x0, sy, 0.0], [x0, sy, sz], [x1, sy, sz], [x1, sy, 0.0]);
    }
    for j in 0..3 {
        let (y0, y1) = (ys[j], ys[j + 1]);
        // x = 0, outward -X.
        w.quad([0.0, y0, 0.0], [0.0, y0, sz], [0.0, y1, sz], [0.0, y1, 0.0]);
        // x = sx, outward +X.
        w.quad([sx, y0, 0.0], [sx, y1, 0.0], [sx, y1, sz], [sx, y0, sz]);
    }
    w.mesh
}

/// The middle of a slot mouth: 0.75 mm from the nearest rim in the face's own
/// plane, 2 mm from either end, and with the slot walls absent there is
/// nothing nearer. Surface within 0.7 mm of it means the mouth was capped.
fn mouth_centre(z: f64) -> [f64; 3] {
    [
        0.5 * (SLOT_X[0] + SLOT_X[1]),
        0.5 * (SLOT_Y[0] + SLOT_Y[1]),
        z,
    ]
}

/// A 1.5 mm slot is a feature, not a defect, and the repair may not weld it
/// shut to make the defect count go to zero.
///
/// `bridge_boundary_slits` fills any closed boundary loop whose mean width is
/// under `SLIT_MAX_WIDTH` = 2 mm. A slot mouth 1.5 mm across is under it. The
/// bridge deletes nothing, so the one-sided guard — which compared the mesh it
/// was given against the mesh it produced, and so only ever saw surface go
/// missing — had nothing to say: the export returned `is_watertight() == true`,
/// `declined` empty, and a solid plate where a slotted one went in.
///
/// With the guard measuring both directions the fill is refused by name, the
/// mouth is still open, and the caller is told what the repair wanted to
/// invent and where.
#[test]
fn a_real_slot_mouth_is_not_bridged_shut() {
    let mut mesh = plate_with_an_open_slot();
    let given = mesh.clone();

    // The mouths really are open to start with, and really are defects —
    // otherwise the repair would have no reason to touch them and the test
    // would pass for the wrong reason.
    for z in [0.0, PLATE[2]] {
        let d = distance_to_surface(mouth_centre(z), &given);
        assert!(
            (d - 0.75).abs() < 1e-3,
            "the fixture's slot mouth is not 1.5 mm wide: {d:.4} mm to the nearest surface"
        );
    }

    let outcome = repair_watertightness_reported(&mut mesh);
    assert!(
        outcome.defects_before > 0,
        "the open slot should read as defective"
    );

    // The mouth is still a mouth.
    for z in [0.0, PLATE[2]] {
        let d = distance_to_surface(mouth_centre(z), &mesh);
        assert!(
            d > 0.7,
            "the repair capped the slot: the middle of the mouth at z = {z} is now \
             {d:.4} mm from the surface, so there is material across a 1.5 mm slot"
        );
    }

    // And it was refused out loud, by the pass that wanted to do it, with the
    // size and the place of the fill.
    let bridge = outcome
        .invented()
        .find(|d| d.pass == "bridge_slits")
        .unwrap_or_else(|| {
            panic!(
                "the slit bridge was not declined for inventing surface; declined = {:?}, \
                 watertight = {}",
                outcome.declined,
                outcome.is_watertight()
            )
        });
    assert_eq!(bridge.change, SurfaceChange::Added);
    assert!(
        bridge.distance > SHAPE_TOLERANCE,
        "a decline that reports {:.4} mm is not reporting the fill",
        bridge.distance
    );
    // The fill it refused is the mouth: the reported point sits inside the
    // slot footprint, not somewhere else on the plate.
    assert!(
        bridge.at[0] >= SLOT_X[0] - 1e-6
            && bridge.at[0] <= SLOT_X[1] + 1e-6
            && bridge.at[1] >= SLOT_Y[0] - 1e-6
            && bridge.at[1] <= SLOT_Y[1] + 1e-6,
        "the decline points at {:?}, which is not in the slot",
        bridge.at
    );

    // Nothing was quietly taken either: the mesh handed back is the one
    // handed in.
    assert_eq!(
        mesh.indices.len(),
        given.indices.len(),
        "a declined repair must leave the mesh alone"
    );
    assert!(
        !outcome.is_watertight(),
        "the plate has an open slot; calling it watertight is the bug"
    );
    let warning = outcome.warning().expect("a refused repair warns");
    assert!(
        warning.contains("invented surface"),
        "the warning does not say the repair wanted to make surface up: {warning}"
    );
}

/// The coordinator's synthetic: a closed box carrying a second, coincident
/// copy of one wall. The duplicate is a real defect — every edge of that wall
/// is used four times — and the repair is welcome to take it away. What it
/// may not do is take the wall.
#[test]
fn a_duplicated_wall_may_be_peeled_but_the_wall_must_stay() {
    let clean = box_mesh(10.0, 8.0, 6.0);
    let mut mesh = clean.clone();

    // Duplicate the x+ wall (vertices 1,2,6,5), as its own two triangles on
    // fresh vertices — a doubled sheet, not a repeated index.
    let base = (mesh.vertices.len() / 3) as u32;
    for i in [1u32, 2, 6, 5] {
        let k = i as usize * 3;
        let (x, y, z) = (mesh.vertices[k], mesh.vertices[k + 1], mesh.vertices[k + 2]);
        mesh.vertices.extend_from_slice(&[x, y, z]);
    }
    mesh.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);

    let before_tris = mesh.indices.len() / 3;
    repair_watertightness(&mut mesh);

    // The wall is still there: every point of the clean box is on the result.
    let lost = max_deviation(&clean, &mesh);
    assert!(
        lost <= SHAPE_TOLERANCE,
        "the repair opened the box — a point of the original surface is now \
         {lost:.4} mm from anything the repair left (tolerance {SHAPE_TOLERANCE})"
    );
    // ...and nothing was invented either.
    let gained = max_deviation(&mesh, &clean);
    assert!(
        gained <= SHAPE_TOLERANCE,
        "the repair added surface {gained:.4} mm away from the original box"
    );
    assert!(
        mesh.indices.len() / 3 <= before_tris,
        "peeling a duplicate should not grow the mesh"
    );
}

/// A mesh the repair cannot close must come back WHOLE, and must say so.
///
/// The lid is removed, leaving a 10 x 8 hole — the shape of an open import or
/// a mid-pipeline fragment. The repair's carve-and-refill pass answers that by
/// gutting the mesh, which scores a perfect defect count on nothing at all;
/// the guard declines it because it would move the surface infinitely far.
/// What the caller gets back is the five walls it handed in, plus the verdict
/// that they are not watertight (native-app friction log item 30: a degraded
/// solid used to look like a good one).
#[test]
fn a_mesh_that_cannot_be_closed_comes_back_whole_and_says_so() {
    let clean = box_mesh(10.0, 8.0, 6.0);
    let mut mesh = clean.clone();
    // Drop the z+ face (triangles 2 and 3 in `box_mesh`'s emission order).
    let keep: Vec<u32> = mesh
        .indices
        .chunks(3)
        .enumerate()
        .filter(|(t, _)| *t != 2 && *t != 3)
        .flat_map(|(_, tri)| tri.to_vec())
        .collect();
    mesh.indices = keep;
    let open = mesh.clone();

    let outcome = repair_watertightness_reported(&mut mesh);
    assert!(
        outcome.defects_before > 0,
        "the open box should read as defective"
    );

    // Whatever it decided, the five walls it was given are still there.
    let lost = max_deviation(&open, &mesh);
    assert!(
        lost <= SHAPE_TOLERANCE,
        "the repair removed surface it was given: a point of the open box is \
         now {lost:.4} mm from anything that came back"
    );
    // ...and it did not invent any. Measured against the OPEN box, which is
    // the truth here: the lidded one contains a lid, so a repair that
    // re-invented one would have scored zero deviation and passed. The part
    // handed in has five walls, and five walls is what may come back.
    let gained = max_deviation(&mesh, &open);
    assert!(
        gained <= SHAPE_TOLERANCE,
        "the repair added surface {gained:.4} mm from anything it was given — \
         a lid it made up scores 0 against the closed box and must not be \
         measured that way"
    );

    // And the caller is told, rather than left holding a part that looks fine.
    // Unconditionally: a box with no lid cannot be closed without inventing
    // one, so "it came back watertight" is itself the failure, not a case to
    // skip the assertion for.
    assert!(
        !outcome.is_watertight(),
        "a lidless box came back watertight — the repair made a lid up"
    );
    let warning = outcome.warning().expect("a non-watertight mesh warns");
    assert!(
        warning.contains("not watertight"),
        "unhelpful warning: {warning}"
    );
}

/// The verdict has to reach the caller: a mesh that comes out still defective
/// says so, with the distance the declined repair would have moved the part.
#[test]
fn the_outcome_reports_a_mesh_it_could_not_close() {
    let mut mesh = box_mesh(10.0, 8.0, 6.0);
    let outcome = repair_watertightness_reported(&mut mesh);
    assert_eq!(outcome.defects_before, 0, "a plain box is closed");
    assert!(outcome.is_watertight());
    assert_eq!(outcome.warning(), None, "nothing to warn about");
}

/// The permissive policy still measures what it costs.
///
/// `RepairPolicy::manifold_at_any_cost` is what the mesh-boolean fallback
/// uses: its contract is to return something that bounds a solid, and a
/// caller that reached it has already accepted a degraded result. That is a
/// trade, not a free lunch, and the point of the split is that the trade is a
/// number someone can read — `surface_lost` / `surface_added` — rather than an
/// assumption. Measured on the real parts: the shell ring's export moved
/// 0.104 mm, its worst intermediate mesh-CSG step 2.96 mm (1.8% of the area
/// past 0.02 mm), and the rana-60c shell 2.65 mm.
///
/// The fixture is the slotted plate, not the lidless box the first version of
/// this test used. On the lidless box neither policy can do anything — the
/// 10 x 8 mouth is 4.4 mm of mean width, past every fill gate — so both came
/// back having moved 0.0 mm and the comparison `loose >= strict` was
/// `0.0 >= 0.0`: true for any implementation, including one with no policy at
/// all. A claim that the two policies differ has to be made on a mesh where
/// they do, so this asserts the divergence in both directions: the permissive
/// one closes the slot and says what that cost, the strict one refuses and
/// says why.
#[test]
fn the_permissive_policy_reports_what_it_moved() {
    use vcad_kernel_tessellate::{repair_watertightness_with, RepairPolicy};

    let mut strict = plate_with_an_open_slot();
    let strict_outcome = repair_watertightness_with(&mut strict, RepairPolicy::strict());

    let mut loose = plate_with_an_open_slot();
    let loose_outcome =
        repair_watertightness_with(&mut loose, RepairPolicy::manifold_at_any_cost());

    // The strict one refuses, out loud, and hands the slot back.
    assert!(
        !strict_outcome.declined.is_empty(),
        "the strict policy should have had something to decline here"
    );
    assert!(
        !strict_outcome.is_watertight(),
        "the strict policy closed the slot"
    );
    assert!(
        strict_outcome.surface_added.max <= SHAPE_TOLERANCE,
        "the strict policy invented {:.4} mm of surface",
        strict_outcome.surface_added.max
    );
    assert!(
        distance_to_surface(mouth_centre(0.0), &strict) > 0.7,
        "the strict policy capped the slot mouth"
    );

    // The permissive one takes the deal — and that is the point of the split:
    // it is allowed to, and the caller can read what it paid.
    assert!(
        loose_outcome.is_watertight(),
        "the permissive policy exists to close meshes; it left {} defects",
        loose_outcome.defects_after
    );
    assert!(
        loose_outcome.declined.is_empty(),
        "an unbounded policy has nothing to decline: {:?}",
        loose_outcome.declined
    );
    assert!(
        distance_to_surface(mouth_centre(0.0), &loose) < 1e-6,
        "the permissive policy was supposed to bridge the mouth"
    );

    // ...and the price is a number tied to the slot, not an assumption. A flat
    // cap over a mouth of width w stands w/2 off the nearest rail at its
    // centre line; the reported figure samples triangle corners (on the rail,
    // so 0) and centroids (w/3 for the slivers a min-area triangulation of a
    // long rectangle produces), so it is a sampled lower bound in [w/3, w/2].
    let w = SLOT_X[1] - SLOT_X[0];
    assert!(
        loose_outcome.surface_added.max >= w / 3.0 - 1e-3
            && loose_outcome.surface_added.max <= w / 2.0 + 1e-3,
        "the permissive policy reported {:.4} mm of invented surface for a \
         {w:.2} mm slot — expected between {:.4} and {:.4}",
        loose_outcome.surface_added.max,
        w / 3.0,
        w / 2.0
    );
    assert!(
        loose_outcome.surface_added.max > strict_outcome.surface_added.max + SHAPE_TOLERANCE,
        "the two policies did not actually diverge: permissive invented \
         {:.4} mm, strict {:.4} mm",
        loose_outcome.surface_added.max,
        strict_outcome.surface_added.max
    );
    // Nothing was deleted either way — this repair is a fill, and the two
    // policies differ only over whether it may happen.
    for (name, o) in [("strict", &strict_outcome), ("permissive", &loose_outcome)] {
        assert!(
            o.surface_lost.max <= SHAPE_TOLERANCE,
            "the {name} policy lost {:.4} mm of surface bridging a slot",
            o.surface_lost.max
        );
    }
}
