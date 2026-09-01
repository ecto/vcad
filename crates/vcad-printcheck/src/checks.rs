//! The individual printability checks.
//!
//! Each `check_*` returns its own summary plus the findings it raised. The
//! caller (`lib.rs`) decides the exit verdict from the finding severities.

use std::collections::HashMap;

use crate::mesh::{intervals, Mesh, RayIndex};
use crate::{Finding, FindingKind, Options, Severity};

// ---------------------------------------------------------------------------
// manifold
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct ManifoldSummary {
    pub triangles: usize,
    pub edges: usize,
    /// Edges not shared by exactly two triangles.
    pub bad_edges: usize,
}

/// Edge census, quantising vertices to 1e-3 mm — the same tolerance
/// `rana tools/manifold-check.py` uses, which is well below any printable
/// feature and well above STL's f32 round-trip noise.
pub fn check_manifold(mesh: &Mesh) -> (ManifoldSummary, Vec<Finding>) {
    let q = |v: &[f64; 3]| {
        [
            (v[0] * 1000.0).round() as i64,
            (v[1] * 1000.0).round() as i64,
            (v[2] * 1000.0).round() as i64,
        ]
    };
    let mut edges: HashMap<([i64; 3], [i64; 3]), u32> = HashMap::new();
    for t in &mesh.tris {
        let vs = [q(&t[0]), q(&t[1]), q(&t[2])];
        for i in 0..3 {
            let (a, b) = (vs[i], vs[(i + 1) % 3]);
            let key = if a <= b { (a, b) } else { (b, a) };
            *edges.entry(key).or_insert(0) += 1;
        }
    }
    let bad = edges.values().filter(|&&c| c != 2).count();
    let summary = ManifoldSummary {
        triangles: mesh.tris.len(),
        edges: edges.len(),
        bad_edges: bad,
    };
    let mut findings = Vec::new();
    if bad > 0 {
        findings.push(Finding {
            kind: FindingKind::NonManifold,
            severity: Severity::Fail,
            message: format!("{bad} edges are not shared by exactly two triangles"),
            location: None,
            value_mm: None,
        });
    }
    (summary, findings)
}

// ---------------------------------------------------------------------------
// closed sections
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct SectionSummary {
    pub z_min: f64,
    pub z_max: f64,
    pub sections: usize,
    pub empty_layers: Vec<f64>,
    pub open_sections: Vec<(f64, usize)>,
}

/// Slice the way a slicer does and verify every cross-section closes.
///
/// A manifold check asks whether the whole mesh is watertight. This asks the
/// question that actually stops a print: at height z, do the triangle/plane
/// intersections join into closed loops? A mesh can be locally holed without
/// being globally broken, and the slicer reports that as "can't be printed for
/// empty layer between a and b". Ported from `rana tools/slice-check.py`.
pub fn check_sections(mesh: &Mesh, opts: &Options) -> (SectionSummary, Vec<Finding>) {
    let (lo, hi) = mesh.bounds();
    let (zlo, zhi) = (lo[2], hi[2]);
    // Horizontal faces live at these heights; a sample plane landing on one
    // gives a degenerate section, so — like a slicer — we nudge off it.
    let mut flat: Vec<f64> = mesh
        .tris
        .iter()
        .flat_map(|t| t.iter().map(|v| (v[2] * 10000.0).round() / 10000.0))
        .collect();
    flat.sort_by(|a, b| a.partial_cmp(b).unwrap());
    flat.dedup();

    let step = opts.section_step;
    let mut empty = Vec::new();
    let mut open = Vec::new();
    let mut count = 0usize;
    let mut z = zlo + step / 2.0;
    while z < zhi {
        if flat
            .binary_search_by(|p| p.partial_cmp(&z).unwrap())
            .is_ok()
            || flat.iter().any(|f| (f - z).abs() < 1e-4)
        {
            z += step * 0.137;
        }
        if z >= zhi {
            break;
        }
        count += 1;
        let (segs, loose) = section(mesh, z);
        if segs == 0 {
            empty.push(round3(z));
        } else if loose > 0 {
            open.push((round3(z), loose));
        }
        z += step;
    }

    let mut findings = Vec::new();
    if !empty.is_empty() {
        findings.push(Finding {
            kind: FindingKind::EmptyLayer,
            severity: Severity::Fail,
            message: format!(
                "{} empty layer(s), first at z={:.2} — the slicer cannot print through this",
                empty.len(),
                empty[0]
            ),
            location: Some([0.0, 0.0, empty[0]]),
            value_mm: None,
        });
    }
    if !open.is_empty() {
        let worst = open.iter().map(|o| o.1).max().unwrap_or(0);
        findings.push(Finding {
            kind: FindingKind::OpenSection,
            severity: Severity::Fail,
            message: format!(
                "{} cross-section(s) do not close (worst {worst} loose ends, first at z={:.2})",
                open.len(),
                open[0].0
            ),
            location: Some([0.0, 0.0, open[0].0]),
            value_mm: None,
        });
    }

    (
        SectionSummary {
            z_min: round3(zlo),
            z_max: round3(zhi),
            sections: count,
            empty_layers: empty,
            open_sections: open,
        },
        findings,
    )
}

