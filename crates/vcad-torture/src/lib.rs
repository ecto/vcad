//! Adversarial robustness "torture track" for the vcad kernel.
//!
//! A large, fully deterministic corpus of adversarial cases — coincident and
//! tangent boolean configurations, near-degenerate slivers, chained curved
//! booleans, seeded random primitive pairs, STEP round-trips, and
//! tessellation watertightness — plus the classification logic that grades
//! each case as pass / graceful-refusal / bad-geometry / crash / timeout.
//!
//! The corpus is generated from constants and a seeded splitmix64 PRNG, so
//! every run (local or CI) sees byte-identical cases. The `vcad-torture`
//! binary orchestrates execution with per-case subprocess isolation.

#![warn(missing_docs)]

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use vcad_kernel::Solid;

/// Corpus category, used for scoreboard grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    /// Booleans between solids sharing coincident faces/edges/vertices,
    /// including epsilon-perturbed near-coincidence.
    BooleanCoincident,
    /// Booleans between tangent curved surfaces.
    BooleanTangent,
    /// Booleans producing near-degenerate sliver overlaps.
    BooleanSliver,
    /// Chains of sequential booleans over curved solids.
    BooleanChain,
    /// Seeded random primitive pairs at controlled poses.
    BooleanRandom,
    /// STEP export → import round-trips with volume comparison.
    StepRoundtrip,
    /// Tessellation watertightness at extreme parameters.
    Tessellation,
}

impl Category {
    /// All categories in scoreboard order.
    pub const ALL: [Category; 7] = [
        Category::BooleanCoincident,
        Category::BooleanTangent,
        Category::BooleanSliver,
        Category::BooleanChain,
        Category::BooleanRandom,
        Category::StepRoundtrip,
        Category::Tessellation,
    ];

    /// Kebab-case name (matches the serde encoding).
    pub fn name(&self) -> &'static str {
        match self {
            Category::BooleanCoincident => "boolean-coincident",
            Category::BooleanTangent => "boolean-tangent",
            Category::BooleanSliver => "boolean-sliver",
            Category::BooleanChain => "boolean-chain",
            Category::BooleanRandom => "boolean-random",
            Category::StepRoundtrip => "step-roundtrip",
            Category::Tessellation => "tessellation",
        }
    }
}

/// A primitive solid specification (pure data, so cases are serializable
/// and reproducible in a child process).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum PrimSpec {
    /// Axis-aligned cube with corner at origin.
    Cube(f64, f64, f64),
    /// Cylinder along +Z, base at origin.
    Cylinder(f64, f64),
    /// Sphere centered at origin.
    Sphere(f64),
    /// Cone along +Z (bottom radius, top radius, height).
    Cone(f64, f64, f64),
    /// Torus in the XY plane (major, minor radius).
    Torus(f64, f64),
    /// Regular prism (sides, circumradius, height).
    Prism(u32, f64, f64),
    /// Wedge (box cut diagonally).
    Wedge(f64, f64, f64),
}

impl PrimSpec {
    /// Build the solid.
    pub fn build(&self) -> Solid {
        const SEGS: u32 = 32;
        match *self {
            PrimSpec::Cube(x, y, z) => Solid::cube(x, y, z),
            PrimSpec::Cylinder(r, h) => Solid::cylinder(r, h, SEGS),
            PrimSpec::Sphere(r) => Solid::sphere(r, SEGS),
            PrimSpec::Cone(rb, rt, h) => Solid::cone(rb, rt, h, SEGS),
            PrimSpec::Torus(mj, mn) => Solid::torus(mj, mn, SEGS),
            PrimSpec::Prism(n, r, h) => Solid::prism(n, r, h),
            PrimSpec::Wedge(x, y, z) => Solid::wedge(x, y, z),
        }
    }
}

/// Boolean operation selector (mirrors the kernel's, but serializable).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Op {
    /// Union.
    Union,
    /// Difference (a − b).
    Difference,
    /// Intersection.
    Intersection,
}

impl Op {
    /// All three ops.
    pub const ALL: [Op; 3] = [Op::Union, Op::Difference, Op::Intersection];

    /// Short tag used in case ids.
    pub fn tag(&self) -> &'static str {
        match self {
            Op::Union => "u",
            Op::Difference => "d",
            Op::Intersection => "i",
        }
    }
}

/// Rigid pose applied to the second operand: translation then rotation
/// (degrees, applied about X/Y/Z before the translation).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Pose {
    /// Rotation in degrees about X, Y, Z (applied first).
    pub rot_deg: [f64; 3],
    /// Translation, applied after rotation.
    pub translate: [f64; 3],
}

impl Pose {
    /// Identity pose.
    pub fn identity() -> Self {
        Pose {
            rot_deg: [0.0; 3],
            translate: [0.0; 3],
        }
    }

    /// Pure translation.
    pub fn at(x: f64, y: f64, z: f64) -> Self {
        Pose {
            rot_deg: [0.0; 3],
            translate: [x, y, z],
        }
    }

