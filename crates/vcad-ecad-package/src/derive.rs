//! The unified generator: one [`PackageClass`] → one [`DerivedPart`]
//! ({footprint, symbol, body, courtyard}) in a single pass off one pin map.
//!
//! The whole point is that the pad numbering, the schematic pin numbering, and
//! the 3D body all come out of the *same* iteration, so they are bijective by
//! construction and can never drift.

use vcad_ir::ecad::{
    Box3D, DerivedPart, FootprintBody, FootprintGraphic, FootprintTemplate, PackageClass,
    PackageFamily, Pad, PadShape, PadType, PcbLayer, PinAssignment, PinRole, PinType, SchematicPin,
    SymbolDef, SymbolGraphic,
};
use vcad_ir::{Vec2, Vec3};

use crate::ipc7351;

/// Why a package could not be derived.
#[derive(Debug, Clone, PartialEq)]
pub enum DeriveError {
    /// No generator implemented for this family yet.
    UnsupportedFamily(PackageFamily),
    /// The package class is internally inconsistent.
    Invalid(String),
}

impl std::fmt::Display for DeriveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeriveError::UnsupportedFamily(fam) => {
                write!(f, "no generator for package family {fam:?} yet")
            }
            DeriveError::Invalid(msg) => write!(f, "invalid package class: {msg}"),
        }
    }
}

impl std::error::Error for DeriveError {}

/// Derive the footprint, symbol, body, and courtyard for a package class.
pub fn derive(pc: &PackageClass) -> Result<DerivedPart, DeriveError> {
    match pc.family {
        PackageFamily::NoLead => derive_no_lead(pc),
        PackageFamily::Chip => derive_chip(pc),
        other => Err(DeriveError::UnsupportedFamily(other)),
    }
}

/// Generate a two-terminal chip-passive land pattern, symbol, and body.
fn derive_chip(pc: &PackageClass) -> Result<DerivedPart, DeriveError> {
    let leads = &pc.leads;
    let g = ipc7351::goals(PackageFamily::Chip, pc.density);
    let body_half_x = pc.body.length / 2.0;

    // The terminal width tracks the body width; the metallization length is the
    // lead_length. Lands grow outward by toe, inward by heel, and the side goal
    // widens the land relative to the body.
    let (land_len, land_wid, outer_x) =
        ipc7351::land_for_terminal(leads.lead_length, leads.lead_width, body_half_x, &g);
    let row_center = outer_x - land_len / 2.0;

    let pads = vec![
        smd_rect("1", -row_center, 0.0, land_len, land_wid),
        smd_rect("2", row_center, 0.0, land_len, land_wid),
    ];

    let cy_half_x = outer_x + g.courtyard_excess;
    let cy_half_y = (pc.body.width / 2.0).max(land_wid / 2.0) + g.courtyard_excess;

    let z_min = pc.body.standoff;
    let z_max = pc.body.standoff + pc.body.height;
    let body = FootprintBody::Box {
        bbox: Box3D {
            min: Vec3::new(-body_half_x, -pc.body.width / 2.0, z_min),
            max: Vec3::new(body_half_x, pc.body.width / 2.0, z_max),
        },
    };
    let courtyard_aabb = Box3D {
        min: Vec3::new(-cy_half_x, -cy_half_y, z_min),
        max: Vec3::new(cy_half_x, cy_half_y, z_max.max(z_min + 0.01)),
    };

    let footprint = FootprintTemplate {
        name: pc.id.clone(),
        pads: pads.clone(),
        graphics: ic_graphics(body_half_x, pc.body.width / 2.0, cy_half_x, cy_half_y),
    };
    let symbol = build_symbol(pc, &pads);

    Ok(DerivedPart {
        footprint,
        symbol,
        body,
        courtyard_aabb,
        ipc: g,
    })
}

/// SMD pad on the standard front-copper land stack.
fn smd_rect(number: &str, x: f64, y: f64, w: f64, h: f64) -> Pad {
    Pad {
        number: number.to_string(),
        pad_type: PadType::SMD,
        shape: PadShape::Rect {
            width: w,
            height: h,
        },
        position: Vec2::new(x, y),
        rotation: 0.0,
        drill: None,
        net: None,
        layers: vec![PcbLayer::FCu, PcbLayer::FPaste, PcbLayer::FMask],
    }
}

