//! Splitting of cylindrical faces along oblique (sampled) intersection curves.
//!
//! An oblique plane cutting a cylinder produces a closed ellipse that, in the
//! cylinder's (u, v) parameter space, is a single-valued sinusoid v = f(u).
//! More generally, every face this module produces is a **v-band**: a region
//! `{ (u, v) : u ∈ [u₀, u₁], lo(u) ≤ v ≤ hi(u) }` whose chains `lo`/`hi` are
//! piecewise-linear in u. The family is closed under splitting by another
//! single-valued curve — the pieces are again bands (possibly over smaller
//! u-intervals when the curve crosses a chain and pinches the region off).
//!
//! # Chains are explicit, independent vertex lists
//!
//! Each chain owns its own `(u, v)` vertex list; the two chains of a band
//! need NOT share u-columns. This is the load-bearing design decision: every
//! split operation slices the surviving chain segments VERBATIM and inserts
//! only exact cut/crossing points, so a chain that is somebody's shared rim
//! (a frozen cap ring, a neighboring band's profile) keeps exactly the
//! vertex set the neighbor carries. The former paired-column representation
//! interpolated each chain onto the union grid, which invented rim vertices
//! the neighboring face never had — a guaranteed seam crack for every
//! column the neighbor didn't share.
//!
//! Band faces are materialized as loops of two vertex chains (bottom chain
//! u-ascending, then top chain u-descending), the exact shape
//! `tessellate_ruled_two_chain` in vcad-kernel-tessellate renders verbatim
//! (its zipper triangulation accepts rails with different vertex sets), so
//! slanted/wavy boundaries survive to the mesh.
//!
//! This replaces the former behavior of returning oblique-cut cylinder faces
//! unsplit, which silently corrupted every boolean touching them (empty
//! near-containment intersections, un-trimmed pattern children, asymmetric
//! instance trimming — see `vcad-eval/tests/torr_boolean_catalogue.rs`).

use std::f64::consts::PI;

use vcad_kernel_geom::CylinderSurface;
use vcad_kernel_math::Point3;
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::FaceId;

use crate::split::SplitResult;

/// Numeric slop for u comparisons (radians).
const U_EPS: f64 = 1e-9;
/// Minimum band height (in v) for a region to count as real material.
const V_EPS: f64 = 1e-9;

/// A piecewise-linear chain v(u): `(u, v)` vertices with ascending u.
pub(crate) type Chain = Vec<(f64, f64)>;

/// A cylindrical face region `lo(u) ≤ v ≤ hi(u)`. The chains are
/// independent vertex lists (they need not share u-columns); both span the
/// same u-interval up to roughly one column of slack at either end (a pinch
/// collapsed into a single shared vertex shortens one chain). For a
/// `full_wrap` band each chain's last u equals its first u + 2π and the
/// region closes on itself around the axis.
#[derive(Debug, Clone)]
pub(crate) struct CylBand {
    /// Lower chain, ascending u.
    pub lo: Chain,
    /// Upper chain, ascending u.
    pub hi: Chain,
    /// Whether the band wraps the full circumference.
    pub full_wrap: bool,
}

/// A closed single-valued curve on a cylinder: v = f(u), periodic in 2π.
/// `us` ascend over one period starting at `us[0]`; evaluation wraps.
#[derive(Debug, Clone)]
pub(crate) struct CylProfile {
    us: Vec<f64>,
    vs: Vec<f64>,
}


macro_rules! band_dbg {
    ($($arg:tt)*) => {
        if std::env::var("VCAD_BAND_DEBUG").is_ok() {
            eprintln!($($arg)*);
        }
    };
}

/// Compute (u, v) cylinder coordinates of a point, u normalized to [0, 2π).
fn uv_of(p: &Point3, cyl: &CylinderSurface) -> (f64, f64) {
    let d = *p - cyl.center;
    let v = d.dot(cyl.axis.as_ref());
    let ref_dir = cyl.ref_dir.as_ref();
    let y_dir = cyl.axis.as_ref().cross(ref_dir);
    let mut u = d.dot(y_dir).atan2(d.dot(ref_dir));
    if u < 0.0 {
        u += 2.0 * PI;
    }
    if (u - 2.0 * PI).abs() < U_EPS {
        u = 0.0;
    }
    (u, v)
}

/// Evaluate the 3D point on the cylinder at (u, v).
fn point_at(cyl: &CylinderSurface, u: f64, v: f64) -> Point3 {
    let ref_dir = cyl.ref_dir.as_ref();
    let y_dir = cyl.axis.as_ref().cross(ref_dir);
    let (sin_u, cos_u) = u.sin_cos();
    cyl.center + cyl.radius * (cos_u * *ref_dir + sin_u * y_dir) + v * *cyl.axis.as_ref()
}

/// Clamped linear interpolation of a chain at `u`.
fn chain_interp(chain: &Chain, u: f64) -> f64 {
    let n = chain.len();
    if u <= chain[0].0 {
        return chain[0].1;
    }
    if u >= chain[n - 1].0 {
        return chain[n - 1].1;
    }
    let i = chain.partition_point(|p| p.0 < u);
    let (ua, va) = chain[i - 1];
    let (ub, vb) = chain[i];
    if (ub - ua).abs() < U_EPS {
        va
    } else {
        va + (u - ua) / (ub - ua) * (vb - va)
    }
}

/// Slice `chain` to `[a, b]`: exact interpolated endpoints plus every chain
/// vertex strictly inside, verbatim. A chain vertex within `U_EPS` of an
/// endpoint is absorbed into the endpoint (no duplicates).
fn cut_chain(chain: &Chain, a: f64, b: f64) -> Chain {
    let mut out: Chain = vec![(a, chain_interp(chain, a))];
    for &(u, v) in chain {
        if u > a + U_EPS && u < b - U_EPS {
            out.push((u, v));
        }
    }
    out.push((b, chain_interp(chain, b)));
    out
}

/// Slice a chain of a FULL-WRAP band to `[a, b]` where the interval may
/// extend past the chain's end (wrapping). The chain covers one period with
/// duplicated first/last u (u₀ and u₀ + 2π); values are periodic.
fn cut_chain_wrap(chain: &Chain, a: f64, b: f64) -> Chain {
    let end = chain[chain.len() - 1].0;
    if b <= end + U_EPS {
        return cut_chain(chain, a, b);
    }
    // [a, end) from the chain tail, then [start, b − 2π] shifted +2π.
    let mut out: Chain = vec![(a, chain_interp(chain, a))];
    for &(u, v) in chain {
        if u > a + U_EPS && u < end - U_EPS {
            out.push((u, v));
        }
    }
    let b2 = b - 2.0 * PI;
    let start = chain[0].0;
    // The chain's duplicated period endpoint (u = end, same point as start)
    // becomes an interior vertex of the wrapped slice.
    out.push((end, chain_interp(chain, end)));
    for &(u, v) in chain {
        if u > start + U_EPS && u < b2 - U_EPS {
            out.push((u + 2.0 * PI, v));
        }
    }
    out.push((b, chain_interp(chain, b2.max(start))));
    out
}

/// Interpret a sampled intersection polyline as a closed single-valued
/// profile v = f(u) around the cylinder. Returns `None` when the samples do
/// not wind monotonically around the full circumference (e.g. a bounded
/// cylinder-cylinder quartic arc) — callers must leave such faces unsplit.
pub(crate) fn profile_from_samples(cyl: &CylinderSurface, points: &[Point3]) -> Option<CylProfile> {
    if points.len() < 8 {
        return None;
    }

    /// Accumulate (unwrapped u, v) pairs; `None` when u is not monotonic.
    fn build<'a>(
        cyl: &CylinderSurface,
        pts: impl Iterator<Item = &'a Point3>,
    ) -> Option<(Vec<f64>, Vec<f64>)> {
        let mut us: Vec<f64> = Vec::new();
        let mut vs: Vec<f64> = Vec::new();
        let mut dir = 0.0f64;
        for p in pts {
            let (u, v) = uv_of(p, cyl);
            match us.last() {
                None => {
                    us.push(u);
                    vs.push(v);
                }
                Some(&prev) => {
                    let mut du = (u - prev).rem_euclid(2.0 * PI);
                    if du > PI {
                        du -= 2.0 * PI;
                    }
                    if du.abs() < U_EPS {
                        continue; // duplicate angle
                    }
                    if dir == 0.0 {
                        dir = du.signum();
                    } else if du.signum() != dir {
                        return None; // not single-valued in u
                    }
                    us.push(prev + du);
                    vs.push(v);
                }
            }
        }
        if us.len() < 8 {
            return None;
        }
        Some((us, vs))
    }

    let (us, vs) = build(cyl, points.iter())?;
    let total = us.last().unwrap() - us[0];

    // Closed around the axis: the samples must cover (almost) a full turn.
    // SSI samples the ellipse at uniform angles without repeating the start,
    // so |total| lands one step short of 2π.
    let step = total.abs() / (us.len() - 1) as f64;
    if (total.abs() - 2.0 * PI).abs() > step + 1e-6 {
        return None;
    }

    if total < 0.0 {
        // Descending: rebuild ascending from the reversed point order.
        let (us, vs) = build(cyl, points.iter().rev())?;
        return Some(CylProfile { us, vs });
    }
    Some(CylProfile { us, vs })
}

