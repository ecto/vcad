//! Catalog-backed fastener forms for the loon stdlib.
//!
//! A fastener in loon is declared by its *axis*, not by hand-written
//! rotations:
//!
//! ```text
//! [bolt "M4x12" "shcs" 10 0 0  10 0 -12]
//! ```
//!
//! and this module turns that declaration into geometry plus a bill-of-
//! materials line. Three things follow from doing it here rather than in
//! user code:
//!
//! * **Orientation is derived.** The head/shaft pair is built once in a local
//!   frame with the shaft along `+Z`, then rotated as a unit onto the
//!   requested axis. Mirroring a subassembly mirrors the whole fastener, so a
//!   head can no longer end up on the wrong side of its flange.
//! * **Heads are real.** A protruding cap head is modeled where it actually
//!   is, and a countersunk head is actually countersunk (flush, protrusion
//!   zero) — so a swept-volume clearance check sees what the shop will see.
//! * **Counts come from the geometry.** Every placed fastener emits a
//!   [`HardwareLine`], multiplied through enclosing patterns, so the BOM is
//!   derived rather than tallied by hand.
//!
//! Dimensions come from [`vcad_parts::fasteners`] (the same tables the
//! built-in fastener parts use); catalog ids, part numbers and the set of
//! *stocked* lengths come from `lib/parts/mechanical.json`, the catalog
//! behind `search_mechanical_parts`. A length that isn't stocked (`M4x11`) is
//! rejected rather than modeled.

use std::collections::HashMap;
use std::sync::OnceLock;

use vcad_ir::{HardwareLine, Vec3};
use vcad_parts::fasteners as dims;

/// The mechanical catalog, embedded so the converter needs no filesystem.
const MECHANICAL_JSON: &str = include_str!("../../../lib/parts/mechanical.json");

/// Head style of a fastener.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadStyle {
    /// Socket-head cap screw (ISO 4762 / DIN 912). Cylindrical, protruding.
    Shcs,
    /// Button-head cap screw (ISO 7380). Domed, protruding, lower profile.
    Bhcs,
    /// Countersunk flat head (ISO 10642), 90° included angle. Sits flush:
    /// the head is *inside* the material and protrudes by zero.
    Flat,
}

impl HeadStyle {
    /// Parse the loon-facing style name.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "shcs" | "socket" | "cap" => Ok(HeadStyle::Shcs),
            "bhcs" | "button" => Ok(HeadStyle::Bhcs),
            "flat" | "fhcs" | "countersunk" | "csk" => Ok(HeadStyle::Flat),
            other => Err(format!(
                "unknown head style {other:?} — expected \"shcs\", \"bhcs\", or \"flat\""
            )),
        }
    }

    /// Catalog id suffix used by `lib/parts/mechanical.json`.
    fn catalog_suffix(self) -> &'static str {
        match self {
            HeadStyle::Shcs => "shcs",
            HeadStyle::Bhcs => "bhcs",
            HeadStyle::Flat => "fhcs",
        }
    }

    /// Short designation used in BOM lines.
    fn label(self) -> &'static str {
        match self {
            HeadStyle::Shcs => "SHCS",
            HeadStyle::Bhcs => "BHCS",
            HeadStyle::Flat => "flat head",
        }
    }

    /// Head height (mm) for a metric size.
    fn head_height(self, size: &str) -> f64 {
        match self {
            HeadStyle::Shcs => dims::socket_head_height(size),
            // ISO 7380 button head height ≈ 0.55·d.
            HeadStyle::Bhcs => 0.55 * dims::metric_major_dia(size),
            // ISO 10642 countersunk head height ≈ 0.5·d (90° included).
            HeadStyle::Flat => 0.5 * dims::metric_major_dia(size),
        }
    }

    /// Head diameter (mm) for a metric size.
    fn head_dia(self, size: &str) -> f64 {
        match self {
            HeadStyle::Shcs => dims::socket_head_dia(size),
            // ISO 7380 button head diameter ≈ 1.9·d.
            HeadStyle::Bhcs => 1.9 * dims::metric_major_dia(size),
            // ISO 10642 countersunk head diameter ≈ 2.0·d.
            HeadStyle::Flat => 2.0 * dims::metric_major_dia(size),
        }
    }

    /// How far the head stands proud of the mating face (mm).
    ///
    /// This is the number that matters for interference with a moving
    /// linkage — and it is zero for a countersunk head by construction.
    pub fn protrusion(self, size: &str) -> f64 {
        match self {
            HeadStyle::Flat => 0.0,
            _ => self.head_height(size),
        }
    }

    /// Whether the catalog length is measured over the head (countersunk) or
    /// under it (protruding heads) — the ISO convention.
    fn length_includes_head(self) -> bool {
        matches!(self, HeadStyle::Flat)
    }
}

