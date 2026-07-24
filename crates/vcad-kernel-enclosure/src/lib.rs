#![warn(missing_docs)]

//! Enclosure feature extraction and cross-domain PCB ↔ enclosure verification.
//!
//! vcad is the only stack with both a real BRep CAD kernel and a PCB engine, so
//! it can cross-check a board against the physical case it lives in. This crate
//! is that verification core, ported from the original pure-TypeScript
//! implementation in `@vcad/engine` (`enclosure-mesh.ts` / `enclosure-fit.ts`)
//! and kept wire-compatible with it: all serde field names match the TS
//! interfaces, so the WASM bindings round-trip the same JSON shapes the MCP
//! `check_enclosure_fit` tool already speaks.
//!
//! Two halves:
//!
//! * [`extract_enclosure_features`] — solid triangle mesh in, axis-aligned
//!   cavity + standoffs + wall openings out. Inside/outside is decided with the
//!   **generalized winding number** (Van Oosterom–Strackee signed solid angle
//!   per triangle), sampled on a coarse voxel grid — robust to the small holes,
//!   coincident faces, and stray internal faces real kernel CSG meshes contain.
//! * [`check_enclosure_fit`] — board outline + features vs the extracted cavity:
//!   wall clearance, lid/stack height, mounting-hole ↔ standoff alignment, and
//!   edge-connector ↔ wall-cutout alignment. Plus [`derive_board_from_cavity`],
//!   the mirror operation that seeds a board from a case.
//!
//! Everything is pure (numbers in, verdict out); units are millimeters, Z-up.

mod fit;
mod mesh;
mod pcb;

pub use fit::{
    check_enclosure_fit, derive_board_from_cavity, outline_aabb, to_world, Aabb2, BoardOutline,
    BoardPlacement, CheckStatus, ComponentExtent, ConnectorRef, DeriveBoardOptions, DerivedBoard,
    EnclosureCavity, EnclosureFitCheck, EnclosureFitInput, EnclosureFitReport, MeasurementValue,
    Measurements, MountingHole, Standoff, Vec2, Vec3, WallEdge, WallOpening,
};
pub use mesh::{extract_enclosure_features, EnclosureFeatures, OuterBounds};
pub use pcb::{
    component_extents_from_meshes, connectors_from_pcb, mounting_holes_from_pcb, ComponentMeshRef,
    PcbLite,
};

/// Round to 2 decimals with JavaScript `Math.round` semantics (half toward
/// +∞), so ported behavior — including report strings — matches the original
/// TS implementation bit-for-bit on shared fixtures.
pub(crate) fn round2(n: f64) -> f64 {
    if !n.is_finite() {
        return n;
    }
    (n * 100.0 + 0.5).floor() / 100.0
}

/// Format a number the way JS string interpolation does (`3` not `3.0`,
/// `Infinity` not `inf`), for report detail strings.
pub(crate) fn fmt_num(n: f64) -> String {
    if n.is_nan() {
        "NaN".to_string()
    } else if n == f64::INFINITY {
        "Infinity".to_string()
    } else if n == f64::NEG_INFINITY {
        "-Infinity".to_string()
    } else {
        // Rust's shortest-roundtrip Display matches JS Number→string for the
        // magnitudes seen here (2-decimal mm values).
        format!("{n}")
    }
}
