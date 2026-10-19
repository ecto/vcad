//! Joint range-of-motion clearance sweep.
//!
//! For each revolute joint, drive it from zero toward each of its limits in
//! fixed steps, every other joint held at zero, and report the first angle in
//! each direction where the *moving subtree* (every instance that joint
//! carries) overlaps the rest of the assembly by more than it did at the zero
//! pose. The zero-pose overlap is the baseline because real mechanisms carry
//! deliberate overlaps there — coaxial drums nested into each other, a motor
//! can seated in its bore — and those are design, not collisions.
//!
//! ## How overlap is measured
//!
//! Volumes come from vertical ray columns on a fixed world-space lattice of
//! pitch `resolution_mm`. Each column is cast through every mesh once; the
//! crossings, sorted and accumulated as a winding number, give the exact
//! inside intervals along z. The overlap of two solids is then
//! `Σ_columns h² · |I_a ∩ I_b|` — exact in z, sampled in x/y. The moving
//! side and the stationary side are each *unioned* per column before
//! intersecting, so the total is the volume of `moving ∩ rest`, not a sum
//! that double-counts where parts within one side overlap each other. The
//! same columns give per-pair overlaps, which is how a collision is
//! attributed to the parts that actually hit.
//!
//! Stationary geometry never moves during a sweep, so its columns are cast
//! once; only the moving subtree is re-cast per step.

use std::collections::{HashMap, HashSet};

use vcad_ir::{Document, JointKind, Transform3D};

use crate::pose::Affine;

/// Which instances count as "the rest of the assembly" for a joint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RangeScope {
    /// Every instance the joint does not move.
    #[default]
    All,
    /// Only the joint's own kinematic chain: instances moved by no joint, or
    /// only by joints that also carry this one. Other limbs are posture — an
    /// arm hanging where a leg swings is a pose problem, not a design one.
    Chain,
}

/// Knobs for [`check_joint_range`].
#[derive(Debug, Clone, PartialEq)]
pub struct JointRangeOptions {
    /// Sweep increment in degrees.
    pub step_deg: f64,
    /// New overlap volume (mm³ past the zero-pose overlap) that counts as a
    /// collision.
    pub threshold_mm3: f64,
    /// Column lattice pitch in mm. Smaller is more accurate and slower.
    pub resolution_mm: f64,
    /// Joint ids or names to sweep. Empty sweeps every revolute joint.
    pub joints: Vec<String>,
    /// What the moving subtree is checked against.
    pub scope: RangeScope,
    /// Range to sweep for a revolute joint with no declared limits, degrees.
    pub unlimited_range_deg: (f64, f64),
}

impl Default for JointRangeOptions {
    fn default() -> Self {
        Self {
            step_deg: 5.0,
            threshold_mm3: 200.0,
            resolution_mm: 1.0,
            joints: Vec::new(),
            scope: RangeScope::All,
            unlimited_range_deg: (-180.0, 180.0),
        }
    }
}

/// One part pair whose overlap grew at the colliding pose.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct CollidingPair {
    /// Instance in the moving subtree.
    pub moving: String,
    /// Stationary instance it ran into.
    pub stationary: String,
    /// Overlap growth over the zero pose, mm³.
    pub new_overlap_mm3: f64,
}

/// One direction of travel of a swept joint.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RangeSide {
    /// The limit this side sweeps toward, degrees.
    pub limit_deg: f64,
    /// The furthest angle reached without collision, degrees. Equals
    /// `limit_deg` when the side is free all the way.
    pub free_to_deg: f64,
    /// The first sampled angle that collided, if any.
    pub first_collision_deg: Option<f64>,
    /// New overlap at that angle, mm³.
    pub new_overlap_mm3: Option<f64>,
    /// Pairs whose overlap grew there, worst first.
    pub hits: Vec<CollidingPair>,
}

impl RangeSide {
    /// True when the side reached its limit without colliding.
    pub fn is_free(&self) -> bool {
        self.first_collision_deg.is_none()
    }
}

/// The sweep result for one joint.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct JointRange {
    /// Joint id.
    pub joint_id: String,
    /// Joint name, when it has one.
    pub name: Option<String>,
    /// Declared limits (or the unlimited default), degrees.
    pub limits_deg: (f64, f64),
    /// Instances the joint moves.
    pub moving: Vec<String>,
    /// Overlap of the moving subtree with the rest at the zero pose, mm³.
    pub baseline_overlap_mm3: f64,
    /// Sweep toward the lower limit.
    pub negative: RangeSide,
    /// Sweep toward the upper limit.
    pub positive: RangeSide,
}