/// One item in a fastener stack (washers and nuts sharing the bolt's axis).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackItem {
    /// ISO 7089 flat washer.
    Washer,
    /// ISO 4032 hex nut.
    Nut,
}

impl StackItem {
    fn parse(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "washer" => Ok(StackItem::Washer),
            "nut" => Ok(StackItem::Nut),
            other => Err(format!(
                "unknown stack item {other:?} — expected \"washer\" or \"nut\""
            )),
        }
    }
}

/// A parsed fastener designation, e.g. `"M4x12"`.
#[derive(Debug, Clone, PartialEq)]
pub struct Designation {
    /// Metric size, e.g. `"M4"`.
    pub size: String,
    /// Nominal length in mm.
    pub length: f64,
}

impl Designation {
    /// Parse `"M4x12"` / `"M4X12"` / `"M4-12"`.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let trimmed = spec.trim();
        let split = trimmed
            .find(['x', 'X', '-'])
            .ok_or_else(|| format!("bad fastener spec {spec:?} — expected e.g. \"M4x12\""))?;
        let (size, rest) = trimmed.split_at(split);
        let length: f64 = rest[1..]
            .trim()
            .parse()
            .map_err(|_| format!("bad length in fastener spec {spec:?}"))?;
        let size = size.trim().to_ascii_uppercase();
        if !size.starts_with('M') || size.len() < 2 {
            return Err(format!(
                "bad size in fastener spec {spec:?} — expected a metric size like \"M4\""
            ));
        }
        if length <= 0.0 {
            return Err(format!("fastener length must be positive, got {length}"));
        }
        Ok(Designation { size, length })
    }
}

// ---------------------------------------------------------------------------
// Catalog
// ---------------------------------------------------------------------------

/// What the mechanical catalog knows about one screw family.
struct CatalogScrew {
    id: String,
    lengths_mm: Vec<f64>,
}

fn screw_catalog() -> &'static HashMap<String, CatalogScrew> {
    static CATALOG: OnceLock<HashMap<String, CatalogScrew>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let mut out = HashMap::new();
        let Ok(root) = serde_json::from_str::<serde_json::Value>(MECHANICAL_JSON) else {
            return out;
        };
        let Some(parts) = root.get("parts").and_then(|p| p.as_array()) else {
            return out;
        };
        for part in parts {
            if part.get("type").and_then(|t| t.as_str()) != Some("screw") {
                continue;
            }
            let Some(id) = part.get("id").and_then(|i| i.as_str()) else {
                continue;
            };
            // `screw.m4-shcs` → key `m4-shcs`. Stainless/set-screw variants
            // (`screw.m3-shcs-ss`, `screw.m4-set`) keep their own key and are
            // simply not matched by the `<size>-<style>` lookup.
            let key = id.trim_start_matches("screw.").to_string();
            let lengths = part
                .get("spec")
                .and_then(|s| s.get("lengths_mm"))
                .and_then(|l| l.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_f64()).collect::<Vec<_>>())
                .unwrap_or_default();
            out.insert(
                key,
                CatalogScrew {
                    id: id.to_string(),
                    lengths_mm: lengths,
                },
            );
        }
        out
    })
}

fn lookup_screw(size: &str, style: HeadStyle) -> Option<&'static CatalogScrew> {
    let key = format!("{}-{}", size.to_ascii_lowercase(), style.catalog_suffix());
    screw_catalog().get(&key)
}

// ---------------------------------------------------------------------------
// Plan
// ---------------------------------------------------------------------------