impl CylProfile {
    /// Evaluate f(u) with periodic wrap-around and linear interpolation.
    pub(crate) fn eval(&self, u: f64) -> f64 {
        let n = self.us.len();
        let u0 = self.us[0];
        // Map u into [u0, u0 + 2π).
        let mut x = (u - u0).rem_euclid(2.0 * PI) + u0;
        if x >= u0 + 2.0 * PI - U_EPS {
            x = u0;
        }
        // Binary search for the containing segment; the final segment wraps
        // from us[n−1] back to us[0] + 2π.
        match self
            .us
            .binary_search_by(|a| a.partial_cmp(&x).unwrap_or(std::cmp::Ordering::Equal))
        {
            Ok(i) => self.vs[i],
            Err(0) => self.vs[0],
            Err(i) if i >= n => {
                // Wrap segment: us[n−1] .. us[0]+2π
                let span = self.us[0] + 2.0 * PI - self.us[n - 1];
                let t = if span.abs() < U_EPS {
                    0.0
                } else {
                    (x - self.us[n - 1]) / span
                };
                self.vs[n - 1] + t * (self.vs[0] - self.vs[n - 1])
            }
            Err(i) => {
                let (ua, ub) = (self.us[i - 1], self.us[i]);
                let t = if (ub - ua).abs() < U_EPS {
                    0.0
                } else {
                    (x - ua) / (ub - ua)
                };
                self.vs[i - 1] + t * (self.vs[i] - self.vs[i - 1])
            }
        }
    }

    /// Does the profile own a vertex at (an unwrapped translate of) `u`?
    fn has_node(&self, u: f64) -> bool {
        let u0 = self.us[0];
        let x = u0 + (u - u0).rem_euclid(2.0 * PI);
        self.us.iter().any(|&pu| {
            (pu - x).abs() < U_EPS || (pu - x + 2.0 * PI).abs() < U_EPS
        })
    }
}

impl CylBand {
    /// Lower chain value at u.
    pub(crate) fn lo_at(&self, u: f64) -> f64 {
        chain_interp(&self.lo, u)
    }

    /// Upper chain value at u.
    pub(crate) fn hi_at(&self, u: f64) -> f64 {
        chain_interp(&self.hi, u)
    }

    /// Start of the band's u-interval.
    fn u_start(&self) -> f64 {
        self.lo[0].0.min(self.hi[0].0)
    }

    /// End of the band's u-interval.
    fn u_end(&self) -> f64 {
        self.lo[self.lo.len() - 1].0.max(self.hi[self.hi.len() - 1].0)
    }
}

/// Parse a cylindrical face into band form. Handles three loop shapes:
/// degenerate seam loops (full tube), 4-vertex rectangles, and the dense
/// two-chain loops this module itself emits. Returns `None` for anything
/// else.
pub(crate) fn parse_band(brep: &BRepSolid, face_id: FaceId) -> Option<(CylinderSurface, CylBand)> {
    let face = &brep.topology.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];
    let cyl = surface.as_any().downcast_ref::<CylinderSurface>()?.clone();

    let verts: Vec<Point3> = brep
        .topology
        .loop_half_edges(face.outer_loop)
        .map(|he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
        .collect();
    // Faces with holes are outside this family.
    if !face.inner_loops.is_empty() {
        return None;
    }

    let raw_uvs: Vec<(f64, f64)> = verts.iter().map(|p| uv_of(p, &cyl)).collect();

    // Degenerate full-tube loop: all vertices on the seam angle, two v levels.
    let all_same_u = raw_uvs
        .iter()
        .all(|(u, _)| angle_close(*u, raw_uvs[0].0, 1e-6));
    if all_same_u {
        let v_min = raw_uvs.iter().map(|(_, v)| *v).fold(f64::MAX, f64::min);
        let v_max = raw_uvs.iter().map(|(_, v)| *v).fold(f64::MIN, f64::max);
        if v_max - v_min < V_EPS {
            return None;
        }
        let u0 = raw_uvs[0].0;
        return Some((
            cyl,
            CylBand {
                lo: vec![(u0, v_min), (u0 + 2.0 * PI, v_min)],
                hi: vec![(u0, v_max), (u0 + 2.0 * PI, v_max)],
                full_wrap: true,
            },
        ));
    }

    // Two-chain loop (covers 4-vertex rectangles too): the loop must
    // decompose into exactly two monotonic runs of angle — the lower chain
    // walked one way and the upper chain walked back. Runs tolerate
    // zero-Δu steps (seam edges, pinch columns), and the two chains'
    // ranges may differ by up to ~one column (a pinch collapsed into a
    // single shared vertex after sewing/vertex-merging shortens one
    // chain). This mirrors `tessellate_ruled_two_chain` in
    // vcad-kernel-tessellate — the parser and the renderer must accept
    // the same loop family or split faces silently degrade to
    // full-rectangle geometry.
    let uvs = raw_uvs;
    let n = uvs.len();
    if n < 4 {
        return None;
    }
    // A step of exactly half a turn is directionally ambiguous (an
    // antipodal 2-point ring reads the same forwards and backwards). The
    // two chains of a valid band travel in opposite directions, so try the
    // four sign policies (constant, and flipping at the first vertical
    // connector between the chains); accept whichever parses into a valid
    // two-run band.
    for policy in [
        TiePolicy::Const(1.0),
        TiePolicy::Const(-1.0),
        TiePolicy::FlipAtConnector(1.0),
        TiePolicy::FlipAtConnector(-1.0),
    ] {
        if let Some(band) = try_parse_two_chain(&uvs, policy) {
            return Some((cyl, band));
        }
    }
    None
}

/// How to resolve directionally ambiguous exactly-π unwrap steps.
#[derive(Clone, Copy)]
enum TiePolicy {
    /// All ties take this sign.
    Const(f64),
    /// Ties take this sign until the first vertical connector (Δu ≈ 0,
    /// Δv ≠ 0 — the edge joining the two chains), then the opposite.
    FlipAtConnector(f64),
}

