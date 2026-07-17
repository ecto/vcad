//! Fail-closed error type for the antenna solver.
//!
//! Every physical-validity gate in this crate **errors instead of silently
//! degrading**: a mesh that violates the thin-wire approximation, a segment
//! too coarse for the wavelength, or a junction the current milestone does
//! not model refuses to produce numbers. The paid-for lesson from the
//! particle-optics crate: a solver that quietly leaves its regime of
//! validity produces confident nonsense, and nobody catches it downstream.

/// Errors from mesh construction, validity gates, and the solve.
#[derive(Debug, Clone, PartialEq)]
pub enum AntennaError {
    /// Wire radius must be positive and finite.
    InvalidRadius {
        /// Offending radius, mm.
        radius_mm: f64,
    },
    /// A wire or path leg must be divided into at least one segment.
    InvalidSegmentCount,
    /// A wire or path leg has (numerically) zero length.
    DegenerateWire {
        /// Leg length, mm.
        length_mm: f64,
    },
    /// A path needs at least two points; a loop at least three.
    PathTooShort,
    /// `segments_per_leg` length does not match the number of legs.
    LegCountMismatch {
        /// Number of legs in the path.
        legs: usize,
        /// Number of per-leg segment counts supplied.
        counts: usize,
    },
    /// With a ground plane enabled, all geometry must satisfy z ≥ 0.
    BelowGroundPlane {
        /// Node index.
        node: usize,
        /// Offending height, mm.
        z_mm: f64,
    },
    /// A grounded node must be a plain wire endpoint (degree 1): interior
    /// nodes or junctions touching the plane are not modeled (fail-closed).
    GroundContactUnsupported {
        /// Node index.
        node: usize,
        /// Number of segments meeting there.
        degree: usize,
    },
    /// A segment lies in the ground plane (both endpoints at z = 0) — it
    /// would be shorted by its own image.
    SegmentOnGroundPlane {
        /// Segment index.
        segment: usize,
    },
    /// The mesh has no interior nodes, hence no current unknowns.
    NoBases,
    /// The requested feed basis does not exist.
    FeedOutOfRange {
        /// Requested basis index.
        feed: usize,
        /// Number of bases in the mesh.
        bases: usize,
    },
    /// Frequency must be positive and finite.
    InvalidFrequency {
        /// Offending frequency, Hz.
        freq_hz: f64,
    },
    /// Thin-wire kernel validity: segment length must be ≥ 4 × wire radius.
    ///
    /// Below this the reduced kernel (source current on the axis, matching
    /// on the surface) misrepresents the self term; NEC-2's manual asks for
    /// Δ/a > 8 with its standard kernel. This crate hard-fails at 4 rather
    /// than degrade silently — refine the wire into fewer, longer segments
    /// or use a thinner wire.
    ThinWireViolation {
        /// Segment index.
        segment: usize,
        /// Segment length, mm.
        length_mm: f64,
        /// Wire radius, mm.
        radius_mm: f64,
    },
    /// Sampling validity: segment length must be ≤ λ/8 (λ/20 recommended).
    SegmentTooLong {
        /// Segment index.
        segment: usize,
        /// Segment length, mm.
        length_mm: f64,
        /// λ/8 at the requested frequency, mm.
        max_mm: f64,
    },
    /// Thin-wire validity: `k·a` must be ≤ 0.1 (radius ≪ wavelength).
    RadiusTooThick {
        /// Wire radius, mm.
        radius_mm: f64,
        /// Maximum radius at the requested frequency, mm.
        max_mm: f64,
    },
    /// The impedance matrix is numerically singular.
    SingularSystem,
    /// A named spec parameter had no binding at resolve time.
    UnboundParameter {
        /// The parameter name.
        name: String,
    },
    /// A parameter velocity has the wrong number of node entries.
    ParamVelocityMismatch {
        /// Nodes in the mesh.
        nodes: usize,
        /// Velocity entries supplied.
        velocities: usize,
    },
    /// A parameter would move a node off the ground plane, changing the
    /// image structure discontinuously.
    GroundedNodeMoved {
        /// Node index.
        node: usize,
    },
    /// Resonance search: `Im(Z)` does not change sign over the bracket.
    ResonanceNotBracketed {
        /// Reactance at the lower frequency, Ω.
        x_lo: f64,
        /// Reactance at the upper frequency, Ω.
        x_hi: f64,
    },
}