/// A primitive piece of fastener geometry, in the local frame where the
/// shaft runs along `+Z` from `z = 0` (the mating face) toward the tip.
#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    /// Solid cylinder spanning `z0 .. z0 + height`.
    Cylinder {
        /// Radius in mm.
        radius: f64,
        /// Height in mm.
        height: f64,
        /// Base of the cylinder along the local axis.
        z0: f64,
    },
    /// Solid regular prism (hex nut bodies, hex socket recesses).
    Prism {
        /// Number of sides.
        sides: u32,
        /// Circumscribed radius in mm.
        radius: f64,
        /// Height in mm.
        height: f64,
        /// Base of the prism along the local axis.
        z0: f64,
    },
    /// Truncated cone spanning `z0 .. z0 + height` (countersunk heads).
    Cone {
        /// Radius at `z0`.
        radius_bottom: f64,
        /// Radius at `z0 + height`.
        radius_top: f64,
        /// Height in mm.
        height: f64,
        /// Base of the cone along the local axis.
        z0: f64,
    },
    /// Spherical cap of `height`, base diameter `2 * base_radius`, base at
    /// `z0`, doming toward `-Z` (button heads).
    Dome {
        /// Radius of the flat base.
        base_radius: f64,
        /// Cap height in mm.
        height: f64,
        /// Plane of the flat base along the local axis.
        z0: f64,
    },
}

/// A fully resolved fastener: geometry to add, geometry to subtract, the axis
/// it sits on, and the hardware it consumes.
#[derive(Debug, Clone, PartialEq)]
pub struct FastenerPlan {
    /// Pieces unioned together, in the local `+Z` frame.
    pub additive: Vec<Piece>,
    /// Pieces subtracted from the union (hex socket recesses).
    pub subtractive: Vec<Piece>,
    /// Origin of the local frame in world space (the mating face).
    pub origin: Vec3,
    /// Unit vector the local `+Z` maps onto (head → tip).
    pub axis: Vec3,
    /// BOM lines for one placement.
    pub hardware: Vec<HardwareLine>,
}