/// Attempt the two-run decomposition of a loop's (u, v) sequence, resolving
/// exactly-π steps with `policy`.
fn try_parse_two_chain(uvs: &[(f64, f64)], policy: TiePolicy) -> Option<CylBand> {
    let n = uvs.len();
    let mut unwrapped: Vec<f64> = Vec::with_capacity(n);
    unwrapped.push(uvs[0].0);
    let mut connector_seen = false;
    for (k, uv) in uvs.iter().enumerate().skip(1) {
        let prev = *unwrapped.last().unwrap();
        let mut du = (uv.0 - prev).rem_euclid(2.0 * PI);
        if du > PI {
            du -= 2.0 * PI;
        }
        if du.abs() < U_EPS && (uv.1 - uvs[k - 1].1).abs() > V_EPS {
            connector_seen = true;
        }
        if (du.abs() - PI).abs() < 1e-9 {
            let sign = match policy {
                TiePolicy::Const(s) => s,
                TiePolicy::FlipAtConnector(s) => {
                    if connector_seen {
                        -s
                    } else {
                        s
                    }
                }
            };
            du = sign * PI;
        }
        unwrapped.push(prev + du);
    }
    let mut run_dir = 0i8;
    let mut flips: Vec<usize> = Vec::new();
    for i in 0..n - 1 {
        let du = unwrapped[i + 1] - unwrapped[i];
        let d = if du > U_EPS {
            1i8
        } else if du < -U_EPS {
            -1
        } else {
            0
        };
        if d == 0 {
            continue;
        }
        if run_dir == 0 {
            run_dir = d;
        } else if d != run_dir {
            flips.push(i);
            run_dir = d;
        }
    }
    if flips.len() != 1 || run_dir == 0 {
        band_dbg!(
            "parse_band: {} flips (need 1), run_dir {}, n {}, uvs head {:?} tail {:?}",
            flips.len(),
            run_dir,
            n,
            &uvs[..n.min(4)],
            &uvs[n.saturating_sub(3)..]
        );
        return None;
    }
    // The run boundary may be preceded by zero-Δu connector steps (the
    // vertical edge joining the chains at a shared column, or a collapsed
    // pinch). Those connector vertices belong to the SECOND chain — leaving
    // them on the first truncates the second chain's range and the clamped
    // interpolation would fabricate a wedge of area.
    let mut split_at = flips[0] + 1;
    while split_at > 1 && (unwrapped[split_at - 1] - unwrapped[split_at - 2]).abs() <= U_EPS {
        split_at -= 1;
    }
    let mut chain_a: Chain = (0..split_at).map(|i| (unwrapped[i], uvs[i].1)).collect();
    let mut chain_b: Chain = (split_at..n).map(|i| (unwrapped[i], uvs[i].1)).collect();
    if chain_a.len() < 2 || chain_b.len() < 2 {
        band_dbg!("parse_band: short chains {} {}", chain_a.len(), chain_b.len());
        return None;
    }
    if chain_a[0].0 > chain_a[chain_a.len() - 1].0 {
        chain_a.reverse();
    }
    if chain_b[0].0 > chain_b[chain_b.len() - 1].0 {
        chain_b.reverse();
    }
    let shift = ((chain_a[0].0 - chain_b[0].0) / (2.0 * PI)).round() * 2.0 * PI;
    for p in &mut chain_b {
        p.0 += shift;
    }
    let (a0, a1) = (chain_a[0].0, chain_a[chain_a.len() - 1].0);
    let (b0, b1) = (chain_b[0].0, chain_b[chain_b.len() - 1].0);
    let span = (a1 - a0).max(b1 - b0);
    if !(U_EPS..=2.0 * PI + 1e-6).contains(&span) {
        band_dbg!("parse_band: bad span {span}");
        return None;
    }
    // A pinch collapsed into a single shared vertex (realize dedups the
    // duplicate point) shortens one chain by up to one of ITS OWN steps.
    // Chains now carry heterogeneous vertex spacings (a sag-dense rim vs a
    // profile at SSI sampling), so the slack must be the largest actual
    // step, not the average.
    let max_step = |c: &Chain| {
        c.windows(2)
            .map(|w| w[1].0 - w[0].0)
            .fold(0.0f64, f64::max)
    };
    let range_tol = (1.5 * max_step(&chain_a).max(max_step(&chain_b))).max(1e-6);
    if (a0 - b0).abs() > range_tol || (a1 - b1).abs() > range_tol {
        band_dbg!(
            "parse_band: range mismatch a=[{a0:.6},{a1:.6}] b=[{b0:.6},{b1:.6}] tol {range_tol:.6}"
        );
        return None;
    }
    let full_wrap = (span - 2.0 * PI).abs() < 1e-6;

    // Chains must not cross (a band has a well-defined lower/upper chain).
    // Compare on the union of both chains' sample angles.
    let mut probe_us: Vec<f64> = chain_a.iter().map(|p| p.0).collect();
    probe_us.extend(chain_b.iter().map(|p| p.0));
    probe_us.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    probe_us.dedup_by(|x, y| (*x - *y).abs() < U_EPS);
    let a_below = probe_us
        .iter()
        .all(|&u| chain_interp(&chain_a, u) <= chain_interp(&chain_b, u) + V_EPS);
    let b_below = probe_us
        .iter()
        .all(|&u| chain_interp(&chain_b, u) <= chain_interp(&chain_a, u) + V_EPS);
    let (mut lo, mut hi) = if a_below {
        (chain_a, chain_b)
    } else if b_below {
        (chain_b, chain_a)
    } else {
        band_dbg!("parse_band: chains cross");
        return None;
    };
    // Close residual end slack: both chains must span the same interval or
    // downstream splits clamp-extend the shorter one into phantom area. At
    // a collapsed pinch the extension point IS the pinch point (the longer
    // chain's endpoint), so this is exact, not a fudge.
    let (start_u, end_u) = (lo[0].0.min(hi[0].0), lo[lo.len() - 1].0.max(hi[hi.len() - 1].0));
    let (lo_head_v, lo_tail_v) = (lo[0].1, lo[lo.len() - 1].1);
    let (hi_head_v, hi_tail_v) = (hi[0].1, hi[hi.len() - 1].1);
    for (c, other_head, other_tail) in [
        (&mut lo, hi_head_v, hi_tail_v),
        (&mut hi, lo_head_v, lo_tail_v),
    ] {
        if c[0].0 > start_u + U_EPS {
            // The missing endpoint is the collapsed pinch — i.e. the OTHER
            // chain's endpoint, where the two chains met.
            c.insert(0, (start_u, other_head));
        }
        if c[c.len() - 1].0 < end_u - U_EPS {
            c.push((end_u, other_tail));
        }
    }
    Some(CylBand { lo, hi, full_wrap })
}

/// Are two angles equal on the circle (mod 2π)?
fn angle_close(a: f64, b: f64, tol: f64) -> bool {
    let mut d = (a - b).rem_euclid(2.0 * PI);
    if d > PI {
        d = 2.0 * PI - d;
    }
    d.abs() < tol
}

/// Does a chain own a vertex at `u` (exact column, within U_EPS)?
fn chain_has_node(chain: &Chain, u: f64) -> bool {
    let i = chain.partition_point(|p| p.0 < u - U_EPS);
    i < chain.len() && (chain[i].0 - u).abs() < U_EPS
}

/// Build the min- or max-envelope of the profile `f` and a parent chain
/// over `[a, b]`, preserving vertex provenance: on stretches where `f`
/// wins, only f's own vertices are emitted; where the chain wins, only the
/// chain's own vertices; the grid nodes at switches (crossings, inserted
/// into `grid` by the caller) and the interval endpoints are always
/// emitted with exact envelope values.
///
/// `grid` must contain every node of both curves inside `[a, b]` plus all
/// crossing points, sorted ascending; `a` and `b` must be grid nodes.
#[allow(clippy::too_many_arguments)]
fn envelope_chain(
    grid: &[f64],
    a: f64,
    b: f64,
    f: &CylProfile,
    chain: &Chain,
    chain_wraps: bool,
    take_min: bool,
) -> Chain {
    let chain_val = |u: f64| -> f64 {
        if chain_wraps {
            let end = chain[chain.len() - 1].0;
            if u > end + U_EPS {
                return chain_interp(chain, chain[0].0 + (u - end));
            }
        }
        chain_interp(chain, u)
    };
    // A sparse chain (an analytic seam loop or 4-corner rectangle, < 8
    // nodes) has no explicit polyline to conform to — treat every grid
    // node as its own so the envelope densifies there, as the paired-column
    // implementation did. Dense (frozen / profile) chains pin their exact
    // vertex set.
    let sparse = chain.len() < 8;
    let chain_node = |u: f64| -> bool {
        if sparse || chain_has_node(chain, u) {
            return true;
        }
        if chain_wraps {
            let end = chain[chain.len() - 1].0;
            if u > end + U_EPS {
                return chain_has_node(chain, chain[0].0 + (u - end));
            }
        }
        false
    };
    let env = |u: f64| -> f64 {
        let fv = f.eval(u);
        let cv = chain_val(u);
        if take_min {
            fv.min(cv)
        } else {
            fv.max(cv)
        }
    };
    let mut out: Chain = vec![(a, env(a))];
    // A wrapped region ([a, b] extending past the parent's period end)
    // needs the +2π translates of the grid nodes too.
    let translated: Vec<f64> = if b > grid[grid.len() - 1] + U_EPS {
        grid.iter()
            .map(|&u| u + 2.0 * PI)
            .filter(|&u| u < b - U_EPS)
            .collect()
    } else {
        Vec::new()
    };
    for &u in grid.iter().chain(translated.iter()) {
        if u <= a + U_EPS || u >= b - U_EPS {
            continue;
        }
        let fv = f.eval(u);
        let cv = chain_val(u);
        let f_wins = if take_min { fv <= cv + V_EPS } else { fv >= cv - V_EPS };
        let c_wins = if take_min { cv <= fv + V_EPS } else { cv >= fv - V_EPS };
        let emit = (f_wins && f.has_node(u)) || (c_wins && chain_node(u)) || (f_wins && c_wins);
        if emit {
            out.push((u, env(u)));
        }
    }
    out.push((b, env(b)));
    out
}

/// Split `band` by the closed profile `f`, producing the sub-bands below f
/// (`lo ≤ v ≤ min(f, hi)`) and above f (`max(f, lo) ≤ v ≤ hi`). Returns
/// `None` when the profile misses the band interior (no split needed).
pub(crate) fn split_band_by_profile(
    band: &CylBand,
    f: &CylProfile,
) -> Option<(Vec<CylBand>, Vec<CylBand>)> {
    let (u_start, u_end) = (band.u_start(), band.u_end());

    // Analysis grid: both chains' nodes ∪ profile nodes mapped into the
    // band's u-interval ∪ profile/chain crossing points. Between adjacent
    // grid nodes all three curves are linear, so sign-change localization
    // below is exact.
    let mut grid: Vec<f64> = Vec::new();
    grid.push(u_start);
    grid.push(u_end);
    for p in band.lo.iter().chain(band.hi.iter()) {
        if p.0 > u_start - U_EPS && p.0 < u_end + U_EPS {
            grid.push(p.0.clamp(u_start, u_end));
        }
    }
    for &pu in &f.us {
        // Every 2π translate of pu that lands inside [u_start, u_end].
        let mut x = u_start + (pu - u_start).rem_euclid(2.0 * PI);
        while x <= u_end + U_EPS {
            if x >= u_start - U_EPS {
                grid.push(x.clamp(u_start, u_end));
            }
            x += 2.0 * PI;
        }
    }
    grid.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    grid.dedup_by(|a, b| (*a - *b).abs() < U_EPS);

    // Insert exact crossing points of f against each chain.
    let mut refined: Vec<f64> = Vec::with_capacity(grid.len() * 2);
    for i in 0..grid.len() {
        refined.push(grid[i]);
        if i + 1 == grid.len() {
            break;
        }
        let (ua, ub) = (grid[i], grid[i + 1]);
        for lower in [true, false] {
            let g = |u: f64| -> f64 {
                if lower {
                    f.eval(u) - band.lo_at(u)
                } else {
                    band.hi_at(u) - f.eval(u)
                }
            };
            let (ga, gb) = (g(ua), g(ub));
            if (ga > 0.0) != (gb > 0.0) && (ga - gb).abs() > 1e-15 {
                let t = ga / (ga - gb);
                let uc = ua + t * (ub - ua);
                if uc > ua + U_EPS && uc < ub - U_EPS {
                    refined.push(uc);
                }
            }
        }
    }
    refined.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    refined.dedup_by(|a, b| (*a - *b).abs() < U_EPS);

    let fv: Vec<f64> = refined.iter().map(|&u| f.eval(u)).collect();
    let lo_v: Vec<f64> = refined.iter().map(|&u| band.lo_at(u)).collect();
    let hi_v: Vec<f64> = refined.iter().map(|&u| band.hi_at(u)).collect();

    // Region widths on the grid.
    let below_w: Vec<f64> = (0..refined.len())
        .map(|i| fv[i].min(hi_v[i]) - lo_v[i])
        .collect();
    let above_w: Vec<f64> = (0..refined.len())
        .map(|i| hi_v[i] - fv[i].max(lo_v[i]))
        .collect();

    let below = extract_regions(band, f, &refined, &below_w, true);
    let above = extract_regions(band, f, &refined, &above_w, false);

    // A real split leaves material on both sides; otherwise the profile
    // missed the band (entirely above or below it).
    if below.is_empty() || above.is_empty() {
        return None;
    }
    // Also require the split to actually change the geometry: if the below
    // side reproduces the whole band, nothing was cut.
    let below_area: f64 = below.iter().map(band_area).sum();
    let above_area: f64 = above.iter().map(band_area).sum();
    let band_a = band_area(band);
    if below_area < 1e-9 * band_a || above_area < 1e-9 * band_a {
        return None;
    }
    Some((below, above))
}