    /// Apply to a solid.
    pub fn apply(&self, s: &Solid) -> Solid {
        let r = if self.rot_deg == [0.0; 3] {
            s.clone()
        } else {
            s.rotate(self.rot_deg[0], self.rot_deg[1], self.rot_deg[2])
        };
        if self.translate == [0.0; 3] {
            r
        } else {
            r.translate(self.translate[0], self.translate[1], self.translate[2])
        }
    }
}

/// What a case actually does.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CaseKind {
    /// One boolean between two posed primitives.
    BoolPair {
        /// First operand (identity pose).
        a: PrimSpec,
        /// Second operand.
        b: PrimSpec,
        /// Pose of the second operand.
        pose_b: Pose,
        /// The boolean operation.
        op: Op,
    },
    /// A chain of booleans applied sequentially to a base solid.
    Chain {
        /// Base solid (identity pose).
        base: PrimSpec,
        /// Sequential (tool, pose, op) steps.
        steps: Vec<(PrimSpec, Pose, Op)>,
    },
    /// STEP round-trip of a solid (optionally a boolean result).
    StepRoundtrip {
        /// The solid to round-trip.
        a: PrimSpec,
        /// Optional boolean applied before export.
        bool_with: Option<(PrimSpec, Pose, Op)>,
    },
    /// Tessellate at a given segment count and check watertightness.
    Tessellate {
        /// The solid to tessellate.
        a: PrimSpec,
        /// Segment count.
        segments: u32,
    },
}

/// A single corpus case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Case {
    /// Stable unique id (baseline key — never reuse an id for different
    /// geometry).
    pub id: String,
    /// Scoreboard category.
    pub category: Category,
    /// Included in the fast PR subset?
    pub pr_subset: bool,
    /// What to run.
    pub kind: CaseKind,
}

/// Classification of a case result. Ordered from best to worst — a class
/// with a higher [`Class::rank`] than the baseline is a regression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Class {
    /// Operation succeeded and the result geometry is sane.
    Pass,
    /// Operation returned a structured error (fail-closed refusal).
    GracefulRefusal,
    /// Operation "succeeded" but produced non-watertight or volume-insane
    /// geometry.
    BadGeometry,
    /// Case exceeded the per-case timeout.
    Timeout,
    /// Panic or abort.
    Crash,
}

impl Class {
    /// Severity rank (0 best). Regressions are rank increases.
    pub fn rank(&self) -> u8 {
        match self {
            Class::Pass => 0,
            Class::GracefulRefusal => 1,
            Class::BadGeometry => 2,
            Class::Timeout => 3,
            Class::Crash => 4,
        }
    }

    /// Kebab-case name (matches serde encoding).
    pub fn name(&self) -> &'static str {
        match self {
            Class::Pass => "pass",
            Class::GracefulRefusal => "graceful-refusal",
            Class::BadGeometry => "bad-geometry",
            Class::Timeout => "timeout",
            Class::Crash => "crash",
        }
    }
}

/// Result of executing a single case (as reported by the child process).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseResult {
    /// Case id.
    pub id: String,
    /// Classification.
    pub class: Class,
    /// Human-readable detail (error text, check failure, …).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

// ---------------------------------------------------------------------------
// Deterministic PRNG
// ---------------------------------------------------------------------------

/// splitmix64 — tiny, deterministic, dependency-free PRNG for corpus
/// generation. Never seeded from time; the corpus is identical everywhere.
pub struct Rng(u64);

impl Rng {
    /// Create from a fixed seed.
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    /// Next raw u64.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform f64 in [0, 1).
    pub fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform f64 in [lo, hi).
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + self.f64() * (hi - lo)
    }

    /// Uniform usize in [0, n).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

// ---------------------------------------------------------------------------
// Corpus generation
// ---------------------------------------------------------------------------

/// Coincidence offsets swept in the structured coincident family: exact
/// coincidence plus perturbations from double-precision noise up to a
/// visible-but-tricky 1e-3 mm.
const EPS_OFFSETS: [f64; 5] = [0.0, 1e-12, 1e-9, 1e-6, 1e-3];

/// Build the full deterministic corpus.
pub fn build_corpus() -> Vec<Case> {
    let mut cases = Vec::new();
    coincident_cases(&mut cases);
    tangent_cases(&mut cases);
    sliver_cases(&mut cases);
    chain_cases(&mut cases);
    random_cases(&mut cases);
    step_cases(&mut cases);
    tessellation_cases(&mut cases);
    // Ids must be unique — the baseline is keyed on them.
    let mut seen = std::collections::BTreeSet::new();
    for c in &cases {
        assert!(seen.insert(c.id.clone()), "duplicate case id {}", c.id);
    }
    cases
}