/// Map a functional pin role to a schematic ERC pin type.
fn pin_type_for(role: PinRole) -> PinType {
    match role {
        PinRole::Power | PinRole::Ground | PinRole::Thermal => PinType::PowerInput,
        PinRole::NoConnect => PinType::NotConnected,
        PinRole::Io => PinType::Bidirectional,
        PinRole::Clock | PinRole::Reset | PinRole::Signal | PinRole::Gate => PinType::Input,
        PinRole::Analog
        | PinRole::Passive
        | PinRole::Anode
        | PinRole::Cathode
        | PinRole::Drain
        | PinRole::Source => PinType::Passive,
    }
}

/// Generate a QFN/DFN/SON no-lead land pattern, symbol, and body.
fn derive_no_lead(pc: &PackageClass) -> Result<DerivedPart, DeriveError> {
    let leads = &pc.leads;
    let pps = leads.count_per_side;
    if pps == 0 {
        return Err(DeriveError::Invalid("count_per_side must be > 0".into()));
    }
    if leads.sides != 2 && leads.sides != 4 {
        return Err(DeriveError::Invalid(format!(
            "no-lead supports 2 or 4 sides, got {}",
            leads.sides
        )));
    }

    let g = ipc7351::goals(pc.family, pc.density);
    let body_half_x = pc.body.length / 2.0;
    let body_half_y = pc.body.width / 2.0;

    // Land geometry is identical on every side (square terminal), but the row
    // center differs per axis for a rectangular body.
    let (land_len, land_wid, outer_x) =
        ipc7351::land_for_terminal(leads.lead_length, leads.lead_width, body_half_x, &g);
    let (_, _, outer_y) =
        ipc7351::land_for_terminal(leads.lead_length, leads.lead_width, body_half_y, &g);
    let row_center_x = outer_x - land_len / 2.0;
    let row_center_y = outer_y - land_len / 2.0;

    // Spread of pads along one side, centered.
    let edge = (pps as f64 - 1.0) / 2.0 * leads.pitch;

    let mut pads: Vec<Pad> = Vec::new();
    let mut num = 1u32;

    // CCW from pin 1, matching the legacy quad() convention so P1 wiring is a
    // drop-in swap: Left (top→bottom), Bottom (left→right), Right (bottom→top),
    // Top (right→left). For a 2-side (DFN) package only Left and Right run.
    // Left side: pads run radially in X (width = land_len), tangentially in Y.
    for i in 0..pps {
        let y = edge - i as f64 * leads.pitch;
        pads.push(smd_rect(
            &num.to_string(),
            -row_center_x,
            y,
            land_len,
            land_wid,
        ));
        num += 1;
    }
    if leads.sides == 4 {
        // Bottom side: pads run radially in Y (height = land_len).
        for i in 0..pps {
            let x = -edge + i as f64 * leads.pitch;
            pads.push(smd_rect(
                &num.to_string(),
                x,
                row_center_y,
                land_wid,
                land_len,
            ));
            num += 1;
        }
    }
    // Right side: bottom→top.
    for i in 0..pps {
        let y = -edge + i as f64 * leads.pitch;
        pads.push(smd_rect(
            &num.to_string(),
            row_center_x,
            y,
            land_len,
            land_wid,
        ));
        num += 1;
    }
    if leads.sides == 4 {
        // Top side: right→left.
        for i in 0..pps {
            let x = edge - i as f64 * leads.pitch;
            pads.push(smd_rect(
                &num.to_string(),
                x,
                -row_center_y,
                land_wid,
                land_len,
            ));
            num += 1;
        }
    }

    // Exposed thermal pad.
    if let Some(tp) = pc.thermal_pad {
        let mut ep = smd_rect("EP", 0.0, 0.0, tp.length, tp.width);
        // The exposed pad solders to the die paddle on all front layers.
        ep.layers = vec![PcbLayer::FCu, PcbLayer::FPaste, PcbLayer::FMask];
        pads.push(ep);
    }

    // Courtyard: max of land-outer and body half-extents, plus excess.
    let cy_half_x = outer_x.max(body_half_x) + g.courtyard_excess;
    let cy_half_y = outer_y.max(body_half_y) + g.courtyard_excess;

    // Body: centered box, base at standoff.
    let z_min = pc.body.standoff;
    let z_max = pc.body.standoff + pc.body.height;
    let body = FootprintBody::Box {
        bbox: Box3D {
            min: Vec3::new(-body_half_x, -body_half_y, z_min),
            max: Vec3::new(body_half_x, body_half_y, z_max),
        },
    };
    let courtyard_aabb = Box3D {
        min: Vec3::new(-cy_half_x, -cy_half_y, z_min),
        max: Vec3::new(cy_half_x, cy_half_y, z_max.max(z_min + 0.01)),
    };

    let graphics = ic_graphics(body_half_x, body_half_y, cy_half_x, cy_half_y);
    let footprint = FootprintTemplate {
        name: pc.id.clone(),
        pads: pads.clone(),
        graphics,
    };

    let symbol = build_symbol(pc, &pads);

    Ok(DerivedPart {
        footprint,
        symbol,
        body,
        courtyard_aabb,
        ipc: g,
    })
}