/// Approximate (u, v) parameter area of a band (trapezoid rule over the
/// union of both chains' nodes).
pub(crate) fn band_area(b: &CylBand) -> f64 {
    let mut us: Vec<f64> = b.lo.iter().map(|p| p.0).collect();
    us.extend(b.hi.iter().map(|p| p.0));
    us.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    us.dedup_by(|x, y| (*x - *y).abs() < U_EPS);
    let mut a = 0.0;
    for w in us.windows(2) {
        let w0 = (b.hi_at(w[0]) - b.lo_at(w[0])).max(0.0);
        let w1 = (b.hi_at(w[1]) - b.lo_at(w[1])).max(0.0);
        a += 0.5 * (w0 + w1) * (w[1] - w[0]);
    }
    a
}

/// Extract the maximal positive-width sub-bands of one side of a profile
/// split. `widths[i]` is the region's height at `grid[i]`; `take_min`
/// selects the below side (`lo ≤ v ≤ min(f, hi)`) vs the above side
/// (`max(f, lo) ≤ v ≤ hi`). Each region's chains are built by slicing the
/// surviving parent chain verbatim and taking the provenance-preserving
/// envelope for the profile-bounded side.
fn extract_regions(
    band: &CylBand,
    f: &CylProfile,
    grid: &[f64],
    widths: &[f64],
    take_min: bool,
) -> Vec<CylBand> {
    let n = grid.len();
    let positive: Vec<bool> = widths.iter().map(|&w| w > V_EPS).collect();

    // Slice a parent rim to [a, b]. A sparse rim (analytic seam loop /
    // 4-corner rectangle) is densified at the analysis-grid columns — it
    // has no explicit polyline to conform to and the two-chain renderer
    // needs comparable rail density. A dense rim stays verbatim.
    let rim_slice = |chain: &Chain, a: f64, b: f64| -> Chain {
        let wrapping = band.full_wrap && b > band.u_end() + U_EPS;
        let mut out = if wrapping {
            cut_chain_wrap(chain, a, b)
        } else {
            cut_chain(chain, a, b)
        };
        if chain.len() < 8 {
            let period = 2.0 * PI;
            let mut extra: Chain = Vec::new();
            for &u in grid {
                for cand in [u, u + period] {
                    if cand > a + U_EPS
                        && cand < b - U_EPS
                        && !out.iter().any(|p| (p.0 - cand).abs() < U_EPS)
                    {
                        let base = if cand > band.u_end() + U_EPS {
                            chain[0].0 + (cand - band.u_end())
                        } else {
                            cand
                        };
                        extra.push((cand, chain_interp(chain, base)));
                    }
                }
            }
            out.extend(extra);
            out.sort_by(|x, y| {
                x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        out
    };
    let make = |a: f64, b: f64, wrap: bool| -> CylBand {
        if take_min {
            // below: lo = parent lo slice, hi = min(f, parent hi)
            let lo = rim_slice(&band.lo, a, b);
            let hi = envelope_chain(grid, a, b, f, &band.hi, band.full_wrap, true);
            CylBand {
                lo,
                hi,
                full_wrap: wrap,
            }
        } else {
            let lo = envelope_chain(grid, a, b, f, &band.lo, band.full_wrap, false);
            let hi = rim_slice(&band.hi, a, b);
            CylBand {
                lo,
                hi,
                full_wrap: wrap,
            }
        }
    };

    if positive.iter().all(|&p| p) {
        // Region spans the whole interval — a single band, preserving wrap.
        return vec![make(grid[0], grid[n - 1], band.full_wrap)];
    }
    if positive.iter().all(|&p| !p) {
        return Vec::new();
    }

    if band.full_wrap {
        // Circular run extraction on the m unique nodes (grid's final node
        // duplicates the first angle at +2π).
        let m = n - 1;
        // Find a non-positive node to anchor the scan.
        let anchor = (0..m).find(|&i| !positive[i]).unwrap_or(0);
        let mut out = Vec::new();
        let mut k = 0usize;
        while k < m {
            let idx = (anchor + k) % m;
            if !positive[idx] {
                k += 1;
                continue;
            }
            // Positive circular run starting at idx.
            let run_start = k;
            let mut run_len = 0usize;
            while run_len < m && positive[(anchor + run_start + run_len) % m] {
                run_len += 1;
            }
            // Bounding pinch nodes on either side.
            let s = run_start - 1; // anchor is non-positive, so run_start ≥ 1
            let e = run_start + run_len; // first non-positive after the run
            let unwrap_at = |k: usize| -> f64 {
                let idx = (anchor + k) % m;
                let turns = ((anchor + k) / m) as f64;
                grid[idx] + turns * 2.0 * PI
            };
            let (a, b) = (unwrap_at(s), unwrap_at(e));
            if b > a + U_EPS {
                out.push(make(a, b, false));
            }
            k = run_start + run_len;
        }
        out
    } else {
        let mut out = Vec::new();
        let mut i = 0;
        while i < n {
            if !positive[i] {
                i += 1;
                continue;
            }
            let mut j = i;
            while j < n && positive[j] {
                j += 1;
            }
            // Include bounding pinch nodes when present.
            let s = i.saturating_sub(1);
            let e = j.min(n - 1);
            if grid[e] > grid[s] + U_EPS {
                out.push(make(grid[s], grid[e], false));
            }
            i = j;
        }
        out
    }
}

/// Split a band at a constant angle (a vertical intersection line at
/// `u_split`). Returns the two halves, or `None` when the line misses the
/// band interior.
pub(crate) fn split_band_at_u(band: &CylBand, u_split: f64) -> Option<(CylBand, CylBand)> {
    let (u_start, u_end) = (band.u_start(), band.u_end());
    // Map u_split into the band's interval.
    let x = u_start + (u_split - u_start).rem_euclid(2.0 * PI);
    if x <= u_start + 1e-6 || x >= u_end - 1e-6 {
        return None;
    }
    let cut = |a: f64, b: f64| -> CylBand {
        CylBand {
            lo: cut_chain(&band.lo, a, b),
            hi: cut_chain(&band.hi, a, b),
            full_wrap: false,
        }
    };
    Some((cut(u_start, x), cut(x, u_end)))
}

/// Materialize band faces in the topology, replacing `parent`. Returns the
/// new face ids (the parent is removed from its shell and the face arena).
pub(crate) fn realize_bands(
    brep: &mut BRepSolid,
    parent: FaceId,
    cyl: &CylinderSurface,
    bands: &[CylBand],
) -> Vec<FaceId> {
    let surface_index = brep.topology.faces[parent].surface_index;
    let orientation = brep.topology.faces[parent].orientation;
    let shell = brep.topology.faces[parent].shell;

    let mut new_faces = Vec::with_capacity(bands.len());
    for band in bands {
        // Each chain is emitted VERBATIM — its vertices are exactly the
        // parent chain / profile / cut vertices the neighboring faces also
        // carry, which is the whole point of the chain representation.
        // Pinch endpoints (chains meeting at a shared point) may duplicate;
        // repair collapses the resulting zero-length half-edges later.
        let mut loop_pts: Vec<Point3> =
            Vec::with_capacity(band.lo.len() + band.hi.len());
        // Bottom chain, ascending u.
        for &(u, v) in &band.lo {
            loop_pts.push(point_at(cyl, u, v));
        }
        // Top chain, descending u.
        for &(u, v) in band.hi.iter().rev() {
            loop_pts.push(point_at(cyl, u, v));
        }
        // Keep pinch-column duplicates: dropping the shared endpoint
        // shortens one chain, the re-parsed band then carries end slack,
        // and the NEXT split's region grid clamp-extends the short chain
        // flat — phantom overlapping geometry that compounds per split.
        // Zero-length half-edges are collapsed by repair after all splits.
        loop_pts.dedup_by(|a, b| (*a - *b).norm() < 1e-12);
        // A real face needs at least 3 distinct positions.
        let mut distinct = 0usize;
        for (i, p) in loop_pts.iter().enumerate() {
            if loop_pts[..i].iter().all(|q| (*p - *q).norm() > 1e-9) {
                distinct += 1;
            }
        }
        if distinct < 3 {
            continue;
        }

        let mut hes = Vec::with_capacity(loop_pts.len());
        for p in &loop_pts {
            let vid = crate::split::find_or_create_vertex(brep, p, 1e-9);
            hes.push(brep.topology.add_half_edge(vid));
        }
        let loop_id = brep.topology.add_loop(&hes);
        let fid = brep.topology.add_face(loop_id, surface_index, orientation);
        band_dbg!(
            "realize {:?}: lo {} pts u[{:.4},{:.4}] v[{:.3},{:.3}], hi {} pts v[{:.3},{:.3}] wrap {}",
            fid,
            band.lo.len(),
            band.lo[0].0,
            band.lo[band.lo.len() - 1].0,
            band.lo.iter().map(|p| p.1).fold(f64::MAX, f64::min),
            band.lo.iter().map(|p| p.1).fold(f64::MIN, f64::max),
            band.hi.len(),
            band.hi.iter().map(|p| p.1).fold(f64::MAX, f64::min),
            band.hi.iter().map(|p| p.1).fold(f64::MIN, f64::max),
            band.full_wrap
        );
        new_faces.push(fid);
    }

    if new_faces.is_empty() {
        return vec![parent];
    }

    if let Some(shell_id) = shell {
        for &f in &new_faces {
            brep.topology.shells[shell_id].faces.push(f);
            brep.topology.faces[f].shell = Some(shell_id);
        }
        brep.topology.shells[shell_id]
            .faces
            .retain(|&f| f != parent);
    }
    brep.topology.faces.remove(parent);
    new_faces
}

/// Split a cylindrical face along an oblique sampled intersection curve
/// (e.g. the ellipse where a tilted plane crosses the cylinder). Returns
/// `None` when the face or curve is outside the band family; the caller
/// then falls back to leaving the face unsplit.
pub(crate) fn split_cylindrical_face_by_sampled(
    brep: &mut BRepSolid,
    face_id: FaceId,
    points: &[Point3],
) -> Option<SplitResult> {
    let (cyl, band) = parse_band(brep, face_id)?;
    let profile = profile_from_samples(&cyl, points)?;
    // A profile that misses this band's interior is a legitimate no-op
    // (the curve crosses the cylinder elsewhere): report "split into just
    // this face" so the caller keeps it without logging a failure.
    let Some((below, above)) = split_band_by_profile(&band, &profile) else {
        return Some(SplitResult {
            sub_faces: vec![face_id],
        });
    };
    let mut bands = below;
    bands.extend(above);
    let sub_faces = realize_bands(brep, face_id, &cyl, &bands);
    if sub_faces.len() < 2 {
        return None;
    }
    Some(SplitResult { sub_faces })
}

/// Split a wavy band face by a constant-v circle (a perpendicular plane
/// cut). Rectangular faces are NOT routed here — the legacy splitter
/// preserves their degenerate seam-loop representation.
pub(crate) fn split_wavy_band_by_circle(
    brep: &mut BRepSolid,
    face_id: FaceId,
    circle: &vcad_kernel_geom::Circle3d,
    require_wavy: bool,
    segments: u32,
) -> Option<SplitResult> {
    let (cyl, band) = parse_band(brep, face_id)?;
    if require_wavy && band_is_rectangular(&band) {
        return None;
    }
    let v_c = (circle.center - cyl.center).dot(cyl.axis.as_ref());
    // The cut ring must be emitted at the canonical sag-dense grid — a
    // 2-node constant-v profile would realize the ring as a chord pair,
    // which neither the tessellator (density gate) nor the frozen caps the
    // ring pairs with can conform to.
    let u0 = band.u_start();
    let p0 = point_at(&cyl, u0, v_c);
    let ring = crate::split::canonical_arc_points(
        circle.center,
        circle.radius,
        *cyl.axis.as_ref(),
        p0,
        p0,
        segments,
    );
    let uvs: Vec<(f64, f64)> = ring.iter().map(|p| uv_of(p, &cyl)).collect();
    // Unwrap ascending from u0.
    let mut us: Vec<f64> = Vec::with_capacity(uvs.len());
    let mut vs: Vec<f64> = Vec::with_capacity(uvs.len());
    for (u, _v) in &uvs {
        let x = match us.last() {
            None => *u,
            Some(&prev) => prev + (*u - prev).rem_euclid(2.0 * PI),
        };
        if us.last().is_some_and(|&prev| (x - prev).abs() < U_EPS) {
            continue;
        }
        us.push(x);
        vs.push(v_c);
    }
    // Drop a duplicated closing point one full turn up.
    while us.len() > 1 && us[us.len() - 1] - us[0] > 2.0 * PI - U_EPS {
        us.pop();
        vs.pop();
    }
    let profile = CylProfile { us, vs };
    let (below, above) = split_band_by_profile(&band, &profile)?;
    let mut bands = below;
    bands.extend(above);
    let sub_faces = realize_bands(brep, face_id, &cyl, &bands);
    if sub_faces.len() < 2 {
        return None;
    }
    Some(SplitResult { sub_faces })
}

/// Split a wavy band face by an axis-parallel line at fixed angle.
/// Rectangular faces keep the legacy path.
pub(crate) fn split_wavy_band_by_line(
    brep: &mut BRepSolid,
    face_id: FaceId,
    line: &vcad_kernel_geom::Line3d,
    require_wavy: bool,
) -> Option<SplitResult> {
    let (cyl, band) = parse_band(brep, face_id)?;
    if require_wavy && band_is_rectangular(&band) {
        return None;
    }
    // The line must be axis-parallel and on the surface.
    let d = line.direction.normalize();
    if d.cross(cyl.axis.as_ref()).norm() > 1e-6 {
        return None;
    }
    let (u_split, _) = uv_of(&line.origin, &cyl);
    let (left, right) = split_band_at_u(&band, u_split)?;
    let sub_faces = realize_bands(brep, face_id, &cyl, &[left, right]);
    if sub_faces.len() < 2 {
        return None;
    }
    Some(SplitResult { sub_faces })
}

/// Is the band a plain rectangle (both chains constant v)? Those faces are
/// handled by the pre-existing splitters and keep their degenerate loop
/// representations.
pub(crate) fn band_is_rectangular(band: &CylBand) -> bool {
    let flat = |c: &Chain| {
        let (mn, mx) = c
            .iter()
            .fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
        mx - mn < 1e-9
    };
    flat(&band.lo) && flat(&band.hi)
}

/// Does this cylindrical face carry a wavy (non-constant-v) band boundary?
/// Such faces come exclusively from oblique boolean splits and must be
/// handled by the band machinery — the legacy rectangular splitters would
/// reconstruct them as full-height rectangles and emit phantom geometry.
pub(crate) fn face_is_wavy_band(brep: &BRepSolid, face_id: FaceId) -> bool {
    parse_band(brep, face_id).is_some_and(|(_, band)| !band_is_rectangular(&band))
}

/// Interior sample point for a band-shaped cylindrical face: the middle of
/// the widest column gap, evaluated on the surface. Guaranteed to lie
/// strictly inside the band (away from both chains), unlike the u-range
/// midpoint heuristic which can leave wavy faces entirely.
pub(crate) fn band_sample_point(brep: &BRepSolid, face_id: FaceId) -> Option<Point3> {
    let (cyl, band) = parse_band(brep, face_id)?;
    if band_is_rectangular(&band) {
        return None; // legacy sampling is fine (and battle-tested) for these
    }
    let mut us: Vec<f64> = band.lo.iter().map(|p| p.0).collect();
    us.extend(band.hi.iter().map(|p| p.0));
    us.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    us.dedup_by(|x, y| (*x - *y).abs() < U_EPS);
    if us.len() < 2 {
        return None;
    }
    // Widest column-interval by mean width.
    let mut best = 0usize;
    let mut best_w = f64::MIN;
    for i in 0..us.len() - 1 {
        let w = 0.5
            * ((band.hi_at(us[i]) - band.lo_at(us[i]))
                + (band.hi_at(us[i + 1]) - band.lo_at(us[i + 1])));
        if w > best_w {
            best_w = w;
            best = i;
        }
    }
    if best_w <= V_EPS {
        return None;
    }
    let u = 0.5 * (us[best] + us[best + 1]);
    let v = 0.5 * (band.lo_at(u) + band.hi_at(u));
    Some(point_at(&cyl, u, v))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_band(v0: f64, v1: f64) -> CylBand {
        CylBand {
            lo: vec![(0.0, v0), (2.0 * PI, v0)],
            hi: vec![(0.0, v1), (2.0 * PI, v1)],
            full_wrap: true,
        }
    }

    fn sinusoid(center: f64, amp: f64, phase: f64, n: usize) -> CylProfile {
        let us: Vec<f64> = (0..n).map(|i| 2.0 * PI * i as f64 / n as f64).collect();
        let vs: Vec<f64> = us
            .iter()
            .map(|u| center + amp * (u + phase).sin())
            .collect();
        CylProfile { us, vs }
    }

    #[test]
    fn profile_inside_band_splits_into_two_full_bands() {
        let band = flat_band(0.0, 10.0);
        let f = sinusoid(5.0, 2.0, 0.0, 64);
        let (below, above) = split_band_by_profile(&band, &f).expect("split");
        assert_eq!(below.len(), 1);
        assert_eq!(above.len(), 1);
        assert!(below[0].full_wrap);
        assert!(above[0].full_wrap);
        // Areas partition the parent.
        let total = band_area(&below[0]) + band_area(&above[0]);
        assert!((total - band_area(&band)).abs() < 1e-9 * band_area(&band));
        assert!(below[0].hi.len() >= 64, "profile side keeps its sampling");
    }

    #[test]
    fn dense_rim_stays_verbatim_under_profile_split() {
        // A frozen (sag-dense) rim must keep EXACTLY its own vertex set
        // when a profile splits the band — no columns invented from the
        // profile's sampling. (Sparse analytic rims, by contrast, densify;
        // see profile_inside_band_splits_into_two_full_bands.)
        let n = 100usize;
        let lo: Chain = (0..=n)
            .map(|i| (2.0 * PI * i as f64 / n as f64, 0.0))
            .collect();
        let hi: Chain = (0..=n)
            .map(|i| (2.0 * PI * i as f64 / n as f64, 10.0))
            .collect();
        let band = CylBand {
            lo: lo.clone(),
            hi,
            full_wrap: true,
        };
        let f = sinusoid(5.0, 2.0, 0.123, 64);
        let (below, above) = split_band_by_profile(&band, &f).expect("split");
        assert_eq!(below.len(), 1);
        assert_eq!(above.len(), 1);
        // The rim slice is the parent's own vertices, nothing more.
        assert_eq!(below[0].lo.len(), lo.len(), "dense rim must stay verbatim");
        let total = band_area(&below[0]) + band_area(&above[0]);
        assert!((total - band_area(&band)).abs() < 1e-9 * band_area(&band));
    }

    #[test]
    fn profile_crossing_top_chain_pinches_above_region() {
        let band = flat_band(0.0, 10.0);
        // Sinusoid pokes above the band top: above-region splits into arcs.
        let f = sinusoid(8.0, 4.0, 0.0, 128);
        let (below, above) = split_band_by_profile(&band, &f).expect("split");
        // Below region always spans the full circle (f > 0 everywhere here).
        assert_eq!(below.len(), 1);
        // Above region exists only where f < 10 → a single arc piece
        // (possibly seam-rotated).
        assert!(!above.is_empty());
        let total: f64 =
            below.iter().map(band_area).sum::<f64>() + above.iter().map(band_area).sum::<f64>();
        assert!((total - band_area(&band)).abs() < 1e-6 * band_area(&band));
    }

    #[test]
    fn crossing_profiles_stay_partitioned() {
        // Two sinusoids with different phases cross; sequential splits must
        // keep partitioning exactly (the doc_10 multi-instance regime).
        let band = flat_band(0.0, 10.0);
        let f1 = sinusoid(5.0, 3.0, 0.0, 64);
        let (mut pieces, above) = split_band_by_profile(&band, &f1).expect("first split");
        pieces.extend(above);
        let f2 = sinusoid(5.0, 3.0, 1.3, 64);
        let mut final_pieces = Vec::new();
        for p in &pieces {
            match split_band_by_profile(p, &f2) {
                Some((b, a)) => {
                    final_pieces.extend(b);
                    final_pieces.extend(a);
                }
                None => final_pieces.push(p.clone()),
            }
        }
        let total: f64 = final_pieces.iter().map(band_area).sum();
        assert!(
            (total - band_area(&band)).abs() < 1e-6 * band_area(&band),
            "area {total} vs parent {}",
            band_area(&band)
        );
        assert!(
            final_pieces.len() >= 4,
            "expected ≥4 pieces, got {}",
            final_pieces.len()
        );
    }

    #[test]
    fn split_at_u_partitions() {
        let band = flat_band(0.0, 5.0);
        let (l, r) = split_band_at_u(&band, 1.0).expect("split");
        let total = band_area(&l) + band_area(&r);
        assert!((total - band_area(&band)).abs() < 1e-9 * band_area(&band));
        // Rim conservation: no invented columns beyond the exact cut point.
        assert_eq!(l.lo.len(), 2);
        assert_eq!(r.lo.len(), 2);
    }

    #[test]
    fn split_at_u_preserves_interior_vertices() {
        let band = CylBand {
            lo: vec![(0.0, 0.0), (1.0, 0.0), (3.0, 0.0), (2.0 * PI, 0.0)],
            hi: vec![(0.0, 5.0), (2.5, 5.0), (2.0 * PI, 5.0)],
            full_wrap: true,
        };
        let (l, r) = split_band_at_u(&band, 2.0).expect("split");
        // Left keeps lo vertex at 1.0 verbatim; right keeps 3.0 and hi 2.5.
        assert!(l.lo.iter().any(|p| (p.0 - 1.0).abs() < 1e-12));
        assert!(r.lo.iter().any(|p| (p.0 - 3.0).abs() < 1e-12));
        assert!(r.hi.iter().any(|p| (p.0 - 2.5).abs() < 1e-12));
        // And no chain gained the other chain's columns.
        assert_eq!(l.lo.len(), 3); // 0.0, 1.0, cut@2.0
        assert_eq!(l.hi.len(), 2); // 0.0, cut@2.0
    }
}

#[cfg(test)]
mod parse_tests {
    use super::*;
    use vcad_kernel_primitives::make_cylinder;

    #[test]
    fn parse_primitive_tube_wall() {
        let cyl = make_cylinder(45.0, 13.0, 32);
        let mut parsed = 0;
        for (fid, face) in cyl.topology.faces.iter() {
            let surf = &cyl.geometry.surfaces[face.surface_index];
            if surf.as_any().downcast_ref::<CylinderSurface>().is_some() {
                let verts: Vec<Point3> = cyl
                    .topology
                    .loop_half_edges(face.outer_loop)
                    .map(|he| cyl.topology.vertices[cyl.topology.half_edges[he].origin].point)
                    .collect();
                eprintln!("wall loop verts: {verts:?}");
                match parse_band(&cyl, fid) {
                    Some((_, band)) => {
                        parsed += 1;
                        eprintln!(
                            "band: lo={:?} hi={:?} wrap={}",
                            band.lo, band.hi, band.full_wrap
                        );
                        assert!(band.full_wrap);
                        assert!((band.hi[0].1 - band.lo[0].1 - 13.0).abs() < 1e-9);
                    }
                    None => panic!("wall face failed to parse as band"),
                }
            }
        }
        assert_eq!(parsed, 1);
    }
}

#[cfg(test)]
mod ssi_profile_tests {
    use super::*;
    use crate::ssi;
    use vcad_kernel_math::Transform;
    use vcad_kernel_primitives::{make_cube, make_cylinder};

    #[test]
    fn blade_plane_ellipses_parse_and_split() {
        let cyl_solid = make_cylinder(45.0, 13.0, 32);
        let mut blade = make_cube(23.5, 0.5, 12.57);
        let rot = Transform::rotation_x(39.29_f64.to_radians());
        let tr = Transform::translation(21.5, 0.0, 0.0);
        let combined = tr.then(&rot);
        for (_id, v) in &mut blade.topology.vertices {
            v.point = combined.apply_point(&v.point);
        }
        for s in &mut blade.geometry.surfaces {
            *s = s.transform(&combined);
        }

        // The cylinder's lateral surface
        let wall = cyl_solid
            .geometry
            .surfaces
            .iter()
            .find(|s| s.as_any().downcast_ref::<CylinderSurface>().is_some())
            .unwrap();
        let cyl = wall.as_any().downcast_ref::<CylinderSurface>().unwrap();

        let band = CylBand {
            lo: vec![(0.0, 0.0), (2.0 * PI, 0.0)],
            hi: vec![(0.0, 13.0), (2.0 * PI, 13.0)],
            full_wrap: true,
        };

        let mut sampled_seen = 0;
        let mut parsed_ok = 0;
        let mut split_ok = 0;
        for surf in &blade.geometry.surfaces {
            let curve = ssi::intersect_surfaces(wall.as_ref(), surf.as_ref()).expect("ssi");
            let kind = match &curve {
                ssi::IntersectionCurve::Sampled(pts) => {
                    sampled_seen += 1;
                    match profile_from_samples(cyl, pts) {
                        Some(profile) => {
                            parsed_ok += 1;
                            match split_band_by_profile(&band, &profile) {
                                Some((below, above)) => {
                                    split_ok += 1;
                                    format!(
                                        "Sampled({}) → profile ok → split {}+{}",
                                        pts.len(),
                                        below.len(),
                                        above.len()
                                    )
                                }
                                None => format!("Sampled({}) → profile ok → SPLIT NONE", pts.len()),
                            }
                        }
                        None => {
                            // Diagnose: print the first few (u, v)
                            let uvs: Vec<(f64, f64)> =
                                pts.iter().take(6).map(|p| uv_of(p, cyl)).collect();
                            format!("Sampled({}) → PROFILE NONE, head uvs {uvs:?}", pts.len())
                        }
                    }
                }
                other => format!("{other:?}").chars().take(60).collect::<String>(),
            };
            eprintln!("blade surface × wall: {kind}");
        }
        assert!(sampled_seen >= 2, "expected sampled ellipses");
        assert_eq!(parsed_ok, sampled_seen, "all sampled ellipses must parse");
        assert!(split_ok >= 2, "side-plane ellipses must split the band");
    }
}

#[cfg(test)]
mod realize_tests {
    use super::*;
    use crate::ssi;
    use vcad_kernel_math::{Transform, Vec3};
    use vcad_kernel_primitives::{make_cube, make_cylinder};
    use vcad_kernel_tessellate::tessellate_brep;

    fn mesh_vol_area(mesh: &vcad_kernel_tessellate::TriangleMesh) -> (f64, f64) {
        let (mut vol, mut area) = (0.0, 0.0);
        for t in mesh.indices.chunks(3) {
            let p = |i: u32| {
                let b = i as usize * 3;
                Vec3::new(
                    mesh.vertices[b] as f64,
                    mesh.vertices[b + 1] as f64,
                    mesh.vertices[b + 2] as f64,
                )
            };
            let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
            vol += a.dot(b.cross(c)) / 6.0;
            area += 0.5 * (b - a).cross(c - a).norm();
        }
        (vol, area)
    }

    #[test]
    fn wall_split_by_one_ellipse_preserves_solid() {
        let mut cyl_solid = make_cylinder(45.0, 13.0, 32);
        let (vol0, area0) = mesh_vol_area(&tessellate_brep(&cyl_solid, 64));

        // Build the blade side-plane ellipse via real SSI.
        let mut blade = make_cube(23.5, 0.5, 12.57);
        let combined = Transform::translation(21.5, 0.0, 0.0)
            .then(&Transform::rotation_x(39.29_f64.to_radians()));
        for (_id, v) in &mut blade.topology.vertices {
            v.point = combined.apply_point(&v.point);
        }
        for s in &mut blade.geometry.surfaces {
            *s = s.transform(&combined);
        }
        let wall_idx = cyl_solid
            .geometry
            .surfaces
            .iter()
            .position(|s| s.as_any().downcast_ref::<CylinderSurface>().is_some())
            .unwrap();
        let wall_face = cyl_solid
            .topology
            .faces
            .iter()
            .find(|(_, f)| f.surface_index == wall_idx)
            .map(|(id, _)| id)
            .unwrap();
        let curve = ssi::intersect_surfaces(
            cyl_solid.geometry.surfaces[wall_idx].as_ref(),
            blade.geometry.surfaces[0].as_ref(),
        )
        .expect("ssi");
        let ssi::IntersectionCurve::Sampled(pts) = curve else {
            panic!("expected sampled ellipse, got {curve:?}");
        };
        let result = split_cylindrical_face_by_sampled(&mut cyl_solid, wall_face, &pts)
            .expect("split should apply");
        eprintln!("split into {} sub-faces", result.sub_faces.len());

        let (vol1, area1) = mesh_vol_area(&tessellate_brep(&cyl_solid, 64));
        eprintln!("vol {vol0:.3} -> {vol1:.3}; area {area0:.3} -> {area1:.3}");
        assert!(
            (vol1 - vol0).abs() < vol0 * 0.002,
            "volume changed: {vol0:.3} -> {vol1:.3}"
        );
        assert!(
            (area1 - area0).abs() < area0 * 0.002,
            "area changed: {area0:.3} -> {area1:.3}"
        );
    }
}

#[cfg(test)]
mod full_split_tests {
    use super::*;
    use crate::ssi;
    use vcad_kernel_math::{Transform, Vec3};
    use vcad_kernel_primitives::{make_cube, make_cylinder};
    use vcad_kernel_tessellate::tessellate_brep;

    #[test]
    fn wall_split_by_all_blade_curves_conserves_area() {
        let mut cyl_solid = make_cylinder(45.0, 13.0, 32);
        let mesh0 = tessellate_brep(&cyl_solid, 64);
        let area0: f64 = mesh0
            .indices
            .chunks(3)
            .map(|t| {
                let p = |i: u32| {
                    let b = i as usize * 3;
                    Vec3::new(
                        mesh0.vertices[b] as f64,
                        mesh0.vertices[b + 1] as f64,
                        mesh0.vertices[b + 2] as f64,
                    )
                };
                0.5 * (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0])).norm()
            })
            .sum();

        let mut blade = make_cube(23.5, 0.5, 12.57);
        let combined = Transform::translation(21.5, 0.0, 0.0)
            .then(&Transform::rotation_x(39.29_f64.to_radians()));
        for (_id, v) in &mut blade.topology.vertices {
            v.point = combined.apply_point(&v.point);
        }
        for s in &mut blade.geometry.surfaces {
            *s = s.transform(&combined);
        }
        let wall_idx = cyl_solid
            .geometry
            .surfaces
            .iter()
            .position(|s| s.as_any().downcast_ref::<CylinderSurface>().is_some())
            .unwrap();
        let wall_face = cyl_solid
            .topology
            .faces
            .iter()
            .find(|(_, f)| f.surface_index == wall_idx)
            .map(|(id, _)| id)
            .unwrap();

        let mut current = vec![wall_face];
        for surf in &blade.geometry.surfaces {
            let curve = ssi::intersect_surfaces(
                cyl_solid.geometry.surfaces[wall_idx].as_ref(),
                surf.as_ref(),
            )
            .expect("ssi");
            let ssi::IntersectionCurve::Sampled(pts) = curve else {
                continue;
            };
            let mut next = Vec::new();
            for fid in current {
                if !cyl_solid.topology.faces.contains_key(fid) {
                    continue;
                }
                match split_cylindrical_face_by_sampled(&mut cyl_solid, fid, &pts) {
                    Some(r) => next.extend(r.sub_faces),
                    None => next.push(fid),
                }
            }
            current = next;
        }
        eprintln!("wall now {} sub-faces", current.len());

        let mesh1 = tessellate_brep(&cyl_solid, 64);
        let area1: f64 = mesh1
            .indices
            .chunks(3)
            .map(|t| {
                let p = |i: u32| {
                    let b = i as usize * 3;
                    Vec3::new(
                        mesh1.vertices[b] as f64,
                        mesh1.vertices[b + 1] as f64,
                        mesh1.vertices[b + 2] as f64,
                    )
                };
                0.5 * (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0])).norm()
            })
            .sum();
        eprintln!("area {area0:.3} -> {area1:.3}");
        assert!(
            (area1 - area0).abs() < area0 * 0.005,
            "area not conserved: {area0:.3} -> {area1:.3}"
        );
    }
}