/// Returns (segment count, vertices with odd degree) for the z-section.
fn section(mesh: &Mesh, z: f64) -> (usize, usize) {
    let mut deg: HashMap<(i64, i64), u32> = HashMap::new();
    let mut segs = 0usize;
    for t in &mesh.tris {
        let mut pts: Vec<(i64, i64)> = Vec::new();
        for i in 0..3 {
            let (a, b) = (t[i], t[(i + 1) % 3]);
            if (a[2] - z) * (b[2] - z) < 0.0 {
                let f = (z - a[2]) / (b[2] - a[2]);
                pts.push(qpt(a[0] + f * (b[0] - a[0]), a[1] + f * (b[1] - a[1])));
            } else if (a[2] - z).abs() < 1e-9 {
                pts.push(qpt(a[0], a[1]));
            }
        }
        pts.dedup();
        if pts.len() > 1 && pts[0] == pts[pts.len() - 1] {
            pts.pop();
        }
        if pts.len() == 2 {
            segs += 1;
            *deg.entry(pts[0]).or_insert(0) += 1;
            *deg.entry(pts[1]).or_insert(0) += 1;
        }
    }
    let loose = deg.values().filter(|d| *d % 2 == 1).count();
    (segs, loose)
}

fn qpt(x: f64, y: f64) -> (i64, i64) {
    ((x * 100.0).round() as i64, (y * 100.0).round() as i64)
}