fn coincident_cases(out: &mut Vec<Case>) {
    // Cube–cube contact configurations: shared face, shared edge, shared
    // vertex, half-overlapped face, fully coincident. Each swept across
    // epsilon offsets along X and all three ops.
    let configs: [(&str, [f64; 3]); 5] = [
        ("face", [10.0, 0.0, 0.0]),
        ("edge", [10.0, 10.0, 0.0]),
        ("vertex", [10.0, 10.0, 10.0]),
        ("halfface", [10.0, 5.0, 0.0]),
        ("identical", [0.0, 0.0, 0.0]),
    ];
    for (name, base) in configs {
        for (ei, eps) in EPS_OFFSETS.iter().enumerate() {
            for op in Op::ALL {
                // Pull the tool *toward* overlap by eps so eps=0 is exact
                // coincidence and eps>0 is a sliver of penetration.
                let t = [base[0] - eps, base[1], base[2]];
                out.push(Case {
                    id: format!("coin-cube-{name}-e{ei}-{}", op.tag()),
                    category: Category::BooleanCoincident,
                    pr_subset: true,
                    kind: CaseKind::BoolPair {
                        a: PrimSpec::Cube(10.0, 10.0, 10.0),
                        b: PrimSpec::Cube(10.0, 10.0, 10.0),
                        pose_b: Pose::at(t[0], t[1], t[2]),
                        op,
                    },
                });
            }
        }
    }
    // Cylinder stacked exactly on a cylinder (coincident circular faces),
    // coaxial and epsilon-shifted.
    for (ei, eps) in EPS_OFFSETS.iter().enumerate() {
        for op in Op::ALL {
            out.push(Case {
                id: format!("coin-cyl-stack-e{ei}-{}", op.tag()),
                category: Category::BooleanCoincident,
                pr_subset: true,
                kind: CaseKind::BoolPair {
                    a: PrimSpec::Cylinder(5.0, 10.0),
                    b: PrimSpec::Cylinder(5.0, 10.0),
                    pose_b: Pose::at(0.0, 0.0, 10.0 - eps),
                    op,
                },
            });
        }
    }
    // Cube face coincident with a cylinder cap.
    for op in Op::ALL {
        out.push(Case {
            id: format!("coin-cube-cylcap-{}", op.tag()),
            category: Category::BooleanCoincident,
            pr_subset: true,
            kind: CaseKind::BoolPair {
                a: PrimSpec::Cube(10.0, 10.0, 10.0),
                b: PrimSpec::Cylinder(3.0, 10.0),
                pose_b: Pose::at(5.0, 5.0, 10.0),
                op,
            },
        });
    }
}

fn tangent_cases(out: &mut Vec<Case>) {
    // Sphere tangent to a cube face (internally and externally).
    for (ei, eps) in EPS_OFFSETS.iter().enumerate() {
        for op in Op::ALL {
            out.push(Case {
                id: format!("tan-sphere-cubeface-e{ei}-{}", op.tag()),
                category: Category::BooleanTangent,
                pr_subset: true,
                kind: CaseKind::BoolPair {
                    a: PrimSpec::Cube(10.0, 10.0, 10.0),
                    b: PrimSpec::Sphere(4.0),
                    // Center sits on the top face plane minus eps: tangency
                    // from outside drifting into penetration.
                    pose_b: Pose::at(5.0, 5.0, 14.0 - eps),
                    op,
                },
            });
        }
    }
    // Two spheres externally tangent.
    for (ei, eps) in EPS_OFFSETS.iter().enumerate() {
        for op in Op::ALL {
            out.push(Case {
                id: format!("tan-sphere-sphere-e{ei}-{}", op.tag()),
                category: Category::BooleanTangent,
                pr_subset: true,
                kind: CaseKind::BoolPair {
                    a: PrimSpec::Sphere(5.0),
                    b: PrimSpec::Sphere(5.0),
                    pose_b: Pose::at(10.0 - eps, 0.0, 0.0),
                    op,
                },
            });
        }
    }
    // Parallel cylinders tangent along a line.
    for (ei, eps) in EPS_OFFSETS.iter().enumerate() {
        for op in Op::ALL {
            out.push(Case {
                id: format!("tan-cyl-cyl-parallel-e{ei}-{}", op.tag()),
                category: Category::BooleanTangent,
                pr_subset: true,
                kind: CaseKind::BoolPair {
                    a: PrimSpec::Cylinder(5.0, 10.0),
                    b: PrimSpec::Cylinder(5.0, 10.0),
                    pose_b: Pose::at(10.0 - eps, 0.0, 0.0),
                    op,
                },
            });
        }
    }
    // Cylinder inscribed in a cube, tangent to all four side faces.
    for op in Op::ALL {
        out.push(Case {
            id: format!("tan-cyl-inscribed-{}", op.tag()),
            category: Category::BooleanTangent,
            pr_subset: true,
            kind: CaseKind::BoolPair {
                a: PrimSpec::Cube(10.0, 10.0, 10.0),
                b: PrimSpec::Cylinder(5.0, 12.0),
                pose_b: Pose::at(5.0, 5.0, -1.0),
                op,
            },
        });
    }
    // Sphere inscribed in a cube.
    for op in Op::ALL {
        out.push(Case {
            id: format!("tan-sphere-inscribed-{}", op.tag()),
            category: Category::BooleanTangent,
            pr_subset: true,
            kind: CaseKind::BoolPair {
                a: PrimSpec::Cube(10.0, 10.0, 10.0),
                b: PrimSpec::Sphere(5.0),
                pose_b: Pose::at(5.0, 5.0, 5.0),
                op,
            },
        });
    }
    // Torus tangent to a plane (cube top face).
    for op in Op::ALL {
        out.push(Case {
            id: format!("tan-torus-cubeface-{}", op.tag()),
            category: Category::BooleanTangent,
            pr_subset: true,
            kind: CaseKind::BoolPair {
                a: PrimSpec::Cube(20.0, 20.0, 10.0),
                b: PrimSpec::Torus(6.0, 2.0),
                pose_b: Pose::at(10.0, 10.0, 12.0),
                op,
            },
        });
    }
    // Equal-radius perpendicular cylinders (Steinmetz).
    for op in Op::ALL {
        out.push(Case {
            id: format!("tan-steinmetz-{}", op.tag()),
            category: Category::BooleanTangent,
            pr_subset: true,
            kind: CaseKind::BoolPair {
                a: PrimSpec::Cylinder(5.0, 20.0),
                b: PrimSpec::Cylinder(5.0, 20.0),
                pose_b: Pose {
                    rot_deg: [90.0, 0.0, 0.0],
                    translate: [0.0, -10.0, 10.0],
                },
                op,
            },
        });
    }
}

