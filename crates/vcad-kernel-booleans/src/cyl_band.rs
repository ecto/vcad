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
//! Band faces are materialized as loops of two angle-paired vertex chains
//! (bottom chain u-ascending, then top chain u-descending), the exact shape
//! `tessellate_ruled_two_chain` in vcad-kernel-tessellate renders verbatim,
//! so slanted/wavy boundaries survive to the mesh.
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

/// A cylindrical face region `lo(u) ≤ v ≤ hi(u)` over `us` (ascending,
/// possibly unwrapped past 2π). For a `full_wrap` band the first and last
/// columns sit at the same angle (u and u + 2π) and the region closes on
/// itself around the axis.
#[derive(Debug, Clone)]
pub(crate) struct CylBand {
    /// Ascending u samples; `us.len() == lo.len() == hi.len() >= 2`.
    pub us: Vec<f64>,
    /// Lower v chain, sampled at `us`.
    pub lo: Vec<f64>,
    /// Upper v chain, sampled at `us`.
    pub hi: Vec<f64>,
    /// Whether the band wraps the full circumference (last u = first u + 2π).
    pub full_wrap: bool,
}

/// A closed single-valued curve on a cylinder: v = f(u), periodic in 2π.
/// `us` ascend over one period starting at `us[0]`; evaluation wraps.
#[derive(Debug, Clone)]
pub(crate) struct CylProfile {
    us: Vec<f64>,
    vs: Vec<f64>,
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
}

impl CylBand {
    /// Linear interpolation of a chain at u (no wrap: u must be in range).
    fn interp(us: &[f64], vs: &[f64], u: f64) -> f64 {
        let n = us.len();
        if u <= us[0] {
            return vs[0];
        }
        if u >= us[n - 1] {
            return vs[n - 1];
        }
        let i = us.partition_point(|a| *a < u);
        let (ua, ub) = (us[i - 1], us[i]);
        let t = if (ub - ua).abs() < U_EPS {
            0.0
        } else {
            (u - ua) / (ub - ua)
        };
        vs[i - 1] + t * (vs[i] - vs[i - 1])
    }

    /// Lower chain value at u.
    pub(crate) fn lo_at(&self, u: f64) -> f64 {
        Self::interp(&self.us, &self.lo, u)
    }

    /// Upper chain value at u.
    pub(crate) fn hi_at(&self, u: f64) -> f64 {
        Self::interp(&self.us, &self.hi, u)
    }
}