impl JointRange {
    /// The joint's display label: its name, else its id.
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.joint_id)
    }

    /// One summary line, e.g.
    /// `l_knee  limits -10..150  free -10..135  hits: + shin_i/thigh_i`.
    pub fn summary(&self) -> String {
        let side = |s: &RangeSide| match s.hits.first() {
            Some(h) => format!("{}↔{}", h.moving, h.stationary),
            None if s.is_free() => "-".to_string(),
            None => "?".to_string(),
        };
        format!(
            "{:18} limits {:7.1}..{:<7.1} free {:7.1}..{:<7.1} hits: -{} +{}",
            self.label(),
            self.limits_deg.0,
            self.limits_deg.1,
            self.negative.free_to_deg,
            self.positive.free_to_deg,
            side(&self.negative),
            side(&self.positive),
        )
    }
}

/// Why a sweep could not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JointRangeError {
    /// The document has no instances or no revolute joints.
    NothingToSweep(String),
    /// A requested joint does not exist or is not revolute.
    UnknownJoint(String),
    /// Bad option values.
    BadOptions(String),
    /// Geometry evaluation failed.
    Eval(String),
}

impl std::fmt::Display for JointRangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NothingToSweep(s) | Self::UnknownJoint(s) | Self::BadOptions(s) => f.write_str(s),
            Self::Eval(e) => write!(f, "assembly geometry evaluation failed: {e}"),
        }
    }
}

impl std::error::Error for JointRangeError {}