fn sliver_cases(out: &mut Vec<Case>) {
    // Cube overlapping a cube by a progressively thinner sliver.
    let thicknesses = [1e-8, 1e-6, 1e-4, 1e-2, 0.1];
    for (ti, t) in thicknesses.iter().enumerate() {
        for op in Op::ALL {
            out.push(Case {
                id: format!("sliv-cube-overlap-t{ti}-{}", op.tag()),
                category: Category::BooleanSliver,
                pr_subset: true,
                kind: CaseKind::BoolPair {
                    a: PrimSpec::Cube(10.0, 10.0, 10.0),
                    b: PrimSpec::Cube(10.0, 10.0, 10.0),
                    pose_b: Pose::at(10.0 - t, 0.0, 0.0),
                    op,
                },
            });
        }
    }
    // Cylinder cutting a sliver off a cube edge.
    for (ti, t) in thicknesses.iter().enumerate() {
        for op in Op::ALL {
            out.push(Case {
                id: format!("sliv-cyl-graze-t{ti}-{}", op.tag()),
                category: Category::BooleanSliver,
                pr_subset: true,
                kind: CaseKind::BoolPair {
                    a: PrimSpec::Cube(10.0, 10.0, 10.0),
                    b: PrimSpec::Cylinder(5.0, 12.0),
                    pose_b: Pose::at(15.0 - t, 5.0, -1.0),
                    op,
                },
            });
        }
    }
    // Extremely thin plate minus a cylinder (thin remaining walls).
    for (ti, t) in [1e-4, 1e-2, 0.1].iter().enumerate() {
        out.push(Case {
            id: format!("sliv-thin-plate-t{ti}-d"),
            category: Category::BooleanSliver,
            pr_subset: true,
            kind: CaseKind::BoolPair {
                a: PrimSpec::Cube(20.0, 20.0, *t),
                b: PrimSpec::Cylinder(5.0, 1.0),
                pose_b: Pose::at(10.0, 10.0, -0.5),
                op: Op::Difference,
            },
        });
    }
}

fn chain_cases(out: &mut Vec<Case>) {
    // Chained curved booleans — the regression family from the torr boolean
    // catalogue: each additional curved boolean re-intersects surfaces
    // produced by the previous one.
    let mut rng = Rng::new(0xC4A1);
    for ci in 0..24 {
        let base = PrimSpec::Cube(30.0, 30.0, 30.0);
        let n_steps = 3 + rng.below(3); // 3..=5
        let mut steps = Vec::new();
        for _ in 0..n_steps {
            let tool = match rng.below(3) {
                0 => PrimSpec::Cylinder(rng.range(2.0, 8.0), 40.0),
                1 => PrimSpec::Sphere(rng.range(3.0, 10.0)),
                _ => PrimSpec::Cone(rng.range(2.0, 8.0), rng.range(0.5, 2.0), 40.0),
            };
            let pose = Pose {
                rot_deg: [[0.0, 90.0][rng.below(2)], 0.0, rng.range(0.0, 90.0).round()],
                translate: [
                    rng.range(2.0, 28.0).round(),
                    rng.range(2.0, 28.0).round(),
                    rng.range(-5.0, 20.0).round(),
                ],
            };
            let op = if rng.below(4) == 0 {
                Op::Union
            } else {
                Op::Difference
            };
            steps.push((tool, pose, op));
        }
        out.push(Case {
            id: format!("chain-{ci:02}"),
            category: Category::BooleanChain,
            pr_subset: ci < 12,
            kind: CaseKind::Chain { base, steps },
        });
    }
}