// ---------------------------------------------------------------------------
// columns: floating regions, interior cracks, bridges
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct BridgeSpan {
    pub z: f64,
    pub span_mm: f64,
    pub columns: usize,
    pub anchored: bool,
    pub whitelisted: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ColumnSummary {
    pub columns_sampled: usize,
    pub columns_with_material: usize,
    pub cracks: usize,
    pub thinnest_gap_mm: Option<f64>,
    pub floating_regions: usize,
    pub bridges: Vec<BridgeSpan>,
}

struct Column {
    iv: Vec<(f64, f64)>,
}

impl Column {
    /// Material passes continuously through `z` from below — i.e. this column
    /// can carry a neighbour's first layer at that height.
    fn supports(&self, z: f64) -> bool {
        self.iv.iter().any(|(a, b)| *a < z - 1e-6 && *b > z + 1e-6)
    }
}

/// Per-column vertical raycast: mid-air restarts (floating regions and
/// bridges) and interior gaps thinner than the crack threshold.
///
/// This is the check that caught rana finding #10: v6's z-banded build left
/// 0.04–0.06 mm horizontal cracks where the descending cam profile ate the
/// bands' z-overlap. An analytic profile check cannot see that; only a raycast
/// on the final mesh can.
pub fn check_columns(mesh: &Mesh, opts: &Options) -> (ColumnSummary, Vec<Finding>) {
    let (lo, hi) = mesh.bounds();
    let idx = RayIndex::build(mesh);
    // A SQUARE sampling pitch, not `grid` columns per axis: the bridge metric
    // counts grid steps outward to an anchor, and on a stretched grid a step
    // in x would be worth a different number of millimetres than a step in y.
    let (nx, ny, pitch) = grid_dims(lo, hi, opts.pitch, opts.max_columns);
    let (cw, ch) = (pitch, pitch);

    // Sample at cell centres nudged by an irrational fraction of a cell, so
    // rays never land exactly on a seam, a vertex, or a symmetry plane — the
    // rana checker dodges its 0.5° sector seams the same way.
    let px = |i: usize| lo[0] + (i as f64 + 0.5 + 0.0193) * cw;
    let py = |j: usize| lo[1] + (j as f64 + 0.5 + 0.0271) * ch;

    let mut cols: Vec<Option<Column>> = Vec::with_capacity(nx * ny);
    for j in 0..ny {
        for i in 0..nx {
            let cr = idx.crossings(px(i), py(j));
            // An odd crossing count means the ray grazed an edge; parity is
            // meaningless there, so the column is dropped rather than guessed.
            if cr.is_empty() || cr.len() % 2 == 1 {
                cols.push(None);
                continue;
            }
            let iv = intervals(&cr);
            if iv.is_empty() {
                cols.push(None);
            } else {
                cols.push(Some(Column { iv }));
            }
        }
    }

    let bed = lo[2];
    let mut findings = Vec::new();
    let mut cracks = 0usize;
    let mut thinnest: Option<f64> = None;
    // (i, j, z) candidates: material that starts in mid-air.
    let mut restarts: Vec<(usize, usize, f64)> = Vec::new();

    for j in 0..ny {
        for i in 0..nx {
            let Some(col) = &cols[j * nx + i] else {
                continue;
            };
            for k in 0..col.iv.len() {
                let z = col.iv[k].0;
                if k == 0 {
                    // A first interval starting off the bed is also material
                    // appearing in mid-air for this column; whether it is a
                    // defect depends on lateral anchoring, decided below.
                    //
                    // "Off the bed" is judged against the first layer, not
                    // against zero. A chamfered rim touches the plate along a
                    // ring far thinner than the sampling pitch, so every
                    // column over the chamfer starts a few hundredths high —
                    // and the extruder lays all of it into the first layer
                    // regardless.
                    if z > bed + opts.bed_tol {
                        restarts.push((i, j, z));
                    }
                    continue;
                }
                let gap = z - col.iv[k - 1].1;
                thinnest = Some(thinnest.map_or(gap, |t: f64| t.min(gap)));
                if gap < opts.crack_threshold {
                    // A gap thinner than the threshold is a crack, never a
                    // channel — and never whitelistable. This is the failure
                    // mode a slicer reports as a floating layer.
                    cracks += 1;
                    if cracks <= opts.max_reported {
                        findings.push(Finding {
                            kind: FindingKind::Crack,
                            severity: Severity::Fail,
                            message: format!(
                                "interior crack: {:.3} mm material gap at z={:.3} (< {:.3} threshold)",
                                gap, z, opts.crack_threshold
                            ),
                            location: Some([round3(px(i)), round3(py(j)), round3(z)]),
                            value_mm: Some(round3(gap)),
                        });
                    }
                } else {
                    restarts.push((i, j, z));
                }
            }
        }
    }
    if cracks > opts.max_reported {
        findings.push(Finding {
            kind: FindingKind::Crack,
            severity: Severity::Fail,
            message: format!(
                "... and {} further interior cracks (use --max-reported to see more)",
                cracks - opts.max_reported
            ),
            location: None,
            value_mm: None,
        });
    }

    // Group restarts into connected regions: adjacent columns whose restart
    // heights agree to within a layer or so are one roof.
    let z_tol = opts.section_step.max(0.4);
    let mut owner: HashMap<(usize, usize, i64), usize> = HashMap::new();
    let key = |i: usize, j: usize, z: f64| (i, j, (z / z_tol).round() as i64);
    let mut groups: Vec<Vec<(usize, usize, f64)>> = Vec::new();
    let mut lookup: HashMap<(usize, usize, i64), Vec<f64>> = HashMap::new();
    for &(i, j, z) in &restarts {
        lookup.entry(key(i, j, z)).or_default().push(z);
    }
    for &(i, j, z) in &restarts {
        let k = key(i, j, z);
        if owner.contains_key(&k) {
            continue;
        }
        let gid = groups.len();
        let mut stack = vec![k];
        owner.insert(k, gid);
        let mut members = Vec::new();
        while let Some((ci, cj, cz)) = stack.pop() {
            let zs = lookup.get(&(ci, cj, cz)).cloned().unwrap_or_default();
            let zrep = zs.iter().copied().fold(f64::INFINITY, f64::min);
            members.push((ci, cj, zrep));
            for (di, dj) in [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)] {
                let ni = ci as i64 + di;
                let nj = cj as i64 + dj;
                if ni < 0 || nj < 0 || ni >= nx as i64 || nj >= ny as i64 {
                    continue;
                }
                for dz in [-1i64, 0, 1] {
                    let nk = (ni as usize, nj as usize, cz + dz);
                    if lookup.contains_key(&nk) && !owner.contains_key(&nk) {
                        owner.insert(nk, gid);
                        stack.push(nk);
                    }
                }
            }
        }
        groups.push(members);
    }

    let mut bridges = Vec::new();
    let mut floating = 0usize;
    for members in &groups {
        let z = members.iter().map(|m| m.2).fold(f64::INFINITY, f64::min);
        let (mut x0, mut y0) = (f64::INFINITY, f64::INFINITY);
        for &(i, j, _) in members {
            x0 = x0.min(px(i));
            y0 = y0.min(py(j));
        }

        // How far is this material from something that can hold it up?
        //
        // Measuring the region's own footprint would be wrong: a roof over a
        // slot is unsupported across the slot but continuous along it, and its
        // bounding diagonal would report the length of the slot rather than
        // the span the extruder actually has to cross. So walk outward from
        // every column that carries material continuously through this height
        // (an anchor) and take the furthest a member sits from one. Twice that
        // reach is the clear span.
        let reach = anchor_reach(&cols, nx, ny, members, z, z_tol, pitch);
        let (anchored, span) = match reach {
            Some(r) => (true, 2.0 * r),
            None => (false, f64::INFINITY),
        };
        let whitelisted = opts.allow_bridges.iter().any(|(a, b)| z >= *a && z <= *b);

        if whitelisted {
            // A documented bridge zone: the author has declared that material
            // starting here is a roof they mean to print. The rana shell is
            // accepted exactly this way — slot roofs at z 1.75..2.65, top leg
            // roof wedges at z 25.6..26.25. Cracks never reach this branch:
            // that verdict is raised above and is not waivable.
        } else if !anchored {
            floating += 1;
            if floating <= opts.max_reported {
                findings.push(Finding {
                    kind: FindingKind::FloatingRegion,
                    severity: Severity::Fail,
                    message: format!(
                        "floating region: {} column(s) of material start at z={:.3} with nothing beneath or beside them",
                        members.len(),
                        z
                    ),
                    location: Some([round3(x0), round3(y0), round3(z)]),
                    value_mm: None,
                });
            }
        } else if span > opts.max_bridge {
            floating += 1;
            findings.push(Finding {
                kind: FindingKind::OverlongBridge,
                severity: Severity::Fail,
                message: format!(
                    "unsupported span {:.2} mm at z={:.3} exceeds the {:.2} mm bridge limit",
                    span, z, opts.max_bridge
                ),
                location: Some([round3(x0), round3(y0), round3(z)]),
                value_mm: Some(round3(span)),
            });
        }
        if anchored {
            bridges.push(BridgeSpan {
                z: round3(z),
                span_mm: round3(span),
                columns: members.len(),
                anchored,
                whitelisted,
            });
        }
    }
    bridges.sort_by(|a, b| b.span_mm.partial_cmp(&a.span_mm).unwrap());
    bridges.truncate(opts.max_reported);

    let summary = ColumnSummary {
        columns_sampled: nx * ny,
        columns_with_material: cols.iter().filter(|c| c.is_some()).count(),
        cracks,
        thinnest_gap_mm: thinnest.map(round3),
        floating_regions: floating,
        bridges,
    };
    (summary, findings)
}

