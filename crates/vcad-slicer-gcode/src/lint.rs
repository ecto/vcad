//! G-code linter: a fail-closed safety pass run before anything uploads.
//!
//! It interprets the subset of G-code the slicer emits (G0/G1/G28/G29/G90/
//! G91/G92, M82/M83, M104/M109/M140/M190, M106/M107, M73, M84, M400) and
//! flags anything that could damage the printer, the plate or the print:
//!
//! - motion before homing;
//! - homing (`G28`) with the nozzle target at or above the ooze temperature
//!   (the P1S homes Z by touching the bed with the nozzle);
//! - X/Y/Z outside the build volume, or Z below zero after homing;
//! - extrusion while the nozzle target is below the minimum extrude
//!   temperature, or before any wait-for-temperature (`M109`);
//! - temperatures above the configured limits;
//! - heaters still on at the end of the file;
//! - unknown commands (anything not in the subset is reported, not ignored).
//!
//! It also estimates print time per layer for `M73` progress lines.

use crate::printer::PrinterProfile;

/// Limits the linter enforces.
#[derive(Debug, Clone)]
pub struct LintLimits {
    /// Build volume (mm).
    pub bed_x: f64,
    /// Build volume (mm).
    pub bed_y: f64,
    /// Build volume (mm).
    pub bed_z: f64,
    /// Nozzle may not home at or above this (°C).
    pub max_homing_nozzle_temp: f64,
    /// No extrusion below this nozzle target (°C).
    pub min_extrude_temp: f64,
    /// Hard nozzle ceiling (°C).
    pub max_nozzle_temp: f64,
    /// Hard bed ceiling (°C).
    pub max_bed_temp: f64,
}

impl LintLimits {
    /// Limits for a printer profile. Temperature ceilings are the P1S
    /// hardware limits (300 °C hotend, 110 °C bed) and apply to every
    /// profile until profiles carry their own.
    pub fn for_printer(p: &PrinterProfile) -> Self {
        Self {
            bed_x: p.bed_x,
            bed_y: p.bed_y,
            bed_z: p.bed_z,
            max_homing_nozzle_temp: 170.0,
            min_extrude_temp: 170.0,
            max_nozzle_temp: 300.0,
            max_bed_temp: 110.0,
        }
    }
}

/// One problem.
#[derive(Debug, Clone, PartialEq)]
pub struct LintFinding {
    /// 1-based line number.
    pub line: usize,
    /// What is wrong.
    pub message: String,
}

/// Linter result.
#[derive(Debug, Clone, Default)]
pub struct LintReport {
    /// Every problem found; empty means clean.
    pub findings: Vec<LintFinding>,
    /// Estimated seconds at the start of each `; CHANGE_LAYER` marker.
    pub layer_start_s: Vec<f64>,
    /// Estimated total seconds (moves only; heating not counted).
    pub total_s: f64,
}

impl LintReport {
    /// True when nothing was found.
    pub fn ok(&self) -> bool {
        self.findings.is_empty()
    }
}

fn word(code: &str, letter: char) -> Option<f64> {
    code.split_whitespace()
        .find(|w| w.starts_with(letter) && w.len() > 1)
        .and_then(|w| w[1..].parse().ok())
}

fn push(r: &mut LintReport, line: usize, message: String) {
    r.findings.push(LintFinding { line, message });
}