fn random_cases(out: &mut Vec<Case>) {
    // Seeded random primitive pairs at controlled poses. Four pose modes:
    // generic overlap, axis-aligned tangency, coincident alignment, and
    // epsilon-offset from tangency.
    let mut rng = Rng::new(0x7047_u64);
    for i in 0..400 {
        let a = random_prim(&mut rng);
        let b = random_prim(&mut rng);
        let mode = i % 4;
        let (ra, _) = prim_extent(&a);
        let (rb, _) = prim_extent(&b);
        let pose_b = match mode {
            // Generic overlap: tool center lands inside a's extent.
            0 => Pose {
                rot_deg: [
                    rng.range(0.0, 360.0).round(),
                    rng.range(0.0, 360.0).round(),
                    rng.range(0.0, 360.0).round(),
                ],
                translate: [rng.range(-ra, ra), rng.range(-ra, ra), rng.range(-ra, ra)],
            },
            // Exact axis tangency along X.
            1 => Pose::at(ra + rb, 0.0, 0.0),
            // Coincident alignment (identical origin, axis-aligned).
            2 => Pose::identity(),
            // Epsilon offset from tangency.
            _ => {
                let eps = EPS_OFFSETS[1 + rng.below(EPS_OFFSETS.len() - 1)];
                Pose::at(ra + rb - eps, 0.0, 0.0)
            }
        };
        let op = Op::ALL[rng.below(3)];
        out.push(Case {
            id: format!("rand-{i:03}"),
            category: Category::BooleanRandom,
            pr_subset: i < 100,
            kind: CaseKind::BoolPair { a, b, pose_b, op },
        });
    }
}

/// Draw a random primitive with sane-but-varied proportions.
fn random_prim(rng: &mut Rng) -> PrimSpec {
    match rng.below(6) {
        0 => PrimSpec::Cube(
            rng.range(1.0, 20.0),
            rng.range(1.0, 20.0),
            rng.range(0.1, 20.0),
        ),
        1 => PrimSpec::Cylinder(rng.range(0.5, 10.0), rng.range(0.5, 20.0)),
        2 => PrimSpec::Sphere(rng.range(0.5, 10.0)),
        3 => PrimSpec::Cone(
            rng.range(0.5, 10.0),
            rng.range(0.1, 5.0),
            rng.range(1.0, 20.0),
        ),
        4 => {
            let minor = rng.range(0.3, 3.0);
            PrimSpec::Torus(minor + rng.range(1.0, 8.0), minor)
        }
        _ => PrimSpec::Prism(
            3 + rng.below(6) as u32,
            rng.range(1.0, 10.0),
            rng.range(1.0, 20.0),
        ),
    }
}

/// Rough half-extent (radius) and center-height of a primitive, used to
/// construct tangent poses.
fn prim_extent(p: &PrimSpec) -> (f64, f64) {
    match *p {
        PrimSpec::Cube(x, _, _) => (x / 2.0, 0.0),
        PrimSpec::Cylinder(r, _) => (r, 0.0),
        PrimSpec::Sphere(r) => (r, 0.0),
        PrimSpec::Cone(rb, rt, _) => (rb.max(rt), 0.0),
        PrimSpec::Torus(mj, mn) => (mj + mn, 0.0),
        PrimSpec::Prism(_, r, _) => (r, 0.0),
        PrimSpec::Wedge(x, _, _) => (x / 2.0, 0.0),
    }
}

fn step_cases(out: &mut Vec<Case>) {
    // Plain primitives with varied (including extreme) parameters.
    let prims: Vec<(&str, PrimSpec)> = vec![
        ("cube", PrimSpec::Cube(10.0, 10.0, 10.0)),
        ("cube-flat", PrimSpec::Cube(100.0, 100.0, 0.01)),
        ("cube-needle", PrimSpec::Cube(0.01, 0.01, 100.0)),
        ("cube-tiny", PrimSpec::Cube(1e-3, 1e-3, 1e-3)),
        ("cube-huge", PrimSpec::Cube(1e5, 1e5, 1e5)),
        ("cyl", PrimSpec::Cylinder(5.0, 10.0)),
        ("cyl-flat", PrimSpec::Cylinder(50.0, 0.01)),
        ("cyl-needle", PrimSpec::Cylinder(0.01, 100.0)),
        ("sphere", PrimSpec::Sphere(5.0)),
        ("sphere-tiny", PrimSpec::Sphere(1e-3)),
        ("sphere-huge", PrimSpec::Sphere(1e5)),
        ("cone", PrimSpec::Cone(5.0, 2.0, 10.0)),
        ("cone-sharp", PrimSpec::Cone(5.0, 1e-6, 10.0)),
        ("torus", PrimSpec::Torus(6.0, 2.0)),
        ("torus-thin", PrimSpec::Torus(10.0, 0.05)),
        ("prism3", PrimSpec::Prism(3, 5.0, 10.0)),
        ("prism12", PrimSpec::Prism(12, 5.0, 10.0)),
        ("wedge", PrimSpec::Wedge(10.0, 10.0, 10.0)),
    ];
    for (name, p) in &prims {
        out.push(Case {
            id: format!("step-prim-{name}"),
            category: Category::StepRoundtrip,
            pr_subset: true,
            kind: CaseKind::StepRoundtrip {
                a: *p,
                bool_with: None,
            },
        });
    }
    // Boolean results round-tripped: seeded pairs biased toward planar +
    // curved combinations.
    let mut rng = Rng::new(0x57E9);
    for i in 0..42 {
        let a = random_prim(&mut rng);
        let b = random_prim(&mut rng);
        let (ra, _) = prim_extent(&a);
        let op = Op::ALL[rng.below(3)];
        out.push(Case {
            id: format!("step-bool-{i:02}"),
            category: Category::StepRoundtrip,
            pr_subset: i < 15,
            kind: CaseKind::StepRoundtrip {
                a,
                bool_with: Some((
                    b,
                    Pose::at(rng.range(0.0, ra), rng.range(0.0, ra), rng.range(0.0, ra)),
                    op,
                )),
            },
        });
    }
}