// ---------------------------------------------------------------------------
// overhang census
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct OverhangSummary {
    pub max_overhang_deg: f64,
    /// Area of near-horizontal downward faces (roofs and bridges).
    pub roof_area_mm2: f64,
    /// Downward faces steeper than the limit — these need support.
    pub unsupported_area_mm2: f64,
    /// Downward faces the printer can staircase its way up.
    pub self_supporting_area_mm2: f64,
    pub downward_area_mm2: f64,
    pub total_area_mm2: f64,
    pub verdict: String,
}

/// Area-weighted census of downward-facing facets.
///
/// `phi` is the angle between the outward facet normal and straight down: 0°
/// is a flat roof, 90° a vertical wall. A facet self-supports when its
/// deviation from vertical (90 - phi) stays within `max_overhang`.
pub fn check_overhangs(mesh: &Mesh, opts: &Options) -> (OverhangSummary, Vec<Finding>) {
    let mut roof = 0.0;
    let mut unsupported = 0.0;
    let mut selfsup = 0.0;
    let mut down = 0.0;
    let mut total = 0.0;
    for t in &mesh.tris {
        let u = sub(t[1], t[0]);
        let v = sub(t[2], t[0]);
        let c = cross(u, v);
        let area2 = norm(c);
        if area2 < 1e-12 {
            continue;
        }
        let area = area2 / 2.0;
        total += area;
        let nz = c[2] / area2;
        if nz >= 0.0 {
            continue;
        }
        down += area;
        let phi = (-nz).clamp(-1.0, 1.0).acos().to_degrees();
        if phi <= opts.roof_deg {
            roof += area;
        } else if 90.0 - phi > opts.max_overhang {
            unsupported += area;
        } else {
            selfsup += area;
        }
    }

    let mut findings = Vec::new();
    let verdict = if unsupported <= 1e-9 {
        format!(
            "every sloped downward face stays within {:.0}° of vertical — staircase, no support needed",
            opts.max_overhang
        )
    } else {
        let msg = format!(
            "{:.1} mm² of downward faces exceed {:.0}° from vertical — support material required",
            unsupported, opts.max_overhang
        );
        findings.push(Finding {
            kind: FindingKind::Overhang,
            severity: if opts.strict_overhangs {
                Severity::Fail
            } else {
                Severity::Warn
            },
            message: msg.clone(),
            location: None,
            value_mm: None,
        });
        msg
    };

    (
        OverhangSummary {
            max_overhang_deg: opts.max_overhang,
            roof_area_mm2: round3(roof),
            unsupported_area_mm2: round3(unsupported),
            self_supporting_area_mm2: round3(selfsup),
            downward_area_mm2: round3(down),
            total_area_mm2: round3(total),
            verdict,
        },
        findings,
    )
}

