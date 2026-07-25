//! Fields of straight current-carrying segments, in closed form.
//!
//! Everything in this crate reduces to these two integrals. An air-core machine
//! has no iron, so the permeability is μ₀ everywhere, the problem is linear, and
//! superposition holds exactly — there is no grid, no mesh, and no iteration.
//! The whole solver is a sum over segments.

use crate::vec3::Vec3;
use crate::MU_0;

use std::f64::consts::PI;

/// A straight current-carrying segment, SI units (metres, amperes).
///
/// `wire_radius_m` regularizes the field on the segment axis, where the
/// filamentary formulae diverge. See [`Segment::b_at`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    /// Start point, m.
    pub a: Vec3,
    /// End point, m. Current flows a → b.
    pub b: Vec3,
    /// Current, A (signed; reverse it by swapping `a`/`b` or negating).
    pub current_a: f64,
    /// Conductor radius, m, used to regularize on-axis singularities.
    pub wire_radius_m: f64,
}

/// Geometry shared by the `B` and `A` evaluations: the segment's unit tangent,
/// the endpoint vectors, and the perpendicular distance to the field point.
struct SegmentGeometry {
    /// Unit tangent along the current direction.
    tangent: Vec3,
    /// Field point relative to `a`. (`r₂` is only ever needed through its norm
    /// and tangential component, both carried below.)
    r1: Vec3,
    /// Norms of `r1`, `r2`.
    n1: f64,
    n2: f64,
    /// Tangential components `t̂·r`.
    t1: f64,
    t2: f64,
    /// Perpendicular distance from the field point to the segment's line, m,
    /// **before** any regularization clamp.
    perp: f64,
}

impl Segment {
    fn geometry(&self, p: Vec3) -> Option<SegmentGeometry> {
        let l = self.b - self.a;
        let len = l.norm();
        if len <= 0.0 || self.current_a == 0.0 {
            return None;
        }
        let tangent = l * (1.0 / len);
        let r1 = p - self.a;
        let r2 = p - self.b;
        let t1 = tangent.dot(r1);
        let t2 = tangent.dot(r2);
        // |r|² = perp² + t², evaluated from whichever endpoint is better
        // conditioned (the one whose tangential offset is smaller).
        let n1 = r1.norm();
        let n2 = r2.norm();
        let perp = (r1 - tangent * t1).norm();
        Some(SegmentGeometry { tangent, r1, n1, n2, t1, t2, perp })
    }

    /// Magnetic flux density at `p`, tesla.
    ///
    /// For a segment from **a** to **b** carrying current `I`, with `t̂` the unit
    /// tangent, `r₁ = p − a`, `r₂ = p − b`, and `d` the perpendicular distance
    /// from `p` to the segment's line:
    ///
    /// ```text
    /// B = μ₀I/(4πd) · (cos θ₁ − cos θ₂) · (t̂ × r̂₁)
    /// ```
    ///
    /// where `cos θᵢ = t̂·rᵢ/|rᵢ|`. Taking the segment to infinity in both
    /// directions gives `μ₀I/(2πd)`, the infinite-wire result.
    ///
    /// **Regularization.** Inside the conductor (`d < wire_radius_m`) the
    /// filamentary field diverges. We evaluate at the conductor surface and
    /// scale linearly to zero on the axis — the field of a uniform current
    /// density, and the same model `vcad_kernel_particle::field::b_ring` uses.
    pub fn b_at(&self, p: Vec3) -> Vec3 {
        let Some(g) = self.geometry(p) else {
            return Vec3::ZERO;
        };
        // Direction: t̂ × r₁, which is perpendicular to both the current and the
        // offset, i.e. azimuthal about the wire by the right-hand rule.
        let dir = g.tangent.cross(g.r1);
        let dir_n = dir.norm();
        if dir_n <= 0.0 {
            // Field point lies exactly on the segment's line: B is zero there by
            // symmetry for a uniform-current conductor.
            return Vec3::ZERO;
        }
        let cos1 = if g.n1 > 0.0 { g.t1 / g.n1 } else { 0.0 };
        let cos2 = if g.n2 > 0.0 { g.t2 / g.n2 } else { 0.0 };

        let (d_eff, scale) = if g.perp < self.wire_radius_m && self.wire_radius_m > 0.0 {
            (self.wire_radius_m, g.perp / self.wire_radius_m)
        } else {
            (g.perp, 1.0)
        };
        if d_eff <= 0.0 {
            return Vec3::ZERO;
        }
        let mag = MU_0 * self.current_a / (4.0 * PI * d_eff) * (cos1 - cos2) * scale;
        dir * (mag / dir_n)
    }