/// Build the plan for a single fastener.
///
/// `from` is the mating face the head bears against; `to` is where the shaft
/// tip should land. The distance between them is the *grip* — the material
/// the bolt passes through — and it is what the length check is made
/// against, so a bolt can no longer be silently too short or too long for
/// the stack it clamps.
pub fn plan(
    spec: &str,
    style: &str,
    from: Vec3,
    to: Vec3,
    stack: &[String],
) -> Result<FastenerPlan, String> {
    let designation = Designation::parse(spec)?;
    let style = HeadStyle::parse(style)?;
    let size = designation.size.as_str();
    let length = designation.length;

    let axis_vec = Vec3::new(to.x - from.x, to.y - from.y, to.z - from.z);
    let grip = (axis_vec.x * axis_vec.x + axis_vec.y * axis_vec.y + axis_vec.z * axis_vec.z).sqrt();
    if grip <= 1e-9 {
        return Err(format!(
            "fastener {spec}: from-point and to-point coincide — the axis is undefined"
        ));
    }
    let axis = Vec3::new(axis_vec.x / grip, axis_vec.y / grip, axis_vec.z / grip);

    // Reject a length the catalog does not stock (the M4x11 case).
    let catalog = lookup_screw(size, style);
    if let Some(cat) = catalog {
        if !cat.lengths_mm.is_empty() && !cat.lengths_mm.iter().any(|l| (l - length).abs() < 1e-6) {
            let available = cat
                .lengths_mm
                .iter()
                .map(|l| format!("{l}"))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "{spec} is not a stocked length for {} — available: {available} mm",
                cat.id
            ));
        }
    }

    // Split the stack: washers before the first nut sit under the head,
    // everything from the first nut onward sits past the tip (a washer
    // listed after the nut goes between the material and the nut).
    let items: Vec<StackItem> = stack
        .iter()
        .map(|s| StackItem::parse(s))
        .collect::<Result<_, _>>()?;
    let nut_at = items.iter().position(|i| *i == StackItem::Nut);
    let (head_side, far_side) = match nut_at {
        Some(i) => (&items[..i], &items[i..]),
        None => (&items[..], &items[items.len()..]),
    };
    if head_side.contains(&StackItem::Nut) {
        return Err("fastener stack: only one nut is supported".into());
    }
    if far_side.iter().filter(|i| **i == StackItem::Nut).count() > 1 {
        return Err("fastener stack: only one nut is supported".into());
    }
    if matches!(style, HeadStyle::Flat) && !head_side.is_empty() {
        return Err(format!(
            "fastener {spec}: a countersunk head sits flush — it cannot have a washer under it"
        ));
    }

    let washer_t = dims::washer_thickness(size);
    let washer_od = dims::washer_outer_dia(size);
    let nut_t = dims::nut_thickness(size);
    let nut_af = dims::nut_across_flats(size);
    let shaft_r = dims::metric_major_dia(size) / 2.0;
    let head_h = style.head_height(size);
    let head_r = style.head_dia(size) / 2.0;

    let head_washers = head_side.len() as f64;
    let far_washers = far_side.iter().filter(|i| **i == StackItem::Washer).count() as f64;
    let has_nut = far_side.contains(&StackItem::Nut);

    // Length check. `usable` is how much of the bolt is available past the
    // head bearing face; it must clear the whole stack, and for a nut it must
    // also show ~1.5 thread pitches of thread beyond.
    let usable = if style.length_includes_head() {
        length - head_h
    } else {
        length
    };
    let pitch = coarse_pitch(size);
    let needed =
        head_washers * washer_t + grip + far_washers * washer_t + nut_t * has_nut as u8 as f64;
    let protrusion_min = if has_nut { 1.5 * pitch } else { 0.0 };
    if usable + 1e-6 < needed + protrusion_min {
        return Err(format!(
            "fastener {spec} is too short: {usable} mm of usable length, but the stack \
             needs {} mm (grip {grip:.2} mm{}{}) plus {protrusion_min} mm of thread past the nut",
            needed,
            if head_washers > 0.0 {
                format!(", {head_washers} washer(s) under the head")
            } else {
                String::new()
            },
            if has_nut { ", 1 nut" } else { "" },
        ));
    }
    // Bottoming out in a blind tapped hole is the other half of the check: a
    // bolt far longer than what it passes through is a mistake worth naming.
    let slack = usable - needed - protrusion_min;
    if !has_nut && slack > 3.0 * dims::metric_major_dia(size) {
        return Err(format!(
            "fastener {spec} is too long: {slack:.1} mm of shaft extends past the \
             {grip:.2} mm it passes through (more than 3·d) — it will bottom out or \
             stick out. Pick a shorter length."
        ));
    }

    let mut additive = Vec::new();
    let mut subtractive = Vec::new();
    let mut hardware = Vec::new();

    // Head-side washers: stacked outward from the mating face at z = 0.
    let mut head_face = 0.0;
    for _ in head_side {
        additive.push(Piece::Cylinder {
            radius: washer_od / 2.0,
            height: washer_t,
            z0: head_face - washer_t,
        });
        head_face -= washer_t;
    }

    match style {
        HeadStyle::Shcs => {
            additive.push(Piece::Cylinder {
                radius: head_r,
                height: head_h,
                z0: head_face - head_h,
            });
            // Hex socket, sunk into the outer face of the head.
            let hex_w = dims::socket_hex_width(size);
            let depth = head_h * 0.5;
            subtractive.push(Piece::Prism {
                sides: 6,
                radius: hex_w / 2.0 / 0.866,
                height: depth + 0.1,
                z0: head_face - head_h - 0.05,
            });
        }
        HeadStyle::Bhcs => {
            additive.push(Piece::Dome {
                base_radius: head_r,
                height: head_h,
                z0: head_face,
            });
            let hex_w = dims::socket_hex_width(size);
            let depth = head_h * 0.5;
            subtractive.push(Piece::Prism {
                sides: 6,
                radius: hex_w / 2.0 / 0.866,
                height: depth,
                z0: head_face - depth,
            });
        }
        HeadStyle::Flat => {
            // Countersunk: the head is *inside* the material, tapering from
            // the flush face at z = 0 down to the shaft.
            additive.push(Piece::Cone {
                radius_bottom: head_r,
                radius_top: shaft_r,
                height: head_h,
                z0: 0.0,
            });
        }
    }

    // Shaft: from the mating face to the tip.
    let shaft_start = if style.length_includes_head() {
        head_h
    } else {
        0.0
    };
    additive.push(Piece::Cylinder {
        radius: shaft_r,
        height: usable,
        z0: shaft_start,
    });

    // Far-side washers then the nut, stacked outward from the far face.
    let mut far_face = grip;
    for item in far_side {
        match item {
            StackItem::Washer => {
                additive.push(Piece::Cylinder {
                    radius: washer_od / 2.0,
                    height: washer_t,
                    z0: far_face,
                });
                far_face += washer_t;
            }
            StackItem::Nut => {
                additive.push(Piece::Prism {
                    sides: 6,
                    radius: nut_af / 2.0 / 0.866,
                    height: nut_t,
                    z0: far_face,
                });
                far_face += nut_t;
            }
        }
    }

    hardware.push(HardwareLine {
        catalog_id: catalog.map(|c| c.id.clone()),
        spec: format!("{}x{} {}", size, length, style.label()),
        qty: 1,
        head_protrusion_mm: Some(style.protrusion(size)),
    });
    let washers = head_washers as u32 + far_washers as u32;
    if washers > 0 {
        hardware.push(HardwareLine {
            catalog_id: None,
            spec: format!("{size} flat washer (ISO 7089)"),
            qty: washers,
            head_protrusion_mm: None,
        });
    }
    if has_nut {
        hardware.push(HardwareLine {
            catalog_id: None,
            spec: format!("{size} hex nut (ISO 4032)"),
            qty: 1,
            head_protrusion_mm: None,
        });
    }

    Ok(FastenerPlan {
        additive,
        subtractive,
        origin: from,
        axis,
        hardware,
    })
}

