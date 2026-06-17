//! Convenience constructors for common package classes.
//!
//! These keep the mapping from "a few KiCad-id parameters" to a full
//! [`PackageClass`] in one place, so both the footprint front door
//! (`vcad-ecad-symbols`) and the 3D body path (`vcad-ecad-pcb`) derive from the
//! same spec instead of re-parsing independently.

use vcad_ir::ecad::{
    BodyEnvelope, DensityLevel, LeadSpec, LeadTerminal, PackageClass, PackageFamily, PinMap,
    PinNumbering, ThermalPad,
};

/// A square QFN/no-lead package from its lead count, pitch, body size, and
/// exposed-pad edge length (all mm). `pins` is rounded down to a multiple of 4.
pub fn qfn(pins: u32, pitch: f64, body: f64, ep: f64) -> PackageClass {
    let pins = pins - (pins % 4);
    PackageClass {
        id: format!("QFN-{pins}"),
        family: PackageFamily::NoLead,
        body: BodyEnvelope {
            length: body,
            width: body,
            height: 0.9,
            standoff: 0.0,
        },
        leads: LeadSpec {
            pitch,
            count_per_side: (pins / 4).max(1),
            sides: 4,
            lead_length: 0.4,
            lead_width: 0.2,
            terminal: LeadTerminal::Smd,
        },
        thermal_pad: (ep > 0.0).then_some(ThermalPad {
            length: ep,
            width: ep,
        }),
        density: DensityLevel::Nominal,
        pin_map: PinMap {
            numbering: PinNumbering::Ccw,
            pins: vec![],
            polarity_marker: true,
        },
    }
}

/// A two-terminal chip passive from its imperial code (`"0402"`, `"0603"`,
/// `"0805"`, `"1206"`, `"1210"`, `"2010"`, `"2512"`, `"0201"`). Body and
/// terminal dimensions are the standard physical part sizes; the land pattern
/// is computed by IPC goals at derive time. Unknown codes default to 0805.
pub fn chip(code: &str) -> PackageClass {
    // (body_len, body_wid, height, term_len) in mm.
    let (l, w, h, term) = match code {
        "0201" => (0.6, 0.3, 0.3, 0.15),
        "0402" => (1.0, 0.5, 0.35, 0.25),
        "0603" => (1.6, 0.8, 0.45, 0.3),
        "0805" => (2.0, 1.25, 0.5, 0.4),
        "1206" => (3.2, 1.6, 0.55, 0.5),
        "1210" => (3.2, 2.5, 0.55, 0.5),
        "2010" => (5.0, 2.5, 0.6, 0.6),
        "2512" => (6.3, 3.2, 0.6, 0.6),
        _ => (2.0, 1.25, 0.5, 0.4), // default 0805
    };
    PackageClass {
        id: code.to_string(),
        family: PackageFamily::Chip,
        body: BodyEnvelope {
            length: l,
            width: w,
            height: h,
            standoff: 0.0,
        },
        leads: LeadSpec {
            pitch: 0.0,
            count_per_side: 1,
            sides: 2,
            lead_length: term,
            lead_width: w,
            terminal: LeadTerminal::Smd,
        },
        thermal_pad: None,
        density: DensityLevel::Nominal,
        pin_map: PinMap {
            numbering: PinNumbering::Sequential,
            pins: vec![],
            polarity_marker: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive;

    #[test]
    fn chip_preset_derives_two_pads() {
        let d = derive(&chip("0603")).unwrap();
        assert_eq!(d.footprint.pads.len(), 2);
        // Two symbol pins, bijective with pads.
        assert_eq!(d.symbol.pins.len(), 2);
        // Lands sit outside the 1.6mm body on either side.
        let x: Vec<f64> = d.footprint.pads.iter().map(|p| p.position.x).collect();
        assert!(x[0] < 0.0 && x[1] > 0.0);
        assert!(d.body.aabb().height() > 0.0);
    }

    #[test]
    fn qfn_preset_derives() {
        let d = derive(&qfn(40, 0.4, 5.0, 2.75)).unwrap();
        assert_eq!(d.footprint.pads.len(), 41);
        assert!(d.body.aabb().height() > 0.0);
    }

    #[test]
    fn qfn_preset_rounds_to_multiple_of_four() {
        let pc = qfn(42, 0.5, 6.0, 3.0);
        assert_eq!(pc.leads.count_per_side, 10); // 42 → 40 → 10/side
    }
}