fn tessellation_cases(out: &mut Vec<Case>) {
    let prims: Vec<(&str, PrimSpec)> = vec![
        ("cube", PrimSpec::Cube(10.0, 10.0, 10.0)),
        ("cube-flat", PrimSpec::Cube(1000.0, 1000.0, 1e-3)),
        ("cube-needle", PrimSpec::Cube(1e-3, 1e-3, 1000.0)),
        ("cyl", PrimSpec::Cylinder(5.0, 10.0)),
        ("cyl-flat", PrimSpec::Cylinder(500.0, 1e-3)),
        ("cyl-needle", PrimSpec::Cylinder(1e-3, 1000.0)),
        ("sphere", PrimSpec::Sphere(5.0)),
        ("sphere-tiny", PrimSpec::Sphere(1e-4)),
        ("sphere-huge", PrimSpec::Sphere(1e6)),
        ("cone", PrimSpec::Cone(5.0, 2.0, 10.0)),
        ("cone-sharp", PrimSpec::Cone(5.0, 0.0, 10.0)),
        ("cone-inverted", PrimSpec::Cone(2.0, 5.0, 10.0)),
        ("torus", PrimSpec::Torus(6.0, 2.0)),
        ("torus-fat", PrimSpec::Torus(5.0, 4.999)),
        ("prism3", PrimSpec::Prism(3, 5.0, 10.0)),
        ("prism64", PrimSpec::Prism(64, 5.0, 10.0)),
        ("wedge", PrimSpec::Wedge(10.0, 10.0, 10.0)),
    ];
    let seg_counts = [3u32, 4, 8, 64, 256];
    for (name, p) in &prims {
        for segs in seg_counts {
            out.push(Case {
                id: format!("tess-{name}-s{segs}"),
                category: Category::Tessellation,
                pr_subset: segs == 8 || segs == 3,
                kind: CaseKind::Tessellate {
                    a: *p,
                    segments: segs,
                },
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Execution + classification
// ---------------------------------------------------------------------------

/// Tessellation density used for geometry checks.
const CHECK_SEGS: u32 = 32;

/// Relative volume tolerance for boolean bound checks. Loose on purpose:
/// tessellated volumes of curved solids differ from analytic ones, and the
/// track polices sanity, not accuracy.
const VOL_RTOL: f64 = 0.05;
/// Absolute volume slack (mm³) so near-zero expected volumes don't flake.
const VOL_ATOL: f64 = 1e-6;

/// Execute a single case in-process. Panics propagate to the caller (the
/// binary isolates them per-subprocess).
pub fn execute_case(case: &Case) -> CaseResult {
    let (class, detail) = match &case.kind {
        CaseKind::BoolPair { a, b, pose_b, op } => run_bool_pair(a, b, pose_b, *op),
        CaseKind::Chain { base, steps } => run_chain(base, steps),
        CaseKind::StepRoundtrip { a, bool_with } => run_step_roundtrip(a, bool_with),
        CaseKind::Tessellate { a, segments } => run_tessellate(a, *segments),
    };
    CaseResult {
        id: case.id.clone(),
        class,
        detail,
    }
}

fn apply_op(a: &Solid, b: &Solid, op: Op) -> Result<Solid, String> {
    match op {
        Op::Union => a.try_union(b),
        Op::Difference => a.try_difference(b),
        Op::Intersection => a.try_intersection(b),
    }
    .map_err(|e| e.to_string())
}

/// Watertightness + volume-bound checks on a boolean result.
fn check_bool_result(result: &Solid, va: f64, vb: f64, op: Op) -> Result<(), String> {
    let mesh = result.to_mesh(CHECK_SEGS);
    if mesh.num_triangles() > 0 {
        let open = mesh.boundary_edges().len();
        if open > 0 {
            return Err(format!("result mesh has {open} open boundary edges"));
        }
    }
    let v = result.volume();
    if !v.is_finite() {
        return Err(format!("result volume is not finite ({v})"));
    }
    let slack = VOL_RTOL * (va + vb) + VOL_ATOL;
    let (lo, hi) = match op {
        Op::Union => (va.max(vb) - slack, va + vb + slack),
        Op::Difference => (-slack, va + slack),
        Op::Intersection => (-slack, va.min(vb) + slack),
    };
    if v < lo || v > hi {
        return Err(format!(
            "volume {v:.6} outside sane bounds [{lo:.6}, {hi:.6}] for {op:?} (va={va:.6}, vb={vb:.6})"
        ));
    }
    Ok(())
}

fn run_bool_pair(a: &PrimSpec, b: &PrimSpec, pose_b: &Pose, op: Op) -> (Class, String) {
    let sa = a.build();
    let sb = pose_b.apply(&b.build());
    let (va, vb) = (sa.volume(), sb.volume());
    match apply_op(&sa, &sb, op) {
        Err(e) => (Class::GracefulRefusal, e),
        Ok(result) => match check_bool_result(&result, va, vb, op) {
            Ok(()) => (Class::Pass, String::new()),
            Err(e) => (Class::BadGeometry, e),
        },
    }
}

fn run_chain(base: &PrimSpec, steps: &[(PrimSpec, Pose, Op)]) -> (Class, String) {
    let mut acc = base.build();
    let mut refusals = Vec::new();
    for (i, (tool, pose, op)) in steps.iter().enumerate() {
        let t = pose.apply(&tool.build());
        let (va, vb) = (acc.volume(), t.volume());
        match apply_op(&acc, &t, *op) {
            Err(e) => {
                refusals.push(format!("step {i}: {e}"));
                // A refused step leaves the accumulator unchanged — keep
                // torturing the remaining steps against it.
            }
            Ok(next) => {
                if let Err(e) = check_bool_result(&next, va, vb, *op) {
                    return (Class::BadGeometry, format!("step {i}: {e}"));
                }
                acc = next;
            }
        }
    }
    if refusals.is_empty() {
        (Class::Pass, String::new())
    } else {
        (Class::GracefulRefusal, refusals.join("; "))
    }
}

fn run_step_roundtrip(a: &PrimSpec, bool_with: &Option<(PrimSpec, Pose, Op)>) -> (Class, String) {
    let mut solid = a.build();
    if let Some((b, pose, op)) = bool_with {
        let t = pose.apply(&b.build());
        match apply_op(&solid, &t, *op) {
            Err(e) => {
                return (
                    Class::GracefulRefusal,
                    format!("boolean before export: {e}"),
                )
            }
            Ok(s) => solid = s,
        }
    }
    if !solid.can_export_step() {
        return (
            Class::GracefulRefusal,
            "no B-rep available for STEP export".into(),
        );
    }
    let buf = match solid.to_step_buffer() {
        Err(e) => return (Class::GracefulRefusal, format!("STEP export refused: {e}")),
        Ok(b) => b,
    };
    let reread = match Solid::from_step_buffer(&buf) {
        // Failing to re-read our own output is a defect, not a refusal.
        Err(e) => {
            return (
                Class::BadGeometry,
                format!("re-import of own STEP failed: {e}"),
            )
        }
        Ok(s) => s,
    };
    let (v0, v1) = (solid.volume(), reread.volume());
    if !v1.is_finite() {
        return (
            Class::BadGeometry,
            format!("round-trip volume not finite ({v1})"),
        );
    }
    let tol = 0.01 * v0.abs() + VOL_ATOL;
    if (v1 - v0).abs() > tol {
        return (
            Class::BadGeometry,
            format!("round-trip volume drift: {v0:.6} → {v1:.6}"),
        );
    }
    (Class::Pass, String::new())
}

fn run_tessellate(a: &PrimSpec, segments: u32) -> (Class, String) {
    let solid = a.build();
    let mesh = solid.to_mesh(segments);
    if mesh.num_triangles() == 0 {
        return (Class::BadGeometry, "empty tessellation".into());
    }
    let open = mesh.boundary_edges().len();
    if open > 0 {
        return (
            Class::BadGeometry,
            format!("tessellation has {open} open boundary edges"),
        );
    }
    for p in &mesh.vertices {
        if !p.is_finite() {
            return (Class::BadGeometry, "non-finite vertex position".into());
        }
    }
    (Class::Pass, String::new())
}

// ---------------------------------------------------------------------------
// Scorecard
// ---------------------------------------------------------------------------

/// Per-category tallies.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CategoryScore {
    /// Cases that passed.
    pub pass: usize,
    /// Structured refusals.
    pub graceful_refusal: usize,
    /// Bad geometry.
    pub bad_geometry: usize,
    /// Timeouts.
    pub timeout: usize,
    /// Crashes.
    pub crash: usize,
}

impl CategoryScore {
    fn add(&mut self, class: Class) {
        match class {
            Class::Pass => self.pass += 1,
            Class::GracefulRefusal => self.graceful_refusal += 1,
            Class::BadGeometry => self.bad_geometry += 1,
            Class::Timeout => self.timeout += 1,
            Class::Crash => self.crash += 1,
        }
    }

    /// Total cases in this category.
    pub fn total(&self) -> usize {
        self.pass + self.graceful_refusal + self.bad_geometry + self.timeout + self.crash
    }

    /// Pass rate in percent.
    pub fn pass_rate(&self) -> f64 {
        if self.total() == 0 {
            100.0
        } else {
            100.0 * self.pass as f64 / self.total() as f64
        }
    }
}

/// The full scorecard: the checked-in baseline and the CI artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scorecard {
    /// Subset the run covered ("pr" or "full").
    pub subset: String,
    /// Per-category tallies (kebab-case category name → score).
    pub categories: BTreeMap<String, CategoryScore>,
    /// Per-case classification (case id → class). The regression check
    /// compares these against the baseline.
    pub cases: BTreeMap<String, Class>,
    /// Details for every non-pass case (id → detail).
    pub details: BTreeMap<String, String>,
}

impl Scorecard {
    /// Build from a set of results.
    pub fn from_results(subset: &str, results: &[CaseResult], corpus: &[Case]) -> Self {
        let cat_of: BTreeMap<&str, Category> =
            corpus.iter().map(|c| (c.id.as_str(), c.category)).collect();
        let mut categories: BTreeMap<String, CategoryScore> = BTreeMap::new();
        let mut cases = BTreeMap::new();
        let mut details = BTreeMap::new();
        for r in results {
            let cat = cat_of[r.id.as_str()];
            categories
                .entry(cat.name().to_string())
                .or_default()
                .add(r.class);
            cases.insert(r.id.clone(), r.class);
            if r.class != Class::Pass && !r.detail.is_empty() {
                details.insert(r.id.clone(), r.detail.clone());
            }
        }
        Scorecard {
            subset: subset.to_string(),
            categories,
            cases,
            details,
        }
    }

    /// Overall tallies.
    pub fn totals(&self) -> CategoryScore {
        let mut t = CategoryScore::default();
        for s in self.categories.values() {
            t.pass += s.pass;
            t.graceful_refusal += s.graceful_refusal;
            t.bad_geometry += s.bad_geometry;
            t.timeout += s.timeout;
            t.crash += s.crash;
        }
        t
    }

    /// Compare against a baseline: returns (regressions, improvements) as
    /// human-readable lines. A regression is any case whose class rank
    /// worsened; cases new to the corpus or missing from this run are
    /// ignored.
    pub fn diff_baseline(&self, baseline: &Scorecard) -> (Vec<String>, Vec<String>) {
        let mut regressions = Vec::new();
        let mut improvements = Vec::new();
        for (id, class) in &self.cases {
            let Some(base) = baseline.cases.get(id) else {
                continue;
            };
            use std::cmp::Ordering;
            match class.rank().cmp(&base.rank()) {
                Ordering::Greater => {
                    let detail = self
                        .details
                        .get(id)
                        .map(|d| format!(" — {d}"))
                        .unwrap_or_default();
                    regressions.push(format!("{id}: {} → {}{detail}", base.name(), class.name()));
                }
                Ordering::Less => {
                    improvements.push(format!("{id}: {} → {}", base.name(), class.name()));
                }
                Ordering::Equal => {}
            }
        }
        (regressions, improvements)
    }

    /// Render the markdown scoreboard.
    pub fn to_markdown(&self) -> String {
        let mut md = String::new();
        md.push_str(
            "| category | cases | pass | refusal | bad-geo | timeout | crash | pass rate |\n",
        );
        md.push_str("|---|---:|---:|---:|---:|---:|---:|---:|\n");
        for (cat, s) in &self.categories {
            md.push_str(&format!(
                "| {cat} | {} | {} | {} | {} | {} | {} | {:.1}% |\n",
                s.total(),
                s.pass,
                s.graceful_refusal,
                s.bad_geometry,
                s.timeout,
                s.crash,
                s.pass_rate()
            ));
        }
        let t = self.totals();
        md.push_str(&format!(
            "| **total** | **{}** | **{}** | **{}** | **{}** | **{}** | **{}** | **{:.1}%** |\n",
            t.total(),
            t.pass,
            t.graceful_refusal,
            t.bad_geometry,
            t.timeout,
            t.crash,
            t.pass_rate()
        ));
        md
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpus_is_deterministic_and_sized() {
        let a = build_corpus();
        let b = build_corpus();
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
        assert!(a.len() >= 500, "corpus has {} cases", a.len());
        let pr = a.iter().filter(|c| c.pr_subset).count();
        assert!(pr >= 150 && pr < a.len(), "pr subset has {pr} cases");
        for cat in Category::ALL {
            assert!(
                a.iter().any(|c| c.category == cat),
                "category {cat:?} is empty"
            );
        }
    }

    #[test]
    fn simple_case_passes() {
        let case = Case {
            id: "test".into(),
            category: Category::BooleanRandom,
            pr_subset: true,
            kind: CaseKind::BoolPair {
                a: PrimSpec::Cube(10.0, 10.0, 10.0),
                b: PrimSpec::Cube(10.0, 10.0, 10.0),
                pose_b: Pose::at(5.0, 5.0, 5.0),
                op: Op::Union,
            },
        };
        let r = execute_case(&case);
        assert_eq!(r.class, Class::Pass, "detail: {}", r.detail);
    }

    #[test]
    fn class_ranks_order() {
        assert!(Class::Pass.rank() < Class::GracefulRefusal.rank());
        assert!(Class::GracefulRefusal.rank() < Class::BadGeometry.rank());
        assert!(Class::BadGeometry.rank() < Class::Timeout.rank());
        assert!(Class::Timeout.rank() < Class::Crash.rank());
    }
}