/// ISO coarse thread pitch (mm) for a metric size.
fn coarse_pitch(size: &str) -> f64 {
    match size {
        "M2" => 0.4,
        "M2.5" => 0.45,
        "M3" => 0.5,
        "M4" => 0.7,
        "M5" => 0.8,
        "M6" => 1.0,
        "M8" => 1.25,
        "M10" => 1.5,
        "M12" => 1.75,
        "M16" => 2.0,
        "M20" => 2.5,
        _ => 1.0,
    }
}

/// Clearance-hole diameter (mm) — ISO 273 "medium" fit.
pub fn clearance_hole_dia(size: &str) -> f64 {
    match size {
        "M2" => 2.4,
        "M2.5" => 2.9,
        "M3" => 3.4,
        "M4" => 4.5,
        "M5" => 5.5,
        "M6" => 6.6,
        "M8" => 9.0,
        "M10" => 11.0,
        "M12" => 13.5,
        "M16" => 17.5,
        "M20" => 22.0,
        _ => dims::metric_major_dia(size) * 1.1,
    }
}

/// Tap-drill diameter (mm) for a coarse ISO thread (~75% thread engagement).
pub fn tap_drill_dia(size: &str) -> f64 {
    dims::metric_major_dia(size) - coarse_pitch(size)
}