// ---------------------------------------------------------------------------
// min wall / min feature
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct WallSummary {
    pub nozzle_mm: f64,
    pub min_feature_mm: Option<f64>,
    /// Interior columns (all four in-plane neighbours also solid) whose
    /// material is thinner than the nozzle.
    pub thin_columns: usize,
    pub axis: Option<&'static str>,
}

/// Minimum wall / feature thickness, measured along all three axes.
///
/// A vertical ray cannot see a thin vertical wall — it runs *along* it. rana
/// shipped a 0.2 mm comb wall that no vertical analysis caught and only the
/// failed print revealed, so this casts sideways too.
///
/// Two filters keep silhouettes from faking thin walls. A curved surface
/// grazed by a ray returns a chord that says nothing about its thickness, so a
/// span counts only when BOTH of its end faces are hit within `wall_align`
/// of head-on; and the column must be interior, with all four in-plane
/// neighbours solid at the same height.
pub fn check_walls(mesh: &Mesh, opts: &Options) -> (WallSummary, Vec<Finding>) {
    let axes = [(0usize, "x"), (1, "y"), (2, "z")];
    let mut best: Option<(f64, &'static str, [f64; 3])> = None;
    let mut thin_total = 0usize;
    let mut worst_axis = None;

    for (axis, name) in axes {
        let m = mesh.permuted(axis);
        let (lo, hi) = m.bounds();
        let idx = RayIndex::build(&m);
        let (nx, ny, pitch) = grid_dims(lo, hi, opts.pitch, opts.max_columns);
        let px = |i: usize| lo[0] + (i as f64 + 0.5 + 0.0193) * pitch;
        let py = |j: usize| lo[1] + (j as f64 + 0.5 + 0.0271) * pitch;
        let mut cols: Vec<Vec<crate::mesh::Span>> = Vec::with_capacity(nx * ny);
        for j in 0..ny {
            for i in 0..nx {
                let cr = idx.crossings(px(i), py(j));
                if cr.len() % 2 == 1 {
                    cols.push(Vec::new());
                } else {
                    cols.push(crate::mesh::solid_spans(&cr));
                }
            }
        }
        let solid_at = |i: i64, j: i64, z: f64| -> bool {
            if i < 0 || j < 0 || i >= nx as i64 || j >= ny as i64 {
                return false;
            }
            cols[j as usize * nx + i as usize]
                .iter()
                .any(|s| s.lo <= z && s.hi >= z)
        };
        for j in 0..ny {
            for i in 0..nx {
                for sp in &cols[j * nx + i] {
                    if sp.align < opts.wall_align || !sp.opposing {
                        // grazing (a silhouette chord) or a taper (a chamfer
                        // feathering to an edge) — neither is a wall thickness
                        continue;
                    }
                    if sp.void_before < opts.crack_threshold || sp.void_after < opts.crack_threshold
                    {
                        // Hairline either side: a mesh seam sliced out of solid
                        // material, not a wall standing in air. Whether that
                        // hairline is itself acceptable is the crack check's
                        // question, not this one's.
                        continue;
                    }
                    let t = sp.hi - sp.lo;
                    let mid = (sp.lo + sp.hi) / 2.0;
                    let interior = [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)]
                        .iter()
                        .all(|(di, dj)| solid_at(i as i64 + di, j as i64 + dj, mid));
                    if !interior {
                        continue;
                    }
                    if best.is_none() || t < best.unwrap().0 {
                        // report the location back in the print frame
                        let p = match axis {
                            0 => [mid, px(i), py(j)],
                            1 => [py(j), mid, px(i)],
                            _ => [px(i), py(j), mid],
                        };
                        best = Some((t, name, p));
                    }
                    if t < opts.nozzle {
                        thin_total += 1;
                        worst_axis = Some(name);
                    }
                }
            }
        }
    }

    let mut findings = Vec::new();
    if thin_total >= opts.min_thin_columns {
        if let Some((t, name, p)) = best {
            findings.push(Finding {
                kind: FindingKind::ThinWall,
                severity: Severity::Fail,
                message: format!(
                    "min feature {:.3} mm along {name} ({thin_total} interior samples below the {:.2} mm nozzle) — unprintable",
                    t, opts.nozzle
                ),
                location: Some([round3(p[0]), round3(p[1]), round3(p[2])]),
                value_mm: Some(round3(t)),
            });
        }
    }

    (
        WallSummary {
            nozzle_mm: opts.nozzle,
            min_feature_mm: best.map(|b| round3(b.0)),
            thin_columns: thin_total,
            axis: worst_axis,
        },
        findings,
    )
}