#[cfg(test)]
mod result_band_tests {
    use super::*;
    use crate::api::{boolean_op, BooleanOp, BooleanResult};
    use vcad_kernel_math::Transform;
    use vcad_kernel_primitives::{make_cube, make_cylinder};

    /// Every cylinder face surviving the near-containment intersection must
    /// be representable to the tessellator: either a parseable band or a
    /// small-angular-span sliver (fan path). A face that is neither would
    /// fall to the rectangular grid path and paint phantom geometry.
    #[test]
    fn b1_result_cylinder_faces_are_bands_or_slivers() {
        let cyl_solid = make_cylinder(45.0, 13.0, 32);
        let mut blade = make_cube(23.5, 0.5, 12.57);
        let combined = Transform::translation(21.5, 0.0, 0.0)
            .then(&Transform::rotation_x(39.29_f64.to_radians()));
        for (_id, v) in &mut blade.topology.vertices {
            v.point = combined.apply_point(&v.point);
        }
        for s in &mut blade.geometry.surfaces {
            *s = s.transform(&combined);
        }
        let BooleanResult::BRep(result) =
            boolean_op(&cyl_solid, &blade, BooleanOp::Intersection, 64).expect("boolean");
        let mut checked = 0;
        for (fid, face) in result.topology.faces.iter() {
            let Some(cyl) = result.geometry.surfaces[face.surface_index]
                .as_any()
                .downcast_ref::<CylinderSurface>()
            else {
                continue;
            };
            checked += 1;
            if let Some((_c, band)) = parse_band(&result, fid) {
                // Kept wall pieces must be sliver-scale, not tube-scale.
                let area = band_area(&band) * cyl.radius;
                assert!(
                    area < 20.0,
                    "{fid:?}: kept wall band area {area:.3} is tube-scale"
                );
            } else {
                // Not a band: must be a small-arc sliver the fan handles.
                let verts: Vec<Point3> = result
                    .topology
                    .loop_half_edges(face.outer_loop)
                    .map(|he| result.topology.vertices[result.topology.half_edges[he].origin].point)
                    .collect();
                let mut span = 0.0f64;
                for i in 0..verts.len() {
                    for j in i + 1..verts.len() {
                        let (ui, _) = uv_of(&verts[i], cyl);
                        let (uj, _) = uv_of(&verts[j], cyl);
                        let mut d = (ui - uj).rem_euclid(2.0 * PI);
                        if d > PI {
                            d = 2.0 * PI - d;
                        }
                        span = span.max(d);
                    }
                }
                assert!(
                    span <= PI / 16.0,
                    "{fid:?}: {} verts, span {span:.4} — neither band nor sliver",
                    verts.len()
                );
            }
        }
        assert!(checked >= 1, "expected kept cylinder faces in the result");
    }
}