/// Parse a cylindrical face into band form. Handles three loop shapes:
/// degenerate seam loops (full tube), 4-vertex rectangles, and the dense
/// angle-paired two-chain loops this module itself emits. Returns `None`
/// for anything else.
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
                us: vec![u0, u0 + 2.0 * PI],
                lo: vec![v_min, v_min],
                hi: vec![v_max, v_max],
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
    let mut unwrapped: Vec<f64> = Vec::with_capacity(n);
    unwrapped.push(uvs[0].0);
    for uv in uvs.iter().skip(1) {
        let prev = *unwrapped.last().unwrap();
        let mut du = (uv.0 - prev).rem_euclid(2.0 * PI);
        if du > PI {
            du -= 2.0 * PI;
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
    let mut chain_a: Vec<(f64, f64)> = (0..split_at).map(|i| (unwrapped[i], uvs[i].1)).collect();
    let mut chain_b: Vec<(f64, f64)> = (split_at..n).map(|i| (unwrapped[i], uvs[i].1)).collect();
    if chain_a.len() < 2 || chain_b.len() < 2 {
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
        return None;
    }
    let max_step = span / (chain_a.len().min(chain_b.len()) as f64 - 1.0).max(1.0);
    let range_tol = (1.5 * max_step).max(1e-6);
    if (a0 - b0).abs() > range_tol || (a1 - b1).abs() > range_tol {
        return None;
    }
    let full_wrap = (span - 2.0 * PI).abs() < 1e-6;

    // Build the band on the union grid, interpolating each chain (clamped
    // at pinch-shortened ends).
    let mut us: Vec<f64> = chain_a.iter().map(|p| p.0).collect();
    us.extend(chain_b.iter().map(|p| p.0));
    us.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    us.dedup_by(|x, y| (*x - *y).abs() < U_EPS);
    let interp = |chain: &[(f64, f64)], u: f64| -> f64 {
        if u <= chain[0].0 {
            return chain[0].1;
        }
        let last = chain.len() - 1;
        if u >= chain[last].0 {
            return chain[last].1;
        }
        let i = chain.partition_point(|p| p.0 < u);
        let (ua, va) = chain[i - 1];
        let (ub, vb) = chain[i];
        if (ub - ua).abs() < U_EPS {
            va
        } else {
            va + (u - ua) / (ub - ua) * (vb - va)
        }
    };
    let a_vals: Vec<f64> = us.iter().map(|&u| interp(&chain_a, u)).collect();
    let b_vals: Vec<f64> = us.iter().map(|&u| interp(&chain_b, u)).collect();

    // Chains must not cross (a band has a well-defined lower/upper chain).
    let a_below = a_vals.iter().zip(&b_vals).all(|(x, y)| *x <= *y + V_EPS);
    let b_below = b_vals.iter().zip(&a_vals).all(|(x, y)| *x <= *y + V_EPS);
    let (lo, hi) = if a_below {
        (a_vals, b_vals)
    } else if b_below {
        (b_vals, a_vals)
    } else {
        return None;
    };
    Some((
        cyl,
        CylBand {
            us,
            lo,
            hi,
            full_wrap,
        },
    ))
}

/// Are two angles equal on the circle (mod 2π)?
fn angle_close(a: f64, b: f64, tol: f64) -> bool {
    let mut d = (a - b).rem_euclid(2.0 * PI);
    if d > PI {
        d = 2.0 * PI - d;
    }
    d.abs() < tol
}

/// Split `band` by the closed profile `f`, producing the sub-bands below f
/// (`lo ≤ v ≤ min(f, hi)`) and above f (`max(f, lo) ≤ v ≤ hi`). Returns
/// `None` when the profile misses the band interior (no split needed).
pub(crate) fn split_band_by_profile(
    band: &CylBand,
    f: &CylProfile,
) -> Option<(Vec<CylBand>, Vec<CylBand>)> {
    // Common refined grid: band chain samples ∪ profile samples mapped into
    // the band's u-interval ∪ chain/profile crossing points.
    let (u_start, u_end) = (band.us[0], *band.us.last().unwrap());
    let mut grid: Vec<f64> = band.us.clone();
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
        for chain in [true, false] {
            let ga = if chain {
                f.eval(ua) - band.lo_at(ua)
            } else {
                band.hi_at(ua) - f.eval(ua)
            };
            let gb = if chain {
                f.eval(ub) - band.lo_at(ub)
            } else {
                band.hi_at(ub) - f.eval(ub)
            };
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

    let lo: Vec<f64> = refined.iter().map(|&u| band.lo_at(u)).collect();
    let hi: Vec<f64> = refined.iter().map(|&u| band.hi_at(u)).collect();
    let fv: Vec<f64> = refined.iter().map(|&u| f.eval(u)).collect();

    let below_hi: Vec<f64> = fv.iter().zip(&hi).map(|(a, b)| a.min(*b)).collect();
    let above_lo: Vec<f64> = fv.iter().zip(&lo).map(|(a, b)| a.max(*b)).collect();

    let below = extract_regions(&refined, &lo, &below_hi, band.full_wrap);
    let above = extract_regions(&refined, &above_lo, &hi, band.full_wrap);

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

/// Approximate (u, v) parameter area of a band (trapezoid rule).
fn band_area(b: &CylBand) -> f64 {
    let mut a = 0.0;
    for i in 0..b.us.len() - 1 {
        let w0 = (b.hi[i] - b.lo[i]).max(0.0);
        let w1 = (b.hi[i + 1] - b.lo[i + 1]).max(0.0);
        a += 0.5 * (w0 + w1) * (b.us[i + 1] - b.us[i]);
    }
    a
}

/// Extract maximal positive-width sub-bands of the region `lo(u) ≤ v ≤ hi(u)`
/// over the refined grid. Pinch points (width 0) bound the pieces.
fn extract_regions(us: &[f64], lo: &[f64], hi: &[f64], parent_wrap: bool) -> Vec<CylBand> {
    let n = us.len();
    let width = |i: usize| hi[i] - lo[i];
    let positive: Vec<bool> = (0..n).map(|i| width(i) > V_EPS).collect();

    if positive.iter().all(|&p| p) {
        // Region spans the whole interval — a single band, preserving wrap.
        return vec![CylBand {
            us: us.to_vec(),
            lo: lo.to_vec(),
            hi: hi.to_vec(),
            full_wrap: parent_wrap,
        }];
    }
    if positive.iter().all(|&p| !p) {
        return Vec::new();
    }

    // For wrapping parents, rotate the grid so it starts at a pinched
    // (zero-width) column; then no positive run straddles the seam. The
    // duplicated final column (same angle as the first) is dropped before
    // rotating and the u coordinates are re-unwrapped from the new start.
    let (us_r, lo_r, hi_r): (Vec<f64>, Vec<f64>, Vec<f64>) = if parent_wrap && positive[0] {
        let m = n - 1; // drop duplicated final column
        let start = (0..m).find(|&i| width(i) <= V_EPS).unwrap_or(0);
        let mut u2 = Vec::with_capacity(m + 1);
        let mut l2 = Vec::with_capacity(m + 1);
        let mut h2 = Vec::with_capacity(m + 1);
        for k in 0..=m {
            let idx = (start + k) % m;
            let turns = ((start + k) / m) as f64;
            u2.push(us[idx] + turns * 2.0 * PI);
            l2.push(lo[idx]);
            h2.push(hi[idx]);
        }
        (u2, l2, h2)
    } else {
        (us.to_vec(), lo.to_vec(), hi.to_vec())
    };

    let n = us_r.len();
    let width = |i: usize| hi_r[i] - lo_r[i];
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        if width(i) <= V_EPS {
            i += 1;
            continue;
        }
        // Positive run [i, j); include bounding pinch columns when present.
        let mut j = i;
        while j < n && width(j) > V_EPS {
            j += 1;
        }
        let s = i.saturating_sub(1);
        let e = (j).min(n - 1);
        if e > s {
            out.push(CylBand {
                us: us_r[s..=e].to_vec(),
                lo: lo_r[s..=e].to_vec(),
                hi: hi_r[s..=e].to_vec(),
                full_wrap: false,
            });
        }
        i = j;
    }
    out
}

/// Split a band at a constant angle (a vertical intersection line at
/// `u_split`). Returns the two halves, or `None` when the line misses the
/// band interior.
pub(crate) fn split_band_at_u(band: &CylBand, u_split: f64) -> Option<(CylBand, CylBand)> {
    let (u_start, u_end) = (band.us[0], *band.us.last().unwrap());
    // Map u_split into the band's interval.
    let x = u_start + (u_split - u_start).rem_euclid(2.0 * PI);
    if x <= u_start + 1e-6 || x >= u_end - 1e-6 {
        return None;
    }
    let cut = |a: f64, b: f64| -> CylBand {
        let mut us = vec![a];
        let mut lo = vec![band.lo_at(a)];
        let mut hi = vec![band.hi_at(a)];
        for (k, &u) in band.us.iter().enumerate() {
            if u > a + U_EPS && u < b - U_EPS {
                us.push(u);
                lo.push(band.lo[k]);
                hi.push(band.hi[k]);
            }
        }
        us.push(b);
        lo.push(band.lo_at(b));
        hi.push(band.hi_at(b));
        CylBand {
            us,
            lo,
            hi,
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
        // Emit BOTH chains at every column, INCLUDING pinch columns where
        // the two chains meet (duplicate positions). The strict angle
        // pairing (vert i ↔ vert 2m−1−i) is what `parse_band` and the
        // ruled two-chain tessellation key on; deduplicating a pinch
        // column would shift the pairing and demote the face to the
        // rectangular-grid tessellation path, which overshoots wavy
        // boundaries.
        let m = band.us.len();
        let mut loop_pts: Vec<Point3> = Vec::with_capacity(2 * m);
        // Bottom chain, ascending u.
        for i in 0..m {
            loop_pts.push(point_at(cyl, band.us[i], band.lo[i]));
        }
        // Top chain, descending u.
        for i in (0..m).rev() {
            loop_pts.push(point_at(cyl, band.us[i], band.hi[i]));
        }
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
) -> Option<SplitResult> {
    let (cyl, band) = parse_band(brep, face_id)?;
    if require_wavy && band_is_rectangular(&band) {
        return None;
    }
    let v_c = (circle.center - cyl.center).dot(cyl.axis.as_ref());
    let profile = CylProfile {
        us: vec![band.us[0], band.us[0] + PI],
        vs: vec![v_c, v_c],
    };
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
    let flat = |vs: &[f64]| {
        let (mn, mx) = vs
            .iter()
            .fold((f64::MAX, f64::MIN), |(a, b), &v| (a.min(v), b.max(v)));
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
    let m = band.us.len();
    // Widest column-interval by mean width.
    let mut best = 0usize;
    let mut best_w = f64::MIN;
    for i in 0..m - 1 {
        let w = 0.5 * ((band.hi[i] - band.lo[i]) + (band.hi[i + 1] - band.lo[i + 1]));
        if w > best_w {
            best_w = w;
            best = i;
        }
    }
    if best_w <= V_EPS {
        return None;
    }
    let u = 0.5 * (band.us[best] + band.us[best + 1]);
    let v = 0.25 * (band.lo[best] + band.hi[best] + band.lo[best + 1] + band.hi[best + 1]);
    Some(point_at(&cyl, u, v))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_band(v0: f64, v1: f64) -> CylBand {
        CylBand {
            us: vec![0.0, 2.0 * PI],
            lo: vec![v0, v0],
            hi: vec![v1, v1],
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
                            "band: us={:?} lo={:?} hi={:?} wrap={}",
                            band.us, band.lo, band.hi, band.full_wrap
                        );
                        assert!(band.full_wrap);
                        assert!((band.hi[0] - band.lo[0] - 13.0).abs() < 1e-9);
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
            us: vec![0.0, 2.0 * PI],
            lo: vec![0.0, 0.0],
            hi: vec![13.0, 13.0],
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