/// Sweep each revolute joint over its limits and report where it collides.
///
/// Every joint is zeroed first, so the sweep is of the zero pose regardless
/// of the document's stored joint states. Geometry is evaluated exactly once.
pub fn check_joint_range(
    doc: &Document,
    options: &JointRangeOptions,
) -> Result<Vec<JointRange>, JointRangeError> {
    if options.step_deg.is_nan()
        || options.resolution_mm.is_nan()
        || options.step_deg <= 0.0
        || options.resolution_mm <= 0.0
    {
        return Err(JointRangeError::BadOptions(
            "step and resolution must be positive".to_string(),
        ));
    }
    if doc.instances.as_ref().is_none_or(|i| i.is_empty()) {
        return Err(JointRangeError::NothingToSweep(
            "document has no instances — not an assembly".to_string(),
        ));
    }

    // Zero every joint: the sweep is defined about the zero pose. FK reads
    // only instances and joints, so the per-step document carries no nodes.
    let mut zeroed = doc.clone();
    for j in zeroed.joints.iter_mut().flatten() {
        j.state = 0.0;
    }
    let mut fk_doc = Document {
        instances: zeroed.instances.clone(),
        joints: zeroed.joints.clone(),
        ..Document::default()
    };
    let joints = fk_doc.joints.clone().unwrap_or_default();

    let targets = select_joints(&joints, &options.joints)?;

    let scene = vcad_eval::evaluate_document(&zeroed, &vcad_eval::EvalOptions::default())
        .map_err(|e| JointRangeError::Eval(format!("{e:?}")))?;
    let instances = scene.instances.unwrap_or_default();
    let ids: Vec<String> = instances.iter().map(|i| i.instance_id.clone()).collect();
    let local: Vec<LocalMesh> = instances
        .iter()
        .map(|i| LocalMesh {
            positions: i.mesh.positions.clone(),
            indices: i.mesh.indices.clone(),
        })
        .collect();

    let zero_world = vcad_eval::solve_forward_kinematics(&fk_doc);
    let world_at = |w: &HashMap<String, Transform3D>, id: &str| {
        w.get(id).map(Affine::from_transform3d).unwrap_or_default()
    };
    let zero_affines: Vec<Affine> = ids.iter().map(|id| world_at(&zero_world, id)).collect();

    // Which instances each revolute joint moves: perturb it and diff FK.
    let mut moves: HashMap<String, HashSet<usize>> = HashMap::new();
    for (ji, j) in joints.iter().enumerate() {
        if !matches!(j.kind, JointKind::Revolute { .. }) {
            continue;
        }
        set_state(&mut fk_doc, ji, 17.0);
        let w = vcad_eval::solve_forward_kinematics(&fk_doc);
        set_state(&mut fk_doc, ji, 0.0);
        let moved: HashSet<usize> = ids
            .iter()
            .enumerate()
            .filter(|(k, id)| !affine_eq(&world_at(&w, id), &zero_affines[*k]))
            .map(|(k, _)| k)
            .collect();
        moves.insert(j.id.clone(), moved);
    }

    let lattice = Lattice::new(options.resolution_mm);
    // Stationary columns are cast once, at the zero pose, for every part.
    let zero_columns: Vec<Columns> = local
        .iter()
        .zip(&zero_affines)
        .map(|(m, a)| lattice.cast(m, a))
        .collect();

    // One joint's sweep. Joints are independent (each starts from the zero
    // pose), so they run in parallel, each worker on its own FK document.
    let sweep_joint = |ji: usize, fk_doc: &mut Document| -> JointRange {
        let joint = &joints[ji];
        let moving_set = moves.get(&joint.id).cloned().unwrap_or_default();
        let mut moving: Vec<usize> = moving_set.iter().copied().collect();
        moving.sort_unstable();

        let stationary: Vec<usize> = (0..ids.len())
            .filter(|k| !moving_set.contains(k))
            .filter(|k| match options.scope {
                RangeScope::All => true,
                RangeScope::Chain => moves.iter().all(|(other, set)| {
                    // `k` is chain geometry unless some joint moves it that
                    // does not also carry this joint's subtree.
                    !set.contains(k) || other == &joint.id || moving_set.is_subset(set)
                }),
            })
            .collect();

        let limits = match &joint.kind {
            JointKind::Revolute { limits, .. } => limits.unwrap_or(options.unlimited_range_deg),
            _ => unreachable!("select_joints keeps revolute joints only"),
        };

        let static_cols = StationaryColumns::new(&stationary, &zero_columns, lattice.h * lattice.h);
        let measure = |fk_doc: &mut Document, deg: f64| {
            set_state(fk_doc, ji, deg);
            let w = vcad_eval::solve_forward_kinematics(fk_doc);
            set_state(fk_doc, ji, 0.0);
            let cols: Vec<(usize, Columns)> = moving
                .iter()
                .map(|&k| (k, lattice.cast(&local[k], &world_at(&w, &ids[k]))))
                .collect();
            static_cols.overlap(&cols)
        };

        let base = measure(fk_doc, 0.0);
        let mut sweep_side = |end: f64| -> RangeSide {
            let mut side = RangeSide {
                limit_deg: end,
                free_to_deg: end,
                first_collision_deg: None,
                new_overlap_mm3: None,
                hits: Vec::new(),
            };
            let mut prev = 0.0;
            for deg in side_angles(end, options.step_deg) {
                let o = measure(fk_doc, deg);
                let grew = o.total - base.total;
                if grew > options.threshold_mm3 {
                    side.free_to_deg = prev;
                    side.first_collision_deg = Some(deg);
                    side.new_overlap_mm3 = Some(grew);
                    side.hits = growth(&o, &base, &ids);
                    break;
                }
                prev = deg;
            }
            side
        };
        let negative = sweep_side(limits.0.min(0.0));
        let positive = sweep_side(limits.1.max(0.0));

        JointRange {
            joint_id: joint.id.clone(),
            name: joint.name.clone(),
            limits_deg: limits,
            moving: moving.iter().map(|&k| ids[k].clone()).collect(),
            baseline_overlap_mm3: base.total,
            negative,
            positive,
        }
    };

    let workers = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(targets.len())
        .max(1);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut results: Vec<(usize, JointRange)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                let mut doc = fk_doc.clone();
                let (next, targets, sweep_joint) = (&next, &targets, &sweep_joint);
                scope.spawn(move || {
                    let mut done = Vec::new();
                    loop {
                        let n = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(&ji) = targets.get(n) else { break };
                        done.push((n, sweep_joint(ji, &mut doc)));
                    }
                    done
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("joint sweep worker panicked"))
            .collect()
    });
    results.sort_by_key(|(n, _)| *n);
    Ok(results.into_iter().map(|(_, r)| r).collect())
}

fn set_state(doc: &mut Document, joint_index: usize, deg: f64) {
    if let Some(j) = doc.joints.as_mut().and_then(|js| js.get_mut(joint_index)) {
        j.state = deg;
    }
}

fn affine_eq(a: &Affine, b: &Affine) -> bool {
    let lin = (0..3).all(|i| (0..3).all(|j| (a.linear[i][j] - b.linear[i][j]).abs() < 1e-9));
    lin && (0..3).all(|i| (a.translation[i] - b.translation[i]).abs() < 1e-6)
}