    /// Magnetic vector potential at `p`, tesla-metres (Coulomb gauge).
    ///
    /// Integrating `dA = μ₀I/(4π) · dl/|p − l|` along the segment gives
    ///
    /// ```text
    /// A = μ₀I/(4π) · t̂ · ln[ (|r₁| + t̂·r₁) / (|r₂| + t̂·r₂) ]
    /// ```
    ///
    /// Flux linkage is computed from `A` rather than by integrating `B` over a
    /// surface: `λ = ∮A·dl` needs only the conductor path, so there is no
    /// spanning surface to construct for a multi-turn spiral — which is the
    /// whole reason this crate can stay grid-free.
    ///
    /// **Conditioning.** `|r| + t̂·r` cancels catastrophically when `t̂·r` is
    /// negative and large. Since `(|r| + t)(|r| − t) = d²`, we evaluate the
    /// unstable branch as `d²/(|r| − t)` instead, which is exact and stable.
    ///
    /// **Regularization.** The logarithm diverges on the axis; `d` is clamped to
    /// `wire_radius_m`. Self-inductance from a filament model is genuinely
    /// sensitive to that clamp — see the crate-level note on `L`.
    pub fn a_at(&self, p: Vec3) -> Vec3 {
        let Some(g) = self.geometry(p) else {
            return Vec3::ZERO;
        };
        let d = g.perp.max(self.wire_radius_m);
        if d <= 0.0 {
            return Vec3::ZERO;
        }
        let d2 = d * d;
        // Stable evaluation of |r| + t for either sign of t.
        let safe = |n: f64, t: f64| -> f64 {
            if t >= 0.0 {
                n + t
            } else {
                d2 / (n - t)
            }
        };
        // Recompute norms against the clamped perpendicular distance so the two
        // endpoints stay consistent with `d` when the point is inside the wire.
        let n1 = (d2 + g.t1 * g.t1).sqrt();
        let n2 = (d2 + g.t2 * g.t2).sqrt();
        let num = safe(n1, g.t1);
        let den = safe(n2, g.t2);
        if num <= 0.0 || den <= 0.0 {
            return Vec3::ZERO;
        }
        g.tangent * (MU_0 * self.current_a / (4.0 * PI) * (num / den).ln())
    }
}

/// A current path: a polyline carrying one current, discretized into segments.
///
/// A spiral PCB coil, a magnet's bound-current sheet and a phase winding are all
/// represented this way, so one integrator serves the whole solver.
#[derive(Debug, Clone, PartialEq)]
pub struct Filament {
    /// Ordered path points, m. Current flows from `points[0]` toward the end.
    pub points: Vec<Vec3>,
    /// Current, A. For an `N`-turn conductor modelled as one path, this is the
    /// ampere-turns.
    pub current_a: f64,
    /// Conductor radius, m, for on-axis regularization.
    pub wire_radius_m: f64,
    /// Whether the path closes back on `points[0]`.
    pub closed: bool,
}

impl Filament {
    /// A closed loop through `points`.
    pub fn closed_loop(points: Vec<Vec3>, current_a: f64, wire_radius_m: f64) -> Self {
        Self { points, current_a, wire_radius_m, closed: true }
    }

    /// An open path through `points`.
    pub fn open_path(points: Vec<Vec3>, current_a: f64, wire_radius_m: f64) -> Self {
        Self { points, current_a, wire_radius_m, closed: false }
    }

    /// Iterate the path's segments.
    pub fn segments(&self) -> impl Iterator<Item = Segment> + '_ {
        let n = self.points.len();
        let last = if self.closed { n } else { n.saturating_sub(1) };
        (0..last).map(move |i| Segment {
            a: self.points[i],
            b: self.points[(i + 1) % n],
            current_a: self.current_a,
            wire_radius_m: self.wire_radius_m,
        })
    }

    /// Total path length, m.
    pub fn length_m(&self) -> f64 {
        self.segments().map(|s| (s.b - s.a).norm()).sum()
    }

    /// Magnetic dipole moment, A·m²: `m = (I/2)∮ r × dl`.
    ///
    /// Only meaningful for a closed path. Used to check that a source set is
    /// magnetically balanced before expanding it in an [`crate::IronStack`] —
    /// see that type's convergence precondition.
    pub fn dipole_moment(&self) -> Vec3 {
        let s: Vec3 = self
            .segments()
            .map(|s| {
                let mid = (s.a + s.b) * 0.5;
                mid.cross(s.b - s.a)
            })
            .sum();
        s * (self.current_a * 0.5)
    }

    /// Flux density at `p` from this path, tesla.
    pub fn b_at(&self, p: Vec3) -> Vec3 {
        self.segments().map(|s| s.b_at(p)).sum()
    }

    /// Vector potential at `p` from this path, T·m.
    pub fn a_at(&self, p: Vec3) -> Vec3 {
        self.segments().map(|s| s.a_at(p)).sum()
    }

    /// Rotate the whole path about the z axis.
    pub fn rotated_z(&self, angle: f64) -> Filament {
        Filament {
            points: self.points.iter().map(|p| p.rotated_z(angle)).collect(),
            ..self.clone()
        }
    }

    /// Flux linked by this path from an external field source, weber.
    ///
    /// `λ = ∮A·dl`, evaluated with the midpoint rule on each segment. The
    /// external `a_field` must exclude this path's own contribution, or the
    /// result is self-inductance rather than mutual flux linkage.
    pub fn flux_linkage<F: Fn(Vec3) -> Vec3>(&self, a_field: F) -> f64 {
        self.segments()
            .map(|s| {
                let mid = (s.a + s.b) * 0.5;
                let dl = s.b - s.a;
                a_field(mid).dot(dl)
            })
            .sum()
    }

    /// Net force on this path in an external field, newtons: `F = ∮ I dl × B`.
    pub fn lorentz_force<F: Fn(Vec3) -> Vec3>(&self, b_field: F) -> Vec3 {
        self.segments()
            .map(|s| {
                let mid = (s.a + s.b) * 0.5;
                let dl = s.b - s.a;
                dl.cross(b_field(mid)) * self.current_a
            })
            .sum()
    }

    /// Torque about the origin's z axis in an external field, N·m:
    /// `T_z = ẑ · ∮ r × (I dl × B)`.
    pub fn lorentz_torque_z<F: Fn(Vec3) -> Vec3>(&self, b_field: F) -> f64 {
        self.segments()
            .map(|s| {
                let mid = (s.a + s.b) * 0.5;
                let dl = s.b - s.a;
                let df = dl.cross(b_field(mid)) * self.current_a;
                mid.cross(df).z
            })
            .sum()
    }
}