#[cfg(test)]
mod frozen_chain_tests {
    use super::*;
    use crate::ssi;
    use vcad_kernel_math::Transform;
    use vcad_kernel_primitives::{make_cube, make_cylinder};

    #[test]
    fn frozen_wall_survives_sequential_ellipse_splits() {
        let mut cyl_solid = make_cylinder(45.0, 13.0, 32);
        crate::freeze::freeze_circle_loops(&mut cyl_solid, 64);
        let mut blade = make_cube(23.5, 0.5, 12.57);
        let combined = Transform::translation(21.5, 0.0, 0.0)
            .then(&Transform::rotation_x(39.29_f64.to_radians()));
        for (_id, v) in &mut blade.topology.vertices {
            v.point = combined.apply_point(&v.point);
        }
        for s in &mut blade.geometry.surfaces {
            *s = s.transform(&combined);
        }
        let wall_idx = cyl_solid
            .geometry
            .surfaces
            .iter()
            .position(|s| s.as_any().downcast_ref::<CylinderSurface>().is_some())
            .unwrap();
        let wall_face = cyl_solid
            .topology
            .faces
            .iter()
            .find(|(_, f)| f.surface_index == wall_idx)
            .map(|(id, _)| id)
            .unwrap();
        let mut current = vec![wall_face];
        for (si, surf) in blade.geometry.surfaces.iter().enumerate() {
            let curve = ssi::intersect_surfaces(
                cyl_solid.geometry.surfaces[wall_idx].as_ref(),
                surf.as_ref(),
            )
            .expect("ssi");
            let ssi::IntersectionCurve::Sampled(pts) = curve else {
                continue;
            };
            let mut next = Vec::new();
            for fid in current {
                if !cyl_solid.topology.faces.contains_key(fid) {
                    continue;
                }
                match split_cylindrical_face_by_sampled(&mut cyl_solid, fid, &pts) {
                    Some(r) => next.extend(r.sub_faces),
                    None => {
                        let nv = cyl_solid
                            .topology
                            .loop_len(cyl_solid.topology.faces[fid].outer_loop);
                        panic!("surface {si}: split returned None on face {fid:?} nv={nv}");
                    }
                }
            }
            current = next;
        }
        assert!(current.len() >= 4, "got {} sub-faces", current.len());
        // Area audit: the sub-bands must tile the wall exactly once.
        let mut total = 0.0;
        let wall_cyl = cyl_solid.geometry.surfaces[wall_idx]
            .as_any()
            .downcast_ref::<CylinderSurface>()
            .unwrap()
            .clone();
        for &fid in &current {
            // Shoelace area of the loop in unwrapped (u, v).
            let uvs: Vec<(f64, f64)> = cyl_solid
                .topology
                .loop_half_edges(cyl_solid.topology.faces[fid].outer_loop)
                .map(|he| {
                    let p = cyl_solid.topology.vertices
                        [cyl_solid.topology.half_edges[he].origin]
                        .point;
                    uv_of(&p, &wall_cyl)
                })
                .collect();
            let mut un: Vec<f64> = Vec::with_capacity(uvs.len());
            un.push(uvs[0].0);
            for k in 1..uvs.len() {
                let prev = *un.last().unwrap();
                let mut du = (uvs[k].0 - prev).rem_euclid(2.0 * PI);
                if du > PI {
                    du -= 2.0 * PI;
                }
                un.push(prev + du);
            }
            let mut a2 = 0.0;
            for k in 0..uvs.len() {
                let j = (k + 1) % uvs.len();
                // closing edge: use unwrapped, treating wrap as small step
                let (u1, v1) = (un[k], uvs[k].1);
                let (mut u2, v2) = (un[j], uvs[j].1);
                if j == 0 {
                    let mut du = (uvs[0].0 - un[uvs.len() - 1]).rem_euclid(2.0 * PI);
                    if du > PI {
                        du -= 2.0 * PI;
                    }
                    u2 = un[uvs.len() - 1] + du;
                }
                a2 += u1 * v2 - u2 * v1;
            }
            total += 0.5 * a2.abs();
        }
        let expect = 2.0 * PI * 13.0;
        assert!(
            (total - expect).abs() < expect * 0.002,
            "band areas sum to {total:.4}, wall is {expect:.4}"
        );

        // Now the blade end-plane line splits (TwoLines in the pipeline).
        let u_line = (21.5f64 / 45.0).acos();
        for sgn in [1.0f64, -1.0] {
            let (su, cu) = (sgn * u_line).sin_cos();
            let line = vcad_kernel_geom::Line3d {
                origin: Point3::new(45.0 * cu, 45.0 * su, 0.0),
                direction: vcad_kernel_math::Vec3::new(0.0, 0.0, 1.0),
            };
            let faces: Vec<FaceId> = cyl_solid
                .topology
                .faces
                .iter()
                .filter(|(_, f)| f.surface_index == wall_idx)
                .map(|(id, _)| id)
                .collect();
            for fid in faces {
                if !cyl_solid.topology.faces.contains_key(fid) {
                    continue;
                }
                let _ = crate::split::split_cylindrical_face(
                    &mut cyl_solid,
                    fid,
                    &ssi::IntersectionCurve::Line(line.clone()),
                    64,
                );
            }
        }
        // Mesh open-edge audit at the boolean's native resolution, after
        // the same repair pass the pipeline runs before classification.
        crate::repair::repair_topology(&mut cyl_solid.topology, 1e-6);
        let mesh = vcad_kernel_tessellate::tessellate_brep(&cyl_solid, 64);
        let quantum = 1e-5;
        let vkey = |vi: usize| -> [i64; 3] {
            let mut k = [0i64; 3];
            for c in 0..3 {
                k[c] = (mesh.vertices[vi * 3 + c] as f64 / quantum).round() as i64;
            }
            k
        };
        let mut net: std::collections::HashMap<([i64; 3], [i64; 3]), i64> =
            std::collections::HashMap::new();
        for t in 0..mesh.indices.len() / 3 {
            for k in 0..3 {
                let x = vkey(mesh.indices[t * 3 + k] as usize);
                let y = vkey(mesh.indices[t * 3 + (k + 1) % 3] as usize);
                if x == y {
                    continue;
                }
                if x < y {
                    *net.entry((x, y)).or_default() += 1;
                } else {
                    *net.entry((y, x)).or_default() -= 1;
                }
            }
        }
        let mut open: Vec<_> = net.iter().filter(|(_, &n)| n != 0).collect();
        open.sort_by_key(|((x, _), _)| *x);
        for ((x, y), _) in open.iter().take(10) {
            eprintln!(
                "open ({:.4},{:.4},{:.4})->({:.4},{:.4},{:.4})",
                x[0] as f64 * quantum,
                x[1] as f64 * quantum,
                x[2] as f64 * quantum,
                y[0] as f64 * quantum,
                y[1] as f64 * quantum,
                y[2] as f64 * quantum
            );
        }
        // Dump unpaired topology edges with owning faces.
        let mut unpaired = 0;
        for (he_id, he) in &cyl_solid.topology.half_edges {
            if he.loop_id.is_none() || he.twin.is_some() {
                continue;
            }
            unpaired += 1;
            if unpaired <= 14 {
                let a = cyl_solid.topology.vertices[he.origin].point;
                let b = cyl_solid.topology.vertices
                    [cyl_solid.topology.half_edge_dest(he_id)]
                    .point;
                let f = he
                    .loop_id
                    .and_then(|l| cyl_solid.topology.loops[l].face);
                let (ua, va) = uv_of(&a, &wall_cyl);
                let (ub, vb) = uv_of(&b, &wall_cyl);
                eprintln!(
                    "UNPAIRED {f:?} uv ({ua:.4},{va:.4})->({ub:.4},{vb:.4})"
                );
            }
        }
        eprintln!("total unpaired: {unpaired}");
        // Dump loops of faces near the tangency point (u=π, v≈0).
        for (fid, face) in cyl_solid.topology.faces.iter() {
            if face.surface_index != wall_idx {
                continue;
            }
            let uvs: Vec<(f64, f64)> = cyl_solid
                .topology
                .loop_half_edges(face.outer_loop)
                .map(|he| {
                    let p = cyl_solid.topology.vertices
                        [cyl_solid.topology.half_edges[he].origin]
                        .point;
                    uv_of(&p, &wall_cyl)
                })
                .collect();
            if uvs
                .iter()
                .any(|&(u, v)| (u - PI).abs() < 0.02 && v.abs() < 0.02)
            {
                let disp: Vec<(f64, f64)> = uvs
                    .iter()
                    .map(|&(u, v)| ((u * 1e4).round() / 1e4, (v * 1e4).round() / 1e4))
                    .collect();
                eprintln!("face {fid:?} nv={} loop: {disp:?}", uvs.len());
            }
        }
        // The caps are deliberately NOT split in this probe, so wall-rim
        // crossing vertices T-junction against the untouched cap rings
        // (the real pipeline splits the caps by the same planes, producing
        // analytically identical points). Only wall-wall conformity is
        // asserted here, via a loose ceiling on the census.
        assert!(
            open.len() < 120,
            "{} open edges after line splits",
            open.len()
        );
    }
}