/// Resolve requested joints (id, then name) to revolute joint indices.
fn select_joints(
    joints: &[vcad_ir::Joint],
    wanted: &[String],
) -> Result<Vec<usize>, JointRangeError> {
    let revolute = |j: &vcad_ir::Joint| matches!(j.kind, JointKind::Revolute { .. });
    if wanted.is_empty() {
        let all: Vec<usize> = (0..joints.len())
            .filter(|&i| revolute(&joints[i]))
            .collect();
        if all.is_empty() {
            return Err(JointRangeError::NothingToSweep(
                "document has no revolute joints to sweep".to_string(),
            ));
        }
        return Ok(all);
    }
    wanted
        .iter()
        .map(|w| {
            let i = joints
                .iter()
                .position(|j| &j.id == w)
                .or_else(|| joints.iter().position(|j| j.name.as_deref() == Some(w)))
                .ok_or_else(|| {
                    let known: Vec<&str> = joints
                        .iter()
                        .filter(|j| revolute(j))
                        .map(|j| j.name.as_deref().unwrap_or(&j.id))
                        .collect();
                    JointRangeError::UnknownJoint(format!(
                        "no joint '{w}'. Revolute joints: {}",
                        known.join(", ")
                    ))
                })?;
            if !revolute(&joints[i]) {
                return Err(JointRangeError::UnknownJoint(format!(
                    "joint '{w}' is not revolute; only revolute joints are swept"
                )));
            }
            Ok(i)
        })
        .collect()
}

/// Sampled angles from zero toward `end`: every `step`, plus `end` itself.
fn side_angles(end: f64, step: f64) -> Vec<f64> {
    let mut out = Vec::new();
    if end == 0.0 {
        return out;
    }
    let dir = end.signum();
    let mut k = 1.0;
    while k * step < end.abs() - 1e-9 {
        out.push(dir * k * step);
        k += 1.0;
    }
    out.push(end);
    out
}

/// Pairs whose overlap grew over the baseline, worst first.
fn growth(now: &Overlap, base: &Overlap, ids: &[String]) -> Vec<CollidingPair> {
    let mut hits: Vec<CollidingPair> = now
        .pairs
        .iter()
        .filter_map(|(&(m, s), &v)| {
            let grew = v - base.pairs.get(&(m, s)).copied().unwrap_or(0.0);
            (grew > 0.0).then(|| CollidingPair {
                moving: ids[m].clone(),
                stationary: ids[s].clone(),
                new_overlap_mm3: grew,
            })
        })
        .collect();
    hits.sort_by(|a, b| b.new_overlap_mm3.total_cmp(&a.new_overlap_mm3));
    hits.truncate(8);
    hits
}

// ── column casting ─────────────────────────────────────────────────────────

struct LocalMesh {
    positions: Vec<f32>,
    indices: Vec<u32>,
}

/// Inside intervals along z, per lattice column.
type Columns = HashMap<(i32, i32), Vec<(f64, f64)>>;

/// The fixed world-space column lattice. Column `(i, j)` sits at
/// `((i + ½)·h + ε, (j + ½)·h + ε)`; the tiny offset keeps rays off the
/// axis-aligned edges and vertices CAD meshes are full of.
struct Lattice {
    h: f64,
    eps: f64,
}

impl Lattice {
    fn new(h: f64) -> Self {
        Self {
            h,
            eps: h * 1.234_567e-4,
        }
    }

    fn center(&self, i: i32) -> f64 {
        (i as f64 + 0.5) * self.h + self.eps
    }