impl std::fmt::Display for AntennaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AntennaError::InvalidRadius { radius_mm } => {
                write!(
                    f,
                    "wire radius must be positive and finite, got {radius_mm} mm"
                )
            }
            AntennaError::InvalidSegmentCount => {
                write!(f, "wires must be divided into at least one segment")
            }
            AntennaError::DegenerateWire { length_mm } => {
                write!(f, "wire leg has zero length ({length_mm} mm)")
            }
            AntennaError::PathTooShort => {
                write!(f, "a path needs at least two points (a loop, three)")
            }
            AntennaError::LegCountMismatch { legs, counts } => {
                write!(
                    f,
                    "path has {legs} legs but {counts} per-leg segment counts were supplied"
                )
            }
            AntennaError::BelowGroundPlane { node, z_mm } => {
                write!(
                    f,
                    "node {node} sits at z = {z_mm:.4} mm below the z = 0 ground plane; \
                     all geometry must satisfy z ≥ 0 when the plane is enabled"
                )
            }
            AntennaError::GroundContactUnsupported { node, degree } => {
                write!(
                    f,
                    "grounded node {node} joins {degree} segments; only a plain wire \
                     endpoint (degree 1) may touch the ground plane (fail-closed)"
                )
            }
            AntennaError::SegmentOnGroundPlane { segment } => {
                write!(
                    f,
                    "segment {segment} lies in the z = 0 ground plane and would be \
                     shorted by its own image; raise it or remove it"
                )
            }
            AntennaError::NoBases => {
                write!(
                    f,
                    "mesh has no interior nodes (no current unknowns); subdivide wires \
                     into at least two segments"
                )
            }
            AntennaError::FeedOutOfRange { feed, bases } => {
                write!(f, "feed basis {feed} out of range ({bases} bases in mesh)")
            }
            AntennaError::InvalidFrequency { freq_hz } => {
                write!(f, "frequency must be positive and finite, got {freq_hz} Hz")
            }
            AntennaError::ThinWireViolation {
                segment,
                length_mm,
                radius_mm,
            } => {
                write!(
                    f,
                    "segment {segment} is {length_mm:.4} mm long with wire radius \
                     {radius_mm:.4} mm; the thin-wire kernel requires length ≥ 4×radius \
                     ({:.4} mm) — use fewer segments or a thinner wire (fail-closed)",
                    4.0 * radius_mm
                )
            }
            AntennaError::SegmentTooLong {
                segment,
                length_mm,
                max_mm,
            } => {
                write!(
                    f,
                    "segment {segment} is {length_mm:.3} mm long but λ/8 at this frequency \
                     is {max_mm:.3} mm; refine the mesh (λ/20 segments recommended)"
                )
            }
            AntennaError::RadiusTooThick { radius_mm, max_mm } => {
                write!(
                    f,
                    "wire radius {radius_mm:.4} mm violates the thin-wire limit k·a ≤ 0.1 \
                     (max {max_mm:.4} mm at this frequency)"
                )
            }
            AntennaError::SingularSystem => {
                write!(f, "impedance matrix is numerically singular")
            }
            AntennaError::UnboundParameter { name } => {
                write!(f, "unbound antenna parameter: {name:?}")
            }
            AntennaError::ParamVelocityMismatch { nodes, velocities } => {
                write!(
                    f,
                    "parameter velocity has {velocities} entries for a mesh with {nodes} nodes"
                )
            }
            AntennaError::GroundedNodeMoved { node } => {
                write!(
                    f,
                    "parameter would move grounded node {node} off the z = 0 plane; \
                     ground-contact geometry must keep v_z = 0 there"
                )
            }
            AntennaError::ResonanceNotBracketed { x_lo, x_hi } => {
                write!(
                    f,
                    "Im(Z) does not change sign over the bracket: X_lo = {x_lo:.3} Ω, \
                     X_hi = {x_hi:.3} Ω"
                )
            }
        }
    }
}

impl std::error::Error for AntennaError {}