/// Lint a G-code program.
pub fn lint(gcode: &str, lim: &LintLimits) -> LintReport {
    let mut r = LintReport::default();
    let mut homed = false;
    let mut waited = false;
    let mut abs_xyz = true;
    let mut abs_e = true;
    let (mut x, mut y, mut z, mut e) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let mut feed = 1500.0f64; // mm/min
    let mut nozzle = 0.0f64;
    let mut bed = 0.0f64;
    let mut t = 0.0f64;

    for (i, raw) in gcode.lines().enumerate() {
        let n = i + 1;
        if raw.trim_start().starts_with("; CHANGE_LAYER") {
            r.layer_start_s.push(t);
        }
        let code = raw.split(';').next().unwrap_or("").trim();
        if code.is_empty() {
            continue;
        }
        let cmd = code.split_whitespace().next().unwrap_or("");
        match cmd {
            "G0" | "G1" => {
                if !homed {
                    push(&mut r, n, format!("motion before homing: `{code}`"));
                }
                let nx = word(code, 'X').map(|v| if abs_xyz { v } else { x + v });
                let ny = word(code, 'Y').map(|v| if abs_xyz { v } else { y + v });
                let nz = word(code, 'Z').map(|v| if abs_xyz { v } else { z + v });
                if let Some(f) = word(code, 'F') {
                    if f <= 0.0 {
                        push(&mut r, n, format!("non-positive feedrate: `{code}`"));
                    } else {
                        feed = f;
                    }
                }
                let (tx, ty, tz) = (nx.unwrap_or(x), ny.unwrap_or(y), nz.unwrap_or(z));
                if !(0.0..=lim.bed_x).contains(&tx) || !(0.0..=lim.bed_y).contains(&ty) {
                    push(
                        &mut r,
                        n,
                        format!("XY ({tx:.2}, {ty:.2}) outside the bed: `{code}`"),
                    );
                }
                if tz < 0.0 || tz > lim.bed_z {
                    push(
                        &mut r,
                        n,
                        format!("Z {tz:.3} outside 0..{}: `{code}`", lim.bed_z),
                    );
                }
                if let Some(ev) = word(code, 'E') {
                    let de = if abs_e { ev - e } else { ev };
                    e = if abs_e { ev } else { e + ev };
                    if de > 1e-9 {
                        if nozzle < lim.min_extrude_temp {
                            push(
                                &mut r,
                                n,
                                format!(
                                    "extrusion with nozzle target {nozzle:.0} °C (cold): `{code}`"
                                ),
                            );
                        } else if !waited {
                            push(
                                &mut r,
                                n,
                                format!("extrusion before any M109 wait: `{code}`"),
                            );
                        }
                    }
                }
                let d = ((tx - x).powi(2) + (ty - y).powi(2) + (tz - z).powi(2)).sqrt();
                t += d / (feed / 60.0);
                (x, y, z) = (tx, ty, tz);
            }
            "G28" => {
                if nozzle >= lim.max_homing_nozzle_temp {
                    push(&mut r, n, format!(
                        "homing with nozzle target {nozzle:.0} °C (>= {:.0}): the nozzle touches the bed and oozes",
                        lim.max_homing_nozzle_temp
                    ));
                }
                homed = true;
            }
            "G29" => {
                if !homed {
                    push(&mut r, n, "bed leveling before homing".into());
                }
            }
            "G90" => abs_xyz = true,
            "G91" => abs_xyz = false,
            "M82" => abs_e = true,
            "M83" => abs_e = false,
            "G92" => {
                if let Some(v) = word(code, 'E') {
                    e = v;
                }
                if word(code, 'X').is_some()
                    || word(code, 'Y').is_some()
                    || word(code, 'Z').is_some()
                {
                    push(&mut r, n, format!("G92 on an axis other than E: `{code}`"));
                }
            }
            "M104" | "M109" => {
                let s = word(code, 'S').unwrap_or(0.0);
                if s > lim.max_nozzle_temp {
                    push(
                        &mut r,
                        n,
                        format!("nozzle {s:.0} °C above {:.0}", lim.max_nozzle_temp),
                    );
                }
                nozzle = s;
                if cmd == "M109" && s >= lim.min_extrude_temp {
                    waited = true;
                }
            }
            "M140" | "M190" => {
                let s = word(code, 'S').unwrap_or(0.0);
                if s > lim.max_bed_temp {
                    push(
                        &mut r,
                        n,
                        format!("bed {s:.0} °C above {:.0}", lim.max_bed_temp),
                    );
                }
                bed = s;
            }
            "M106" | "M107" | "M73" | "M84" | "M400" | "M17" => {}
            other => push(
                &mut r,
                n,
                format!("unknown command `{other}` (not in the linted subset)"),
            ),
        }
    }
    if nozzle > 0.0 || bed > 0.0 {
        push(
            &mut r,
            gcode.lines().count(),
            format!("heaters still on at end of file (nozzle {nozzle:.0}, bed {bed:.0})"),
        );
    }
    r.total_s = t;
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lim() -> LintLimits {
        LintLimits::for_printer(&PrinterProfile::bambu_p1s())
    }

    const GOOD: &str = "M140 S55\nM104 S140\nG28\nM190 S55\nG1 Z5 F600\nM109 S220\nG92 E0\nG1 X10 Y10 Z0.2 E1 F1200\nM104 S0\nM140 S0\n";

    #[test]
    fn clean_program_passes() {
        let r = lint(GOOD, &lim());
        assert!(r.ok(), "{:?}", r.findings);
        assert!(r.total_s > 0.0);
    }

    fn fails(g: &str, why: &str) {
        let r = lint(g, &lim());
        assert!(
            r.findings.iter().any(|f| f.message.contains(why)),
            "expected `{why}` in {:?}",
            r.findings
        );
    }

    #[test]
    fn catches_each_hazard() {
        // The old Bambu start sequence: heat to print temp, then home.
        fails("M109 S220\nG28\nM104 S0\n", "homing with nozzle target 220");
        fails("G1 X10\n", "motion before homing");
        fails("G28\nG1 X300 Y10\n", "outside the bed");
        fails("G28\nG1 Z-0.1\n", "Z -0.100");
        fails("G28\nG91\nG1 Z-1\n", "Z -1.000");
        fails("G28\nG1 X10 E1\n", "cold");
        fails("M104 S220\nG28\nG1 X10 E1\nM104 S0\n", "before any M109");
        fails("M104 S320\n", "above 300");
        fails("M140 S120\n", "above 110");
        fails("M104 S200\n", "heaters still on");
        fails(
            "G28\nM1002 gcode_claim_action : 2\n",
            "unknown command `M1002`",
        );
        fails("G92 Z0\n", "other than E");
    }

    #[test]
    fn retraction_and_relative_e_are_not_extrusion() {
        let g = "M104 S140\nG28\nM109 S220\nG1 E-0.8 F1800\nM83\nG1 X10 E-0.5\nM104 S0\n";
        assert!(lint(g, &lim()).ok());
    }

    #[test]
    fn layer_markers_get_timestamps() {
        let g = "M104 S140\nG28\nM109 S200\n; CHANGE_LAYER\nG1 X60 F6000\n; CHANGE_LAYER\nG1 X0\nM104 S0\n";
        let r = lint(g, &lim());
        assert_eq!(r.layer_start_s.len(), 2);
        assert!(r.layer_start_s[1] > r.layer_start_s[0]);
    }
}