// ---------------------------------------------------------------------------

/// Column counts and a square sampling pitch covering the XY footprint.
///
/// The pitch is set by the feature size being looked for — the nozzle — not by
/// a fixed column count over the bounding box. A 70 mm part sampled 64 columns
/// wide puts 1.1 mm between rays, which straddles a 2 mm tube wall: neighbours
/// land in air, so roofs read as unanchored and spans get measured across the
/// bore. `max_columns` keeps a large part from exploding the grid.
fn grid_dims(lo: [f64; 3], hi: [f64; 3], pitch: f64, max_columns: usize) -> (usize, usize, f64) {
    let sx = (hi[0] - lo[0]).max(1e-6);
    let sy = (hi[1] - lo[1]).max(1e-6);
    let longest = sx.max(sy);
    let pitch = pitch.max(longest / max_columns as f64);
    let nx = ((sx / pitch).ceil() as usize).max(1);
    let ny = ((sy / pitch).ceil() as usize).max(1);
    (nx, ny, pitch)
}

/// Furthest distance, in mm, from a member of `members` to the nearest column
/// that carries material continuously through height `z`.
///
/// Multi-source BFS from the anchors, propagating only through columns that
/// have material at `z` — either an anchor or a member of the region. A region
/// no anchor can reach is floating, and returns `None`.
fn anchor_reach(
    cols: &[Option<Column>],
    nx: usize,
    ny: usize,
    members: &[(usize, usize, f64)],
    z: f64,
    tol: f64,
    pitch: f64,
) -> Option<f64> {
    // Anything holding material at roughly this height can carry the walk:
    // the region's own columns, an anchor, or a neighbouring roof cell that
    // landed in a different height bucket. Restricting the walk to this
    // region's members alone orphans single columns off a curved roof, whose
    // restart heights straddle a bucket boundary, and reports them as
    // floating when they are part of the roof next door.
    let passable = |i: usize, j: usize| -> bool {
        cols[j * nx + i]
            .as_ref()
            .is_some_and(|c| c.iv.iter().any(|(a, b)| *a <= z + tol && *b >= z - tol))
    };
    let mut dist = vec![u32::MAX; nx * ny];
    let mut queue = std::collections::VecDeque::new();
    for j in 0..ny {
        for i in 0..nx {
            if cols[j * nx + i].as_ref().is_some_and(|c| c.supports(z)) {
                dist[j * nx + i] = 0;
                queue.push_back((i, j));
            }
        }
    }
    if queue.is_empty() {
        return None;
    }
    while let Some((i, j)) = queue.pop_front() {
        let d = dist[j * nx + i];
        for (di, dj) in [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)] {
            let (ni, nj) = (i as i64 + di, j as i64 + dj);
            if ni < 0 || nj < 0 || ni >= nx as i64 || nj >= ny as i64 {
                continue;
            }
            let (ni, nj) = (ni as usize, nj as usize);
            if dist[nj * nx + ni] != u32::MAX || !passable(ni, nj) {
                continue;
            }
            dist[nj * nx + ni] = d + 1;
            queue.push_back((ni, nj));
        }
    }
    let mut worst = 0u32;
    for &(i, j, _) in members {
        let d = dist[j * nx + i];
        if d == u32::MAX {
            return None; // unreachable from any anchor: floating
        }
        worst = worst.max(d);
    }
    Some(worst as f64 * pitch)
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm(a: [f64; 3]) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}
pub(crate) fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}