    /// Cast every column a mesh covers; returns its inside intervals.
    fn cast(&self, mesh: &LocalMesh, at: &Affine) -> Columns {
        let verts: Vec<[f64; 3]> = mesh
            .positions
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| at.point([p[0] as f64, p[1] as f64, p[2] as f64]))
            .collect();
        // Mirroring poses flip winding; keep "entering" meaning entering.
        let flip = at.determinant() < 0.0;
        let mut hits: HashMap<(i32, i32), Vec<(f64, i32)>> = HashMap::new();
        for t in mesh.indices.as_chunks::<3>().0 {
            let (a, b, c) = (
                verts[t[0] as usize],
                verts[t[1] as usize],
                verts[t[2] as usize],
            );
            // Signed projected area: the sign is the z-component of the normal.
            let area = (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1]);
            if area.abs() < 1e-12 {
                continue;
            }
            // A ray going +z enters where the normal faces down.
            let entering = (area < 0.0) != flip;
            let sign = if entering { 1 } else { -1 };
            let lo_x = a[0].min(b[0]).min(c[0]);
            let hi_x = a[0].max(b[0]).max(c[0]);
            let lo_y = a[1].min(b[1]).min(c[1]);
            let hi_y = a[1].max(b[1]).max(c[1]);
            let i0 = ((lo_x - self.eps) / self.h - 0.5).ceil() as i32;
            let i1 = ((hi_x - self.eps) / self.h - 0.5).floor() as i32;
            let j0 = ((lo_y - self.eps) / self.h - 0.5).ceil() as i32;
            let j1 = ((hi_y - self.eps) / self.h - 0.5).floor() as i32;
            for i in i0..=i1 {
                let x = self.center(i);
                for j in j0..=j1 {
                    let y = self.center(j);
                    // Barycentric coordinates of (x, y) in the projection.
                    let w0 = ((b[0] - x) * (c[1] - y) - (c[0] - x) * (b[1] - y)) / area;
                    let w1 = ((c[0] - x) * (a[1] - y) - (a[0] - x) * (c[1] - y)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let z = w0 * a[2] + w1 * b[2] + w2 * c[2];
                    hits.entry((i, j)).or_default().push((z, sign));
                }
            }
        }
        hits.into_iter()
            .filter_map(|(key, mut crossings)| {
                crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
                let mut winding = 0;
                let mut start = 0.0;
                let mut spans = Vec::new();
                for (z, s) in crossings {
                    let was = winding != 0;
                    winding += s;
                    let is = winding != 0;
                    if !was && is {
                        start = z;
                    } else if was && !is && z > start {
                        spans.push((start, z));
                    }
                }
                (!spans.is_empty()).then_some((key, spans))
            })
            .collect()
    }
}

/// Overlap volumes at one pose.
struct Overlap {
    /// `|moving ∩ stationary|` with each side unioned, mm³.
    total: f64,
    /// Per (moving, stationary) instance pair, mm³.
    pairs: HashMap<(usize, usize), f64>,
}

/// Column → the stationary parts covering it, with their spans.
type ColumnOwners<'a> = HashMap<(i32, i32), Vec<(usize, &'a [(f64, f64)])>>;

/// The stationary side's columns, indexed by column for fast lookup.
struct StationaryColumns<'a> {
    /// Column → the stationary parts covering it, with their spans.
    by_column: ColumnOwners<'a>,
    area: f64,
}

impl<'a> StationaryColumns<'a> {
    /// `area` is one column's cross-section, h².
    fn new(stationary: &[usize], zero: &'a [Columns], area: f64) -> Self {
        let mut by_column: ColumnOwners<'a> = HashMap::new();
        for &k in stationary {
            for (key, spans) in &zero[k] {
                by_column
                    .entry(*key)
                    .or_default()
                    .push((k, spans.as_slice()));
            }
        }
        Self { by_column, area }
    }

    fn overlap(&self, moving: &[(usize, Columns)]) -> Overlap {
        let mut pairs: HashMap<(usize, usize), f64> = HashMap::new();
        // Per column: every moving part's spans, for the unioned total.
        let mut moving_by_column: HashMap<(i32, i32), Vec<(f64, f64)>> = HashMap::new();
        for (m, cols) in moving {
            for (key, spans) in cols {
                let Some(stat) = self.by_column.get(key) else {
                    continue;
                };
                for (s, sspans) in stat {
                    let v = intersect_len(spans, sspans);
                    if v > 0.0 {
                        *pairs.entry((*m, *s)).or_default() += v * self.area;
                    }
                }
                moving_by_column.entry(*key).or_default().extend(spans);
            }
        }
        let mut total = 0.0;
        for (key, spans) in moving_by_column {
            // `moving_by_column` only holds columns the stationary side covers.
            let m = union(spans);
            let s = union(
                self.by_column[&key]
                    .iter()
                    .flat_map(|(_, sp)| sp.iter().copied())
                    .collect(),
            );
            total += intersect_len(&m, &s) * self.area;
        }
        Overlap { total, pairs }
    }
}

/// Merge spans into a sorted disjoint set.
fn union(mut spans: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Vec<(f64, f64)> = Vec::with_capacity(spans.len());
    for (a, b) in spans {
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// Total length of the intersection of two span sets (each sorted, disjoint
/// within itself).
fn intersect_len(a: &[(f64, f64)], b: &[(f64, f64)]) -> f64 {
    let (mut i, mut j, mut len) = (0, 0, 0.0);
    while i < a.len() && j < b.len() {
        let lo = a[i].0.max(b[j].0);
        let hi = a[i].1.min(b[j].1);
        if hi > lo {
            len += hi - lo;
        }
        if a[i].1 < b[j].1 {
            i += 1;
        } else {
            j += 1;
        }
    }
    len
}