/// Silkscreen body outline + courtyard rectangle + pin-1 marker.
fn ic_graphics(bx: f64, by: f64, cx: f64, cy: f64) -> Vec<FootprintGraphic> {
    vec![
        // Courtyard rectangle (assembly).
        FootprintGraphic::Rect {
            start: Vec2::new(-cx, -cy),
            end: Vec2::new(cx, cy),
            width: 0.05,
            layer: PcbLayer::FCrtYd,
        },
        // Body outline (silk).
        FootprintGraphic::Rect {
            start: Vec2::new(-bx, -by),
            end: Vec2::new(bx, by),
            width: 0.12,
            layer: PcbLayer::FSilkS,
        },
        // Pin-1 marker dot, near the top-left body corner.
        FootprintGraphic::Circle {
            center: Vec2::new(-bx - 0.3, by + 0.3),
            radius: 0.1,
            width: 0.12,
            layer: PcbLayer::FSilkS,
        },
    ]
}

/// Build a schematic symbol whose pins are bijective with the footprint pads.
///
/// Pin identities come from the package's pin map when present; unspecified
/// pads default to passive pins named after their number. Layout is a simple
/// IC box with leads split left/right (cosmetic — only the numbering matters
/// for connectivity).
fn build_symbol(pc: &PackageClass, pads: &[Pad]) -> SymbolDef {
    // number → (name, role) lookup from the pin map.
    let lookup = |number: &str| -> (String, PinRole) {
        pc.pin_map
            .pins
            .iter()
            .find(|p: &&PinAssignment| p.number == number)
            .map(|p| (p.name.clone(), p.role))
            .unwrap_or_else(|| {
                let role = if number == "EP" {
                    PinRole::Thermal
                } else {
                    PinRole::Passive
                };
                (number.to_string(), role)
            })
    };

    let pitch = 2.54;
    let n = pads.len();
    let per_col = n.div_ceil(2);
    let col_h = (per_col.saturating_sub(1)) as f64 * pitch;
    let half_w = 5.08;

    let mut pins: Vec<SchematicPin> = Vec::with_capacity(n);
    for (i, pad) in pads.iter().enumerate() {
        let (name, role) = lookup(&pad.number);
        let (x, y) = if i < per_col {
            (-half_w - pitch, col_h / 2.0 - i as f64 * pitch)
        } else {
            let j = i - per_col;
            (half_w + pitch, col_h / 2.0 - j as f64 * pitch)
        };
        pins.push(SchematicPin {
            number: pad.number.clone(),
            name,
            pin_type: pin_type_for(role),
            position: Vec2::new(x, y),
        });
    }

    let box_h = col_h + 2.0 * pitch;
    let graphics = vec![SymbolGraphic::Rect {
        x: -half_w,
        y: -box_h / 2.0,
        width: 2.0 * half_w,
        height: box_h,
    }];

    SymbolDef {
        id: pc.id.clone(),
        name: pc.id.clone(),
        prefix: "U".to_string(),
        default_value: String::new(),
        pins,
        graphics,
        footprint_template: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use vcad_ir::ecad::{
        BodyEnvelope, DensityLevel, LeadSpec, LeadTerminal, PinMap, PinNumbering, ThermalPad,
    };

    /// A real-world QFN-40 5x5mm 0.4mm-pitch package.
    fn qfn_40() -> PackageClass {
        PackageClass {
            id: "QFN-40_5x5mm_P0.4mm".into(),
            family: PackageFamily::NoLead,
            body: BodyEnvelope {
                length: 5.0,
                width: 5.0,
                height: 0.9,
                standoff: 0.0,
            },
            leads: LeadSpec {
                pitch: 0.4,
                count_per_side: 10,
                sides: 4,
                lead_length: 0.4,
                lead_width: 0.2,
                terminal: LeadTerminal::Smd,
            },
            thermal_pad: Some(ThermalPad {
                length: 3.7,
                width: 3.7,
            }),
            density: DensityLevel::Nominal,
            pin_map: PinMap {
                numbering: PinNumbering::Ccw,
                pins: vec![],
                polarity_marker: true,
            },
        }
    }

    #[test]
    fn qfn_pad_count_includes_thermal() {
        let d = derive(&qfn_40()).unwrap();
        assert_eq!(d.footprint.pads.len(), 41, "40 leads + 1 exposed pad");
    }

    #[test]
    fn qfn_lead_numbering_is_one_through_forty_plus_ep() {
        let d = derive(&qfn_40()).unwrap();
        let nums: BTreeSet<String> = d.footprint.pads.iter().map(|p| p.number.clone()).collect();
        for i in 1..=40 {
            assert!(nums.contains(&i.to_string()), "missing pad {i}");
        }
        assert!(nums.contains("EP"), "missing exposed pad");
    }

    #[test]
    fn qfn_pitch_preserved_along_a_side() {
        let d = derive(&qfn_40()).unwrap();
        // Pads 1..10 are the left side, top→bottom; consecutive Y delta == pitch.
        let p1 = &d.footprint.pads[0];
        let p2 = &d.footprint.pads[1];
        assert!((p1.position.x - p2.position.x).abs() < 1e-9, "same column");
        assert!(
            ((p1.position.y - p2.position.y).abs() - 0.4).abs() < 1e-9,
            "pitch must be 0.4mm, got {}",
            (p1.position.y - p2.position.y).abs()
        );
    }

    #[test]
    fn qfn_outer_edge_sits_toe_beyond_body() {
        let d = derive(&qfn_40()).unwrap();
        // A left-side pad: outer (most negative X) edge == -(2.5 + toe).
        let p1 = &d.footprint.pads[0];
        let (w, _h) = match p1.shape {
            PadShape::Rect { width, height } => (width, height),
            _ => panic!("expected rect pad"),
        };
        let outer_edge = p1.position.x - w / 2.0; // most-negative X
        let expected = -(2.5 + d.ipc.toe);
        assert!(
            (outer_edge - expected).abs() < 1e-9,
            "land outer edge {outer_edge} should be {expected}"
        );
    }

    #[test]
    fn qfn_land_width_avoids_bridging() {
        let d = derive(&qfn_40()).unwrap();
        // Tangential land width (height for a left-side pad) must be < pitch.
        let p1 = &d.footprint.pads[0];
        if let PadShape::Rect { height, .. } = p1.shape {
            assert!(height < 0.4, "land width {height} must be < 0.4mm pitch");
            assert!(height > 0.0);
        }
    }

    #[test]
    fn symbol_pins_are_bijective_with_pads() {
        let d = derive(&qfn_40()).unwrap();
        let pad_nums: BTreeSet<String> =
            d.footprint.pads.iter().map(|p| p.number.clone()).collect();
        let pin_nums: BTreeSet<String> = d.symbol.pins.iter().map(|p| p.number.clone()).collect();
        assert_eq!(
            pad_nums, pin_nums,
            "every pad must have exactly one matching symbol pin"
        );
    }

    #[test]
    fn body_is_nonzero_and_inside_courtyard() {
        let d = derive(&qfn_40()).unwrap();
        let bb = d.body.aabb();
        assert!(bb.height() > 0.0, "body must have nonzero height");
        // Body XY corners lie within the courtyard.
        for &(sx, sy) in &[(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
            let corner = Vec2::new(sx * bb.max.x, sy * bb.max.y);
            assert!(
                d.courtyard_aabb.contains_xy(corner),
                "body corner {corner:?} must lie inside courtyard"
            );
        }
    }

    #[test]
    fn courtyard_encloses_all_pads() {
        let d = derive(&qfn_40()).unwrap();
        for pad in &d.footprint.pads {
            assert!(
                d.courtyard_aabb.contains_xy(pad.position),
                "pad {} center {:?} escaped courtyard",
                pad.number,
                pad.position
            );
        }
    }

    #[test]
    fn dfn_two_sided_has_no_top_bottom() {
        let mut pc = qfn_40();
        pc.id = "DFN-8_2x2mm_P0.5mm".into();
        pc.leads.sides = 2;
        pc.leads.count_per_side = 4;
        pc.body = BodyEnvelope {
            length: 2.0,
            width: 2.0,
            height: 0.75,
            standoff: 0.0,
        };
        pc.thermal_pad = None;
        let d = derive(&pc).unwrap();
        assert_eq!(d.footprint.pads.len(), 8, "2 sides × 4");
    }

    #[test]
    fn unsupported_family_errors() {
        let mut pc = qfn_40();
        pc.family = PackageFamily::Bga;
        assert!(matches!(
            derive(&pc),
            Err(DeriveError::UnsupportedFamily(PackageFamily::Bga))
        ));
    }
}