/// Rotation (Euler XYZ degrees, applied X then Y then Z) that takes local
/// `+Z` onto `axis`.
///
/// This is the piece that used to be two hand-written rotates per bolt.
pub fn axis_to_euler_xyz(axis: Vec3) -> Vec3 {
    let n = (axis.x * axis.x + axis.y * axis.y + axis.z * axis.z).sqrt();
    if n < 1e-12 {
        return Vec3::new(0.0, 0.0, 0.0);
    }
    let (x, y, z) = (axis.x / n, axis.y / n, axis.z / n);

    // Rodrigues rotation taking (0,0,1) → (x,y,z): axis v = ẑ × d, angle from
    // c = ẑ · d.
    let (vx, vy, vz) = (-y, x, 0.0);
    let s = (vx * vx + vy * vy + vz * vz).sqrt();
    let c = z;
    let m = if s < 1e-12 {
        if c > 0.0 {
            // Already aligned.
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
        } else {
            // Antiparallel: 180° about X.
            [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]]
        }
    } else {
        let (kx, ky, kz) = (vx / s, vy / s, vz / s);
        let t = 1.0 - c;
        [
            [t * kx * kx + c, t * kx * ky - s * kz, t * kx * kz + s * ky],
            [t * kx * ky + s * kz, t * ky * ky + c, t * ky * kz - s * kx],
            [t * kx * kz - s * ky, t * ky * kz + s * kx, t * kz * kz + c],
        ]
    };

    // Decompose R = Rz(c)·Ry(b)·Rx(a) — the order CsgOp::Rotate applies.
    let sy = -m[2][0];
    let (ax, ay, az) = if sy.abs() > 1.0 - 1e-9 {
        // Gimbal lock: pitch is ±90°, fold roll into yaw.
        (0.0_f64, sy.clamp(-1.0, 1.0).asin(), -m[0][1].atan2(m[1][1]))
    } else {
        (
            m[2][1].atan2(m[2][2]),
            sy.clamp(-1.0, 1.0).asin(),
            m[1][0].atan2(m[0][0]),
        )
    };

    Vec3::new(ax.to_degrees(), ay.to_degrees(), az.to_degrees())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rotate_xyz(angles: Vec3, p: Vec3) -> Vec3 {
        let (a, b, c) = (
            angles.x.to_radians(),
            angles.y.to_radians(),
            angles.z.to_radians(),
        );
        // Rx
        let (y, z) = (p.y * a.cos() - p.z * a.sin(), p.y * a.sin() + p.z * a.cos());
        let (x, y, z) = (p.x, y, z);
        // Ry
        let (x, z) = (x * b.cos() + z * b.sin(), -x * b.sin() + z * b.cos());
        // Rz
        let (x, y) = (x * c.cos() - y * c.sin(), x * c.sin() + y * c.cos());
        Vec3::new(x, y, z)
    }

    #[test]
    fn euler_maps_z_onto_every_axis() {
        let axes = [
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(-1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.0, -1.0, 0.0),
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::new(-0.3, 0.7, -0.2),
        ];
        for a in axes {
            let n = (a.x * a.x + a.y * a.y + a.z * a.z).sqrt();
            let unit = Vec3::new(a.x / n, a.y / n, a.z / n);
            let e = axis_to_euler_xyz(unit);
            let got = rotate_xyz(e, Vec3::new(0.0, 0.0, 1.0));
            assert!(
                (got.x - unit.x).abs() < 1e-9
                    && (got.y - unit.y).abs() < 1e-9
                    && (got.z - unit.z).abs() < 1e-9,
                "axis {unit:?} → euler {e:?} → {got:?}"
            );
        }
    }

    #[test]
    fn designation_parses_and_rejects_junk() {
        let d = Designation::parse("M4x12").unwrap();
        assert_eq!(d.size, "M4");
        assert_eq!(d.length, 12.0);
        assert!(Designation::parse("M4").is_err());
        assert!(Designation::parse("4x12").is_err());
    }

    #[test]
    fn unstocked_length_is_rejected() {
        let err = plan(
            "M4x11",
            "shcs",
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 8.0),
            &[],
        )
        .unwrap_err();
        assert!(err.contains("not a stocked length"), "{err}");
    }

    #[test]
    fn too_short_for_its_stack_is_rejected() {
        // M4x12 through 10 mm of plate with a washer and a nut cannot work.
        let err = plan(
            "M4x12",
            "shcs",
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 10.0),
            &["washer".into(), "nut".into()],
        )
        .unwrap_err();
        assert!(err.contains("too short"), "{err}");
    }

    #[test]
    fn too_long_for_a_blind_hole_is_rejected() {
        let err = plan(
            "M4x40",
            "shcs",
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 6.0),
            &[],
        )
        .unwrap_err();
        assert!(err.contains("too long"), "{err}");
    }

    #[test]
    fn flat_head_is_flush() {
        assert_eq!(HeadStyle::Flat.protrusion("M4"), 0.0);
        assert!(HeadStyle::Shcs.protrusion("M4") > 0.0);
        let p = plan(
            "M4x12",
            "flat",
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 8.0),
            &[],
        )
        .unwrap();
        // No additive geometry above the mating face.
        for piece in &p.additive {
            let z0 = match piece {
                Piece::Cylinder { z0, .. }
                | Piece::Prism { z0, .. }
                | Piece::Cone { z0, .. }
                | Piece::Dome { z0, .. } => *z0,
            };
            assert!(
                z0 >= -1e-9,
                "flat head has geometry above the face: {piece:?}"
            );
        }
    }

    #[test]
    fn hardware_lines_name_the_catalog_part() {
        let p = plan(
            "M4x12",
            "shcs",
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 8.0),
            &[],
        )
        .unwrap();
        assert_eq!(p.hardware.len(), 1);
        assert_eq!(p.hardware[0].catalog_id.as_deref(), Some("screw.m4-shcs"));
        assert_eq!(p.hardware[0].qty, 1);
        assert_eq!(p.hardware[0].head_protrusion_mm, Some(4.0));
    }
}
