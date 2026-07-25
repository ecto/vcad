//! Recover the pad rectangle a Gerber file actually describes.
//!
//! Shared by the cross-surface test; lives here rather than in the library so
//! the crate itself doesn't take a dependency on the exporter.

use std::collections::HashMap;

use vcad_ecad_invariants::PadRect;
use vcad_ir::Vec2;

/// An aperture recovered from a `%ADD...%` definition (plus its macro body, if
/// it is a rotated one).
#[derive(Debug, Clone)]
pub struct Aperture {
    /// Width along the aperture's local X (mm). Diameter, for a circle.
    pub width: f64,
    /// Height along the aperture's local Y (mm). Diameter, for a circle.
    pub height: f64,
    /// Rotation in degrees CCW.
    pub rot_deg: f64,
    /// True for a plain `C` circle aperture.
    pub is_round: bool,
}

/// A flash: an aperture struck at a board coordinate.
#[derive(Debug, Clone)]
pub struct Flash {
    /// Board-space centre (mm).
    pub center: Vec2,
    /// The aperture struck.
    pub aperture: Aperture,
}

impl Flash {
    /// The pad rectangle this flash describes.
    pub fn rect(&self) -> PadRect {
        PadRect {
            center: self.center,
            half_w: self.aperture.width / 2.0,
            half_h: self.aperture.height / 2.0,
            rot_deg: self.aperture.rot_deg,
            is_round: self.aperture.is_round,
        }
    }
}

/// Parse every `D03` flash out of a Gerber layer, resolving its aperture.
///
/// Understands `C`, `R`, `O` and the `ROT<code>` aperture macros this repo's
/// exporter emits for rotated pads (a `21` centre-line primitive, optionally
/// capped by two `1` circles for an obround).
pub fn parse_flashes(gerber: &str) -> Vec<Flash> {
    // 1) Macro bodies, keyed by macro name.
    let mut macros: HashMap<String, Aperture> = HashMap::new();
    let mut current_macro: Option<String> = None;
    let mut body: Vec<String> = Vec::new();

    let flush = |name: &Option<String>, body: &[String], macros: &mut HashMap<String, Aperture>| {
        let Some(name) = name else { return };
        // The centre-line primitive carries the whole orientation; the cap
        // circles only round the ends off.
        // An obround's cap circles extend it by one cap diameter along the
        // long axis, so the centre line alone under-reads the total width.
        let caps: f64 = body
            .iter()
            .filter_map(|l| {
                let f: Vec<&str> = l.split(',').collect();
                (f.first() == Some(&"1") && f.len() >= 3).then(|| f[2].parse::<f64>().unwrap())
            })
            .fold(0.0f64, f64::max);
        for line in body {
            let f: Vec<&str> = line.split(',').collect();
            if f.first() == Some(&"21") && f.len() >= 7 {
                macros.insert(
                    name.clone(),
                    Aperture {
                        width: f[2].parse::<f64>().unwrap() + caps,
                        height: f[3].parse().unwrap(),
                        rot_deg: f[6].parse().unwrap(),
                        is_round: false,
                    },
                );
                return;
            }
        }
        // Circle-only macro (a degenerate obround).
        for line in body {
            let f: Vec<&str> = line.split(',').collect();
            if f.first() == Some(&"1") && f.len() >= 3 {
                let d: f64 = f[2].parse().unwrap();
                macros.insert(
                    name.clone(),
                    Aperture {
                        width: d,
                        height: d,
                        rot_deg: 0.0,
                        is_round: true,
                    },
                );
                return;
            }
        }
    };

    for raw in gerber.lines() {
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix("%AM") {
            flush(&current_macro, &body, &mut macros);
            body.clear();
            current_macro = Some(rest.trim_end_matches('*').to_string());
            continue;
        }
        if current_macro.is_some() {
            if line.starts_with("%AD") {
                flush(&current_macro, &body, &mut macros);
                body.clear();
                current_macro = None;
            } else {
                body.push(line.trim_end_matches(['%', '*']).to_string());
                continue;
            }
        }
    }
    flush(&current_macro, &body, &mut macros);

    // 2) Aperture definitions.
    let mut apertures: HashMap<u32, Aperture> = HashMap::new();
    for raw in gerber.lines() {
        let line = raw.trim();
        let Some(rest) = line.strip_prefix("%ADD") else {
            continue;
        };
        let rest = rest.trim_end_matches("*%");
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        let Ok(code) = digits.parse::<u32>() else {
            continue;
        };
        let spec = &rest[digits.len()..];
        let ap = if let Some(dims) = spec.strip_prefix("C,") {
            let d: f64 = dims.parse().unwrap();
            Aperture {
                width: d,
                height: d,
                rot_deg: 0.0,
                is_round: true,
            }
        } else if let Some(dims) = spec.strip_prefix("R,").or(spec.strip_prefix("O,")) {
            let (w, h) = dims.split_once('X').unwrap();
            Aperture {
                width: w.parse().unwrap(),
                height: h.parse().unwrap(),
                rot_deg: 0.0,
                is_round: false,
            }
        } else if let Some(m) = macros.get(spec) {
            m.clone()
        } else {
            continue;
        };
        apertures.insert(code, ap);
    }

    // 3) Flashes.
    let mut out = Vec::new();
    let mut current: Option<u32> = None;
    for raw in gerber.lines() {
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix('D') {
            if let Ok(code) = rest.trim_end_matches('*').parse::<u32>() {
                current = Some(code);
                continue;
            }
        }
        if !line.ends_with("D03*") || !line.starts_with('X') {
            continue;
        }
        let Some(ap) = current.and_then(|c| apertures.get(&c)) else {
            continue;
        };
        let body = line.trim_end_matches("D03*");
        let (x, y) = body[1..].split_once('Y').unwrap();
        // Coordinates are integer nanometres (6 decimal places, mm units).
        out.push(Flash {
            center: Vec2::new(
                x.parse::<f64>().unwrap() / 1_000_000.0,
                y.parse::<f64>().unwrap() / 1_000_000.0,
            ),
            aperture: ap.clone(),
        });
    }
    out
}
