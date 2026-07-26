//! Closed-form checks for prismatic members — the thin-wall answer the
//! lattice solver cannot give.
//!
//! The lattice fill in [`crate::mesh`] staircases the boundary at the
//! lattice pitch, so a 2 mm wall on a 312 mm member gets about one cell
//! through it at any resolution the MCP tier can afford. For a *prismatic*
//! member that is not a limitation to work around — it is the wrong
//! discretization. A closed section under torsion is priced exactly by
//! Bredt's theory, a slender member under bending by beam theory, and both
//! are cheaper *and* more accurate than any lattice FEA of the same part.
//!
//! This module is that answer, with the same fail-closed contract as the
//! lattice path:
//!
//! * Section properties ([`SectionProperties`]) come from exact integrals
//!   where one exists (round, round tube, rectangle bending) and from
//!   named, cited series or thin-wall theory where it does not (rectangle
//!   torsion — the convergent Saint-Venant series, not a table lookup;
//!   closed thin-wall torsion — Bredt). Every approximation states itself
//!   in [`SectionProperties::notes`].
//! * The check gates its own applicability ([`BeamVerdict`]): too stubby
//!   for beam theory, too thick-walled for Bredt, deflecting too far for
//!   small-displacement theory, or buckling before it yields — any of
//!   those makes the study `Unverifiable` and **no** claim is emitted, the
//!   same rule [`crate::convergence`] plays by.
//! * Claims ride `vcad.fea-claims/1` with basis `predicted`, so a receipt
//!   built from them rolls up Provisional, never Pass.
//!
//! Axis convention: the member runs along **X**; the cross-section lives
//! in the (Y, Z) plane. `width` is the Y extent, `height` the Z extent.
//! `i_y` is the second moment about Y (it resists bending that deflects
//! the member along Z). Units are the crate-wide mm-N-MPa system.

use serde::{Deserialize, Serialize};

use crate::spec::SpecError;

/// A prismatic cross-section.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Profile {
    /// Solid rectangle.
    Rect {
        /// Y extent, mm.
        width_mm: f64,
        /// Z extent, mm.
        height_mm: f64,
    },
    /// Rectangular tube of uniform wall thickness (outside dimensions).
    RectTube {
        /// Outside Y extent, mm.
        width_mm: f64,
        /// Outside Z extent, mm.
        height_mm: f64,
        /// Wall thickness, mm.
        wall_mm: f64,
    },
    /// Solid round bar.
    Round {
        /// Diameter, mm.
        diameter_mm: f64,
    },
    /// Round tube of uniform wall thickness (outside diameter).
    RoundTube {
        /// Outside diameter, mm.
        diameter_mm: f64,
        /// Wall thickness, mm.
        wall_mm: f64,
    },
    /// Doubly-symmetric I-section (an *open* section — see the torsion
    /// note it emits).
    IBeam {
        /// Flange width (Y extent), mm.
        width_mm: f64,
        /// Overall depth (Z extent), mm.
        height_mm: f64,
        /// Flange thickness, mm.
        flange_mm: f64,
        /// Web thickness, mm.
        web_mm: f64,
    },
}

/// Geometric properties of a [`Profile`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SectionProperties {
    /// Cross-sectional area, mm².
    pub area_mm2: f64,
    /// Second moment about Y (bending deflects along Z), mm⁴.
    pub i_y_mm4: f64,
    /// Second moment about Z (bending deflects along Y), mm⁴.
    pub i_z_mm4: f64,
    /// Elastic section modulus `I_y / c_z`, mm³.
    pub section_modulus_y_mm3: f64,
    /// Elastic section modulus `I_z / c_y`, mm³.
    pub section_modulus_z_mm3: f64,
    /// Saint-Venant torsion constant `J` (`T = G·J·θ/L`), mm⁴.
    pub torsion_constant_mm4: f64,
    /// Torsional section modulus `T / τ_max`, mm³.
    pub torsion_modulus_mm3: f64,
    /// Y extent of the section, mm.
    pub extent_y_mm: f64,
    /// Z extent of the section, mm.
    pub extent_z_mm: f64,
    /// Ratio of peak transverse shear stress to the mean `V/A`.
    pub shear_stress_factor: f64,
    /// Timoshenko shear coefficient κ (effective shear area `κ·A`).
    pub shear_coefficient: f64,
    /// Whether torsion is carried by a closed shear circuit. Open
    /// sections are torsionally soft and warping-dominated.
    pub closed_section: bool,
    /// Where each number came from, and what it approximates.
    pub notes: Vec<String>,
}

impl Profile {
    /// Validate the dimensions (positive, wall fits inside the section).
    pub fn validate(&self) -> Result<(), SpecError> {
        let bad = |m: String| Err(SpecError::Invalid(m));
        let pos = |v: f64| v.is_finite() && v > 0.0;
        match *self {
            Profile::Rect {
                width_mm,
                height_mm,
            } => {
                if !(pos(width_mm) && pos(height_mm)) {
                    return bad("rect needs positive width_mm and height_mm".into());
                }
            }
            Profile::RectTube {
                width_mm,
                height_mm,
                wall_mm,
            } => {
                if !(pos(width_mm) && pos(height_mm) && pos(wall_mm)) {
                    return bad("rect_tube needs positive width_mm, height_mm, wall_mm".into());
                }
                if 2.0 * wall_mm >= width_mm.min(height_mm) {
                    return bad(format!(
                        "rect_tube wall {wall_mm} mm leaves no bore in a \
                         {width_mm}x{height_mm} mm section — use a solid rect"
                    ));
                }
            }
            Profile::Round { diameter_mm } => {
                if !pos(diameter_mm) {
                    return bad("round needs a positive diameter_mm".into());
                }
            }
            Profile::RoundTube {
                diameter_mm,
                wall_mm,
            } => {
                if !(pos(diameter_mm) && pos(wall_mm)) {
                    return bad("round_tube needs positive diameter_mm and wall_mm".into());
                }
                if 2.0 * wall_mm >= diameter_mm {
                    return bad(format!(
                        "round_tube wall {wall_mm} mm leaves no bore in a \
                         {diameter_mm} mm tube — use a solid round"
                    ));
                }
            }
            Profile::IBeam {
                width_mm,
                height_mm,
                flange_mm,
                web_mm,
            } => {
                if !(pos(width_mm) && pos(height_mm) && pos(flange_mm) && pos(web_mm)) {
                    return bad(
                        "i_beam needs positive width_mm, height_mm, flange_mm, web_mm".into(),
                    );
                }
                if 2.0 * flange_mm >= height_mm {
                    return bad(format!(
                        "i_beam flanges ({flange_mm} mm each) consume the whole \
                         {height_mm} mm depth"
                    ));
                }
                if web_mm >= width_mm {
                    return bad("i_beam web is wider than its flange".into());
                }
            }
        }
        Ok(())
    }

    /// Compute the section properties.
    pub fn properties(&self) -> Result<SectionProperties, SpecError> {
        self.validate()?;
        Ok(match *self {
            Profile::Rect {
                width_mm: w,
                height_mm: h,
            } => {
                let (j, tor_mod, series_note) = rect_torsion(w, h);
                SectionProperties {
                    area_mm2: w * h,
                    i_y_mm4: w * h * h * h / 12.0,
                    i_z_mm4: h * w * w * w / 12.0,
                    section_modulus_y_mm3: w * h * h / 6.0,
                    section_modulus_z_mm3: h * w * w / 6.0,
                    torsion_constant_mm4: j,
                    torsion_modulus_mm3: tor_mod,
                    extent_y_mm: w,
                    extent_z_mm: h,
                    shear_stress_factor: 1.5,
                    shear_coefficient: 5.0 / 6.0,
                    closed_section: true,
                    notes: vec![
                        "bending properties are exact integrals of the rectangle".into(),
                        series_note,
                        "peak transverse shear 1.5·V/A at the neutral axis (exact parabolic \
                         distribution)"
                            .into(),
                    ],
                }
            }
            Profile::RectTube {
                width_mm: w,
                height_mm: h,
                wall_mm: t,
            } => {
                let (wi, hi) = (w - 2.0 * t, h - 2.0 * t);
                let area = w * h - wi * hi;
                // Bredt closed thin-wall torsion on the wall midline.
                let (wm, hm) = (w - t, h - t);
                let a_m = wm * hm;
                let s = 2.0 * (wm + hm);
                let j = 4.0 * a_m * a_m * t / s;
                let ratio = t / w.min(h);
                // Peak transverse shear rides the two webs (the walls
                // parallel to the load); V / (2·t·h_midline).
                let web_area = 2.0 * t * hm;
                SectionProperties {
                    area_mm2: area,
                    i_y_mm4: (w * h * h * h - wi * hi * hi * hi) / 12.0,
                    i_z_mm4: (h * w * w * w - hi * wi * wi * wi) / 12.0,
                    section_modulus_y_mm3: (w * h * h * h - wi * hi * hi * hi) / (6.0 * h),
                    section_modulus_z_mm3: (h * w * w * w - hi * wi * wi * wi) / (6.0 * w),
                    torsion_constant_mm4: j,
                    torsion_modulus_mm3: 2.0 * a_m * t,
                    extent_y_mm: w,
                    extent_z_mm: h,
                    shear_stress_factor: area / web_area,
                    shear_coefficient: web_area / area,
                    closed_section: true,
                    notes: vec![
                        "bending properties are exact (outer rectangle minus bore)".into(),
                        format!(
                            "torsion from Bredt closed thin-wall theory on the wall midline: \
                             J = 4·A_m²·t/s, tau = T/(2·A_m·t) with A_m = {a_m:.1} mm², \
                             s = {s:.1} mm; wall/section = {:.3} (theory assumes << 1, and \
                             ignores the corner-radius stress riser a real tube has)",
                            ratio
                        ),
                        "peak transverse shear taken as V/(2·t·h_midline) — carried by the two \
                         webs, uniform through the wall"
                            .into(),
                    ],
                }
            }
            Profile::Round { diameter_mm: d } => {
                let r = 0.5 * d;
                let i = std::f64::consts::PI * d * d * d * d / 64.0;
                SectionProperties {
                    area_mm2: std::f64::consts::PI * r * r,
                    i_y_mm4: i,
                    i_z_mm4: i,
                    section_modulus_y_mm3: i / r,
                    section_modulus_z_mm3: i / r,
                    torsion_constant_mm4: 2.0 * i,
                    torsion_modulus_mm3: 2.0 * i / r,
                    extent_y_mm: d,
                    extent_z_mm: d,
                    shear_stress_factor: 4.0 / 3.0,
                    shear_coefficient: 0.9,
                    closed_section: true,
                    notes: vec![
                        "every property is an exact integral — a circular section is the one \
                         case where Saint-Venant torsion has no approximation (J = 2·I)"
                            .into(),
                    ],
                }
            }
            Profile::RoundTube {
                diameter_mm: d,
                wall_mm: t,
            } => {
                let (r, di) = (0.5 * d, d - 2.0 * t);
                let i = std::f64::consts::PI * (d * d * d * d - di * di * di * di) / 64.0;
                let area = std::f64::consts::PI * (d * d - di * di) / 4.0;
                SectionProperties {
                    area_mm2: area,
                    i_y_mm4: i,
                    i_z_mm4: i,
                    section_modulus_y_mm3: i / r,
                    section_modulus_z_mm3: i / r,
                    torsion_constant_mm4: 2.0 * i,
                    torsion_modulus_mm3: 2.0 * i / r,
                    extent_y_mm: d,
                    extent_z_mm: d,
                    shear_stress_factor: 2.0,
                    shear_coefficient: 0.5,
                    closed_section: true,
                    notes: vec![
                        "exact for any wall thickness — the annulus needs no thin-wall \
                         approximation (J = 2·I, tau = T·r/J)"
                            .into(),
                        format!(
                            "d/t = {:.1}; local wall buckling and ovalization are NOT checked \
                             and govern thin tubes in bending",
                            d / t
                        ),
                    ],
                }
            }
            Profile::IBeam {
                width_mm: w,
                height_mm: h,
                flange_mm: tf,
                web_mm: tw,
            } => {
                let hw = h - 2.0 * tf;
                let area = 2.0 * w * tf + hw * tw;
                let i_y = (w * h * h * h - (w - tw) * hw * hw * hw) / 12.0;
                let i_z = (2.0 * tf * w * w * w + hw * tw * tw * tw) / 12.0;
                let j = (2.0 * w * tf * tf * tf + hw * tw * tw * tw) / 3.0;
                let t_max = tf.max(tw);
                let web_area = tw * hw;
                SectionProperties {
                    area_mm2: area,
                    i_y_mm4: i_y,
                    i_z_mm4: i_z,
                    section_modulus_y_mm3: i_y / (0.5 * h),
                    section_modulus_z_mm3: i_z / (0.5 * w),
                    torsion_constant_mm4: j,
                    torsion_modulus_mm3: j / t_max,
                    extent_y_mm: w,
                    extent_z_mm: h,
                    shear_stress_factor: area / web_area,
                    shear_coefficient: web_area / area,
                    closed_section: false,
                    notes: vec![
                        "bending properties are exact (three-rectangle decomposition, sharp \
                         web-flange junctions — a rolled section's fillets add a little area \
                         and stiffness)"
                            .into(),
                        "OPEN section: torsion is the Saint-Venant sum of thin-strip terms \
                         J = Σb·t³/3, which ignores warping restraint entirely. Real torsional \
                         stiffness is HIGHER when the ends cannot warp freely and the shear \
                         stress here is a rough estimate. An I-beam is a poor torsion member; \
                         if torque governs, use a closed section."
                            .into(),
                        "peak transverse shear taken as V/(t_web·h_web)".into(),
                    ],
                }
            }
        })
    }
}

/// Saint-Venant torsion of a solid rectangle, by the convergent Fourier
/// series (not a table interpolation). Returns `(J, T/τ_max, note)`.
///
/// For a rectangle `2a × 2b` with `a ≥ b`:
///
/// ```text
/// J   = (16/3)·a·b³·[1 − (192·b)/(π⁵·a)·Σ_{k odd} tanh(kπa/2b)/k⁵]
/// τ   = (2·T·b/J)·[1 − (8/π²)·Σ_{k odd} sech(kπa/2b)/k²]
/// ```
///
/// Both sums converge geometrically; 25 odd terms is machine precision.
/// Reproduces the classical square-bar constants `J = 0.1406·s⁴` and
/// `τ_max = T/(0.208·s³)`.
fn rect_torsion(w: f64, h: f64) -> (f64, f64, String) {
    let long = w.max(h);
    let short = w.min(h);
    let (a, b) = (0.5 * long, 0.5 * short);
    let arg = std::f64::consts::PI * a / (2.0 * b);
    let mut s_j = 0.0;
    let mut s_t = 0.0;
    for k in (1..=49).step_by(2) {
        let kf = k as f64;
        let x = kf * arg;
        // tanh saturates and sech underflows for large x; both are then
        // exactly their limits to within f64.
        let (tanh, sech) = if x > 350.0 {
            (1.0, 0.0)
        } else {
            (x.tanh(), 1.0 / x.cosh())
        };
        s_j += tanh / kf.powi(5);
        s_t += sech / (kf * kf);
    }
    let pi = std::f64::consts::PI;
    let j = (16.0 / 3.0) * a * b * b * b * (1.0 - (192.0 * b) / (pi.powi(5) * a) * s_j);
    let tau_coeff = 2.0 * b * (1.0 - 8.0 / (pi * pi) * s_t);
    let tor_mod = j / tau_coeff;
    (
        j,
        tor_mod,
        format!(
            "solid-rectangle torsion from the convergent Saint-Venant series (aspect \
             {:.2}): J = {j:.1} mm⁴, T/tau_max = {tor_mod:.1} mm³. Peak shear sits at the \
             midpoint of the long side. A solid rectangle is far softer in torsion than a \
             closed tube of the same area.",
            long / short
        ),
    )
}

/// How the member is held and loaded transversely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndCondition {
    /// Fixed at one end, transverse force applied at the free tip.
    CantileverTip,
    /// Fixed at one end, transverse force spread uniformly along the span.
    CantileverUniform,
    /// Pinned at both ends, transverse force at midspan.
    SimpleCenter,
    /// Pinned at both ends, transverse force spread uniformly.
    SimpleUniform,
    /// Built in at both ends, transverse force at midspan.
    FixedFixedCenter,
    /// Built in at both ends, transverse force spread uniformly.
    FixedFixedUniform,
}

impl EndCondition {
    /// `(max |M| / (F·L), δ·E·I / (F·L³), max |V| / F, shear-deflection
    /// coefficient, effective-length factor K)`.
    fn coefficients(self) -> (f64, f64, f64, f64, f64) {
        match self {
            EndCondition::CantileverTip => (1.0, 1.0 / 3.0, 1.0, 1.0, 2.0),
            EndCondition::CantileverUniform => (0.5, 0.125, 1.0, 0.5, 2.0),
            EndCondition::SimpleCenter => (0.25, 1.0 / 48.0, 0.5, 0.25, 1.0),
            EndCondition::SimpleUniform => (0.125, 5.0 / 384.0, 0.5, 0.125, 1.0),
            EndCondition::FixedFixedCenter => (0.125, 1.0 / 192.0, 0.5, 0.25, 0.5),
            EndCondition::FixedFixedUniform => (1.0 / 12.0, 1.0 / 384.0, 0.5, 0.125, 0.5),
        }
    }

    /// Human-readable description for provenance.
    fn label(self) -> &'static str {
        match self {
            EndCondition::CantileverTip => "cantilever, tip load",
            EndCondition::CantileverUniform => "cantilever, uniformly distributed load",
            EndCondition::SimpleCenter => "simply supported, center load",
            EndCondition::SimpleUniform => "simply supported, uniformly distributed load",
            EndCondition::FixedFixedCenter => "fixed-fixed, center load",
            EndCondition::FixedFixedUniform => "fixed-fixed, uniformly distributed load",
        }
    }
}

/// Which principal axis the transverse load bends the member about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BendAxis {
    /// Bending about Y — the member deflects along Z (uses `i_y`).
    Y,
    /// Bending about Z — the member deflects along Y (uses `i_z`).
    Z,
}

/// A load case on a prismatic member.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BeamCase {
    /// The cross-section.
    pub profile: Profile,
    /// Free span, mm.
    pub length_mm: f64,
    /// Support and load arrangement.
    pub end_condition: EndCondition,
    /// Total transverse force, N (magnitude; direction is the bend axis).
    #[serde(default)]
    pub transverse_force_n: f64,
    /// Which principal axis to bend about.
    #[serde(default = "default_bend_axis")]
    pub bend_axis: BendAxis,
    /// Applied torque about the member axis, N·mm.
    #[serde(default)]
    pub torque_nmm: f64,
    /// Axial force, N — positive tension, negative compression (a
    /// compressive force is also checked against Euler buckling).
    #[serde(default)]
    pub axial_force_n: f64,
    /// Young's modulus, MPa.
    #[serde(default = "default_youngs")]
    pub youngs_modulus_mpa: f64,
    /// Poisson's ratio, in `[0, 0.5)` — sets `G = E/(2(1+ν))`.
    #[serde(default = "default_poisson")]
    pub poisson: f64,
    /// Yield strength, MPa. When given, a safety factor is claimed.
    #[serde(default)]
    pub yield_strength_mpa: Option<f64>,
}

fn default_bend_axis() -> BendAxis {
    BendAxis::Y
}
fn default_youngs() -> f64 {
    69_000.0
}
fn default_poisson() -> f64 {
    0.33
}

impl BeamCase {
    /// Validate the load case (geometry, material, and "is anything even
    /// loaded" — a member with no load is not a passing check).
    pub fn validate(&self) -> Result<(), SpecError> {
        self.profile.validate()?;
        if !(self.length_mm.is_finite() && self.length_mm > 0.0) {
            return Err(SpecError::Invalid(format!(
                "length_mm must be positive, got {}",
                self.length_mm
            )));
        }
        for (name, v) in [
            ("transverse_force_n", self.transverse_force_n),
            ("torque_nmm", self.torque_nmm),
            ("axial_force_n", self.axial_force_n),
        ] {
            if !v.is_finite() {
                return Err(SpecError::Invalid(format!("{name} must be finite")));
            }
        }
        if self.transverse_force_n == 0.0 && self.torque_nmm == 0.0 && self.axial_force_n == 0.0 {
            return Err(SpecError::Invalid(
                "no load applied — set at least one of transverse_force_n, torque_nmm, \
                 axial_force_n (an unloaded member trivially 'passes', which is a lie)"
                    .into(),
            ));
        }
        if !self.youngs_modulus_mpa.is_finite() || self.youngs_modulus_mpa <= 0.0 {
            return Err(SpecError::Invalid(format!(
                "youngs_modulus_mpa must be positive, got {}",
                self.youngs_modulus_mpa
            )));
        }
        if !(0.0..0.5).contains(&self.poisson) {
            return Err(SpecError::Invalid(format!(
                "poisson must be in [0, 0.5), got {}",
                self.poisson
            )));
        }
        if let Some(y) = self.yield_strength_mpa {
            if !y.is_finite() || y <= 0.0 {
                return Err(SpecError::Invalid(format!(
                    "yield_strength_mpa must be positive, got {y}"
                )));
            }
        }
        Ok(())
    }
}

/// Applicability verdict for a closed-form check.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "verdict")]
pub enum BeamVerdict {
    /// Every assumption the closed forms rest on holds for this case.
    Applicable,
    /// At least one assumption fails; no QoI is claimed.
    Unverifiable {
        /// One reason per failed gate, each naming a route forward.
        reasons: Vec<String>,
    },
}

/// The result of a closed-form prismatic check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BeamCheck {
    /// Section properties of the profile.
    pub section: SectionProperties,
    /// Maximum bending moment, N·mm.
    pub max_moment_nmm: f64,
    /// Peak bending stress `M/S`, MPa.
    pub bending_stress_mpa: f64,
    /// Axial stress `P/A`, MPa (signed).
    pub axial_stress_mpa: f64,
    /// Worst-fiber normal stress magnitude, MPa.
    pub max_normal_stress_mpa: f64,
    /// Torsional shear stress, MPa.
    pub torsional_shear_mpa: f64,
    /// Peak transverse shear stress, MPa.
    pub transverse_shear_mpa: f64,
    /// Combined shear used in the von Mises check, MPa (conservative sum).
    pub max_shear_stress_mpa: f64,
    /// Von Mises stress `sqrt(σ² + 3τ²)`, MPa.
    pub max_von_mises_mpa: f64,
    /// Bending deflection, mm.
    pub bending_deflection_mm: f64,
    /// Shear deflection, mm.
    pub shear_deflection_mm: f64,
    /// Total transverse deflection, mm.
    pub max_deflection_mm: f64,
    /// Deflection as a fraction of the span (`1/300` reads as 0.00333).
    pub deflection_over_span: f64,
    /// Twist over the span, degrees.
    pub twist_deg: f64,
    /// Torsional stiffness `G·J/L`, N·mm per degree.
    pub torsional_stiffness_nmm_per_deg: f64,
    /// Slenderness `L / max(section extent)`.
    pub slenderness: f64,
    /// Euler critical load for the weak axis, N — only under compression.
    pub euler_buckling_load_n: Option<f64>,
    /// `P_cr / |P|` — only under compression.
    pub buckling_margin: Option<f64>,
    /// `yield / von Mises` — only when applicable and a yield was given.
    pub safety_factor: Option<f64>,
    /// The applicability verdict.
    pub verdict: BeamVerdict,
    /// Non-blocking cautions: assumptions that hold but are being leaned
    /// on. These travel into the claim provenance rather than gating.
    pub cautions: Vec<String>,
}

impl BeamCheck {
    /// True when every closed-form assumption held.
    pub fn applicable(&self) -> bool {
        self.verdict == BeamVerdict::Applicable
    }
}

/// Run the closed-form check.
///
/// Fail-closed: the returned [`BeamVerdict`] is `Unverifiable` — and
/// `safety_factor` is `None` — whenever an assumption behind the closed
/// forms does not hold for this case. The computed numbers are still
/// returned as diagnostics, exactly as the lattice path returns fields for
/// an unconverged study, but nothing downstream may claim them.
pub fn check_beam(case: &BeamCase) -> Result<BeamCheck, SpecError> {
    case.validate()?;
    let section = case.profile.properties()?;
    let l = case.length_mm;
    let (c_m, c_d, c_v, c_s, k_eff) = case.end_condition.coefficients();

    let (i, s_mod) = match case.bend_axis {
        BendAxis::Y => (section.i_y_mm4, section.section_modulus_y_mm3),
        BendAxis::Z => (section.i_z_mm4, section.section_modulus_z_mm3),
    };
    let f = case.transverse_force_n.abs();
    let e = case.youngs_modulus_mpa;
    let g = e / (2.0 * (1.0 + case.poisson));

    let max_moment_nmm = c_m * f * l;
    let bending_stress_mpa = max_moment_nmm / s_mod;
    let axial_stress_mpa = case.axial_force_n / section.area_mm2;
    // Worst fiber: the bending peak and the axial stress can share a sign,
    // so add magnitudes.
    let max_normal_stress_mpa = bending_stress_mpa.abs() + axial_stress_mpa.abs();

    let torsional_shear_mpa = case.torque_nmm.abs() / section.torsion_modulus_mm3;
    let shear_force = c_v * f;
    let transverse_shear_mpa = section.shear_stress_factor * shear_force / section.area_mm2;
    // Conservative: assume the two shears superpose at one point. They
    // generally peak at different points on the section.
    let max_shear_stress_mpa = torsional_shear_mpa + transverse_shear_mpa;
    let max_von_mises_mpa = (max_normal_stress_mpa * max_normal_stress_mpa
        + 3.0 * max_shear_stress_mpa * max_shear_stress_mpa)
        .sqrt();

    let bending_deflection_mm = c_d * f * l * l * l / (e * i);
    let shear_deflection_mm = c_s * f * l / (section.shear_coefficient * g * section.area_mm2);
    let max_deflection_mm = bending_deflection_mm + shear_deflection_mm;

    let twist_rad = case.torque_nmm.abs() * l / (g * section.torsion_constant_mm4);
    let twist_deg = twist_rad.to_degrees();
    let torsional_stiffness_nmm_per_deg =
        g * section.torsion_constant_mm4 / l * std::f64::consts::PI / 180.0;

    let max_extent = section.extent_y_mm.max(section.extent_z_mm);
    let slenderness = l / max_extent;

    let (euler_buckling_load_n, buckling_margin) = if case.axial_force_n < 0.0 {
        let i_min = section.i_y_mm4.min(section.i_z_mm4);
        let le = k_eff * l;
        let p_cr = std::f64::consts::PI * std::f64::consts::PI * e * i_min / (le * le);
        (Some(p_cr), Some(p_cr / case.axial_force_n.abs()))
    } else {
        (None, None)
    };

    // ── Fail-closed applicability gates ──────────────────────────────
    let mut reasons = Vec::new();
    let mut cautions = Vec::new();
    if f != 0.0 {
        if slenderness < 5.0 {
            reasons.push(format!(
                "L/depth = {slenderness:.1} — even with the Timoshenko shear term this is too \
                 stubby for beam theory: plane sections do not stay plane and the load path is \
                 three-dimensional. Use analyze_structure (a stubby part is exactly what the \
                 lattice CAN resolve — the pitch only defeats thin walls on a long member)."
            ));
        } else if slenderness < 10.0 {
            cautions.push(format!(
                "L/depth = {slenderness:.1} is short for beam theory; the shear-deflection term \
                 is included (Timoshenko) but expect a few percent error against a 3D solve"
            ));
        }
    }
    if let Profile::RectTube {
        width_mm,
        height_mm,
        wall_mm,
    } = case.profile
    {
        let ratio = wall_mm / width_mm.min(height_mm);
        if ratio > 0.2 && case.torque_nmm != 0.0 {
            reasons.push(format!(
                "wall/section = {ratio:.2} — Bredt thin-wall torsion assumes a thin wall \
                 (roughly <= 0.2); at this thickness the torsional shear is not uniform \
                 through the wall and J is overestimated. Model it as a solid rect minus the \
                 bore, or run analyze_structure (a thick wall is exactly the case the lattice \
                 CAN resolve)."
            ));
        }
    }
    if let Profile::RoundTube {
        diameter_mm,
        wall_mm,
    } = case.profile
    {
        if diameter_mm / wall_mm > 50.0 && f != 0.0 {
            cautions.push(format!(
                "d/t = {:.0}: local wall buckling and ovalization govern tubes this thin in \
                 bending, and neither is checked here",
                diameter_mm / wall_mm
            ));
        }
    }
    if case.axial_force_n < 0.0 {
        if let Some(margin) = buckling_margin {
            if (1.0..2.0).contains(&margin) {
                cautions.push(format!(
                    "Euler buckling margin is only {margin:.2} — the ideal-column formula has no \
                     allowance for initial crookedness or eccentricity, so treat anything under \
                     ~2 as unproven"
                ));
            }
        }
    }
    if max_deflection_mm > 0.1 * l {
        reasons.push(format!(
            "deflection {max_deflection_mm:.1} mm is {:.0}% of the {l:.0} mm span — \
             small-displacement theory no longer applies (the load geometry changes as it \
             deflects). This member is far too flexible; resize it and re-run.",
            100.0 * max_deflection_mm / l
        ));
    }
    if let (Some(p_cr), Some(margin)) = (euler_buckling_load_n, buckling_margin) {
        if margin < 1.0 {
            reasons.push(format!(
                "Euler critical load {p_cr:.0} N is below the applied compression {:.0} N \
                 (margin {margin:.2}) — this member buckles, and the strength numbers above \
                 do not describe how it fails. Increase the weak-axis I, shorten the span, or \
                 brace it.",
                case.axial_force_n.abs()
            ));
        }
    }
    if !section.closed_section && case.torque_nmm != 0.0 {
        reasons.push(
            "torque applied to an OPEN section — Saint-Venant J ignores warping restraint, so \
             the twist here is an upper bound and the shear estimate is rough. The closed form \
             will not be pinned down without a warping analysis; switch to a closed section, \
             or accept the twist as a bound and say so."
                .into(),
        );
    }

    let verdict = if reasons.is_empty() {
        BeamVerdict::Applicable
    } else {
        BeamVerdict::Unverifiable { reasons }
    };
    let safety_factor = match (&verdict, case.yield_strength_mpa) {
        (BeamVerdict::Applicable, Some(y)) if max_von_mises_mpa > 0.0 => {
            Some(y / max_von_mises_mpa)
        }
        _ => None,
    };

    Ok(BeamCheck {
        section,
        max_moment_nmm,
        bending_stress_mpa,
        axial_stress_mpa,
        max_normal_stress_mpa,
        torsional_shear_mpa,
        transverse_shear_mpa,
        max_shear_stress_mpa,
        max_von_mises_mpa,
        bending_deflection_mm,
        shear_deflection_mm,
        max_deflection_mm,
        deflection_over_span: max_deflection_mm / l,
        twist_deg,
        torsional_stiffness_nmm_per_deg,
        slenderness,
        euler_buckling_load_n,
        buckling_margin,
        safety_factor,
        verdict,
        cautions,
    })
}

/// Provenance for the closed-form route: which formulas ran, on what.
fn provenance_lines(check: &BeamCheck, case: &BeamCase) -> Vec<String> {
    let mut lines = vec![
        format!(
            "profile {:?}, span {:.1} mm, {} , bend about {:?}",
            case.profile,
            case.length_mm,
            case.end_condition.label(),
            case.bend_axis
        ),
        format!(
            "A {:.1} mm², I_y {:.4e} mm⁴, I_z {:.4e} mm⁴, J {:.4e} mm⁴, S_y {:.1} mm³, \
             T/tau {:.1} mm³",
            check.section.area_mm2,
            check.section.i_y_mm4,
            check.section.i_z_mm4,
            check.section.torsion_constant_mm4,
            check.section.section_modulus_y_mm3,
            check.section.torsion_modulus_mm3
        ),
        format!(
            "loads: transverse {:.1} N, torque {:.1} N·mm, axial {:.1} N",
            case.transverse_force_n, case.torque_nmm, case.axial_force_n
        ),
        format!(
            "material: E {} MPa, nu {}, G {:.0} MPa{}",
            case.youngs_modulus_mpa,
            case.poisson,
            case.youngs_modulus_mpa / (2.0 * (1.0 + case.poisson)),
            match case.yield_strength_mpa {
                Some(y) => format!(", yield {y} MPa"),
                None => String::new(),
            }
        ),
        format!(
            "L/depth {:.1}, deflection {:.4} mm ({:.4} of span, {:.0}% of it shear)",
            check.slenderness,
            check.max_deflection_mm,
            check.deflection_over_span,
            if check.max_deflection_mm > 0.0 {
                100.0 * check.shear_deflection_mm / check.max_deflection_mm
            } else {
                0.0
            }
        ),
    ];
    lines.extend(check.section.notes.iter().cloned());
    lines.extend(check.cautions.iter().map(|c| format!("caution: {c}")));
    lines
}

fn caveat() -> &'static str {
    "closed-form prismatic theory: linear-elastic, small-displacement, uniform section over \
     the whole span, idealized end conditions, no stress concentrations (holes, welds, corner \
     radii, tab cutouts), no local wall buckling or ovalization, no fatigue; bending and \
     torsional shear are superposed conservatively at a single point"
}

/// Predicted claims for an **applicable** check, on the same
/// `vcad.fea-claims/1` schema the lattice solver emits.
///
/// Refuses (fail-closed) when the verdict is `Unverifiable`.
pub fn predicted_claims(
    check: &BeamCheck,
    case: &BeamCase,
) -> Result<crate::receipt::ClaimSet, crate::receipt::ClaimError> {
    if let BeamVerdict::Unverifiable { reasons } = &check.verdict {
        return Err(crate::receipt::ClaimError::Unverifiable(reasons.clone()));
    }
    let cav = caveat();
    let mk = |name: &str, value: f64, unit: &str, note: String| crate::receipt::Claim {
        name: name.into(),
        value,
        unit: unit.into(),
        basis: "predicted".into(),
        note,
    };
    let mut claims = vec![
        mk(
            "max_von_mises_mpa",
            check.max_von_mises_mpa,
            "MPa",
            format!(
                "sqrt(sigma^2 + 3·tau^2) from normal {:.2} MPa (bending {:.2} + axial {:.2}) \
                 and shear {:.2} MPa (torsion {:.2} + transverse {:.2}); {cav}",
                check.max_normal_stress_mpa,
                check.bending_stress_mpa,
                check.axial_stress_mpa,
                check.max_shear_stress_mpa,
                check.torsional_shear_mpa,
                check.transverse_shear_mpa
            ),
        ),
        mk(
            "max_deflection_mm",
            check.max_deflection_mm,
            "mm",
            format!(
                "bending {:.4} mm + shear {:.4} mm, {} over a {:.0} mm span (1/{:.0} of it); \
                 {cav}",
                check.bending_deflection_mm,
                check.shear_deflection_mm,
                case.end_condition.label(),
                case.length_mm,
                if check.deflection_over_span > 0.0 {
                    1.0 / check.deflection_over_span
                } else {
                    f64::INFINITY
                }
            ),
        ),
    ];
    if case.torque_nmm != 0.0 {
        claims.push(mk(
            "twist_deg",
            check.twist_deg,
            "deg",
            format!(
                "T·L/(G·J) over the {:.0} mm span, J = {:.4e} mm⁴{}; {cav}",
                case.length_mm,
                check.section.torsion_constant_mm4,
                if check.section.closed_section {
                    " (closed shear circuit)"
                } else {
                    " (open section, warping ignored)"
                }
            ),
        ));
        claims.push(mk(
            "torsional_stiffness_nmm_per_deg",
            check.torsional_stiffness_nmm_per_deg,
            "N·mm/deg",
            format!("G·J/L; {cav}"),
        ));
    }
    if let (Some(p_cr), Some(margin)) = (check.euler_buckling_load_n, check.buckling_margin) {
        claims.push(mk(
            "euler_buckling_load_n",
            p_cr,
            "N",
            format!(
                "pi^2·E·I_min/(K·L)^2 with K = {:.1} for {}; margin over the applied \
                 compression is {margin:.2}. Euler only — no inelastic (Johnson) knockdown, \
                 no local or torsional buckling mode; {cav}",
                case.end_condition.coefficients().4,
                case.end_condition.label()
            ),
        ));
    }
    if let (Some(sf), Some(y)) = (check.safety_factor, case.yield_strength_mpa) {
        claims.push(mk(
            "safety_factor",
            sf,
            "1",
            format!(
                "yield {y} MPa / von Mises {:.2} MPa; against YIELD only — buckling, fatigue, \
                 and stress concentrations are separate checks; {cav}",
                check.max_von_mises_mpa
            ),
        ));
    }
    Ok(crate::receipt::ClaimSet {
        schema: crate::receipt::CLAIM_SCHEMA.to_string(),
        provenance: crate::receipt::SolverProvenance {
            levels: provenance_lines(check, case),
            material: format!("E {} MPa, nu {}", case.youngs_modulus_mpa, case.poisson),
            loads: vec![format!(
                "transverse {:.2} N ({}), torque {:.2} N·mm, axial {:.2} N",
                case.transverse_force_n,
                case.end_condition.label(),
                case.torque_nmm,
                case.axial_force_n
            )],
            supports: vec![case.end_condition.label().to_string()],
            // Closed forms have no discretization error — that is the
            // whole point of reaching for them here.
            displacement_change_rel: 0.0,
            stress_change_rel: 0.0,
        },
        claims,
    })
}

/// The oracle reference for the closed-form route.
pub fn oracle() -> vcad_receipt::OracleRef {
    vcad_receipt::OracleRef::new("vcad-kernel-fea/section", env!("CARGO_PKG_VERSION"))
}

/// Translate a closed-form claim set onto the unified receipt schema.
///
/// Claim ids are namespaced `structure.beam.*` so a receipt carrying both
/// routes never conflates a closed-form number with a lattice one. Basis
/// is `Predicted` — exact arithmetic on an idealized member is still not a
/// load test, so a receipt built from these rolls up Provisional.
pub fn design_claims(set: &crate::receipt::ClaimSet) -> Vec<vcad_receipt::ReceiptClaim> {
    let oracle = oracle();
    let provenance = set.provenance.levels.join(" | ");
    set.claims
        .iter()
        .map(|c| {
            vcad_receipt::ReceiptClaim::pass(
                format!("structure.beam.{}", c.name),
                crate::receipt::RECEIPT_DOMAIN,
                c.note.clone(),
                oracle.clone(),
            )
            .with_basis(vcad_receipt::ClaimBasis::Predicted)
            .with_measured(if c.unit == "1" {
                vcad_receipt::ClaimQuantity::bare(c.value)
            } else {
                vcad_receipt::ClaimQuantity::new(c.value, &c.unit)
            })
            .with_details(provenance.clone())
        })
        .collect()
}

/// The unified-receipt claim for an inapplicable check: one Unverifiable
/// claim carrying the reasons, so a receipt including it cannot pass.
pub fn design_claims_unverifiable(reasons: &[String]) -> Vec<vcad_receipt::ReceiptClaim> {
    vec![vcad_receipt::ReceiptClaim::unverifiable(
        "structure.beam.applicability",
        crate::receipt::RECEIPT_DOMAIN,
        "closed-form prismatic check applicability gates",
        oracle(),
        format!(
            "the closed forms do not apply to this case — no structural QoI is claimed: {}",
            reasons.join("; ")
        ),
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, rel: f64, what: &str) {
        let d = (a - b).abs() / b.abs().max(1e-300);
        assert!(d < rel, "{what}: {a} vs {b} (rel {d} >= {rel})");
    }

    #[test]
    fn square_bar_torsion_matches_classical_constants() {
        // The textbook square-bar results: J = 0.1406·s⁴ and
        // tau_max = T/(0.208·s³). Our series must reproduce both.
        let s = 20.0;
        let p = Profile::Rect {
            width_mm: s,
            height_mm: s,
        }
        .properties()
        .unwrap();
        approx(p.torsion_constant_mm4, 0.1406 * s.powi(4), 1e-3, "square J");
        approx(
            p.torsion_modulus_mm3,
            0.208 * s.powi(3),
            2e-3,
            "square T/tau",
        );
        // A 2:1 rectangle: Roark gives J = 0.229·a·b³ and
        // tau_max = T/(0.246·a·b²) for a/b = 2 (a long, b short).
        let (a, b) = (40.0, 20.0);
        let p = Profile::Rect {
            width_mm: a,
            height_mm: b,
        }
        .properties()
        .unwrap();
        approx(p.torsion_constant_mm4, 0.229 * a * b.powi(3), 5e-3, "2:1 J");
        approx(p.torsion_modulus_mm3, 0.246 * a * b * b, 5e-3, "2:1 T/tau");
    }

    #[test]
    fn round_and_round_tube_properties_are_exact() {
        let d = 30.0;
        let p = Profile::Round { diameter_mm: d }.properties().unwrap();
        approx(
            p.area_mm2,
            std::f64::consts::PI * d * d / 4.0,
            1e-12,
            "round A",
        );
        approx(
            p.i_y_mm4,
            std::f64::consts::PI * d.powi(4) / 64.0,
            1e-12,
            "round I",
        );
        // J = 2I exactly for a circle.
        approx(p.torsion_constant_mm4, 2.0 * p.i_y_mm4, 1e-12, "round J");
        let t = 2.0;
        let q = Profile::RoundTube {
            diameter_mm: d,
            wall_mm: t,
        }
        .properties()
        .unwrap();
        let di = d - 2.0 * t;
        approx(
            q.i_y_mm4,
            std::f64::consts::PI * (d.powi(4) - di.powi(4)) / 64.0,
            1e-12,
            "tube I",
        );
        // The whole point of a tube: 44% of the solid bar's bending
        // stiffness for 25% of its material.
        assert!(q.area_mm2 < 0.26 * p.area_mm2, "area ratio");
        assert!(q.i_y_mm4 > 0.43 * p.i_y_mm4, "stiffness ratio");
    }

    #[test]
    fn bredt_torsion_of_a_thin_square_tube() {
        // 40x40x2 tube: midline 38x38, A_m = 1444 mm², s = 152 mm.
        // J = 4·A_m²·t/s = 4·1444²·2/152 = 109 754 mm⁴ (hand arithmetic).
        // tau = T/(2·A_m·t) -> T/tau = 5776 mm³.
        let p = Profile::RectTube {
            width_mm: 40.0,
            height_mm: 40.0,
            wall_mm: 2.0,
        }
        .properties()
        .unwrap();
        approx(p.torsion_constant_mm4, 109_744.0, 1e-12, "Bredt J");
        approx(p.torsion_modulus_mm3, 5776.0, 1e-9, "Bredt T/tau");
        assert!(p.closed_section);
        // The closed tube is enormously stiffer in torsion than the solid
        // rectangle of the same *area* — the reason the tube is the right
        // section for a torsion member.
        let solid = Profile::Rect {
            width_mm: 40.0,
            height_mm: p.area_mm2 / 40.0,
        }
        .properties()
        .unwrap();
        assert!(
            p.torsion_constant_mm4 > 20.0 * solid.torsion_constant_mm4,
            "tube J {} vs equal-area strip J {}",
            p.torsion_constant_mm4,
            solid.torsion_constant_mm4
        );
    }

    #[test]
    fn cantilever_matches_the_lattice_solvers_own_reference_case() {
        // The same 80x10x10 aluminum cantilever with a 100 N tip load that
        // convergence.rs validates the lattice against: Timoshenko
        // delta = FL^3/(3EI) + FL/(kappa·G·A) ~ 0.297 + 0.004 = 0.301 mm,
        // root bending sigma = Mc/I = 48 MPa.
        let case = BeamCase {
            profile: Profile::Rect {
                width_mm: 10.0,
                height_mm: 10.0,
            },
            length_mm: 80.0,
            end_condition: EndCondition::CantileverTip,
            transverse_force_n: 100.0,
            bend_axis: BendAxis::Y,
            torque_nmm: 0.0,
            axial_force_n: 0.0,
            youngs_modulus_mpa: 69_000.0,
            poisson: 0.33,
            yield_strength_mpa: Some(276.0),
        };
        let out = check_beam(&case).unwrap();
        assert!(out.applicable(), "verdict {:?}", out.verdict);
        approx(out.max_deflection_mm, 0.301, 0.02, "tip deflection");
        approx(out.bending_stress_mpa, 48.0, 0.01, "root bending stress");
        // Shear is a small but real share of the deflection.
        assert!(out.shear_deflection_mm > 0.0);
        assert!(out.shear_deflection_mm < 0.05 * out.bending_deflection_mm);
        let sf = out.safety_factor.expect("safety factor");
        assert!(sf > 4.0 && sf < 6.0, "sf {sf}");
    }

    #[test]
    fn torsion_tube_sizing_reproduces_the_hand_calculation() {
        // The case that motivated this module: a 312 mm torsion tube in a
        // robot chassis, 40x40x2 aluminum, carrying 40 N·m.
        let case = BeamCase {
            profile: Profile::RectTube {
                width_mm: 40.0,
                height_mm: 40.0,
                wall_mm: 2.0,
            },
            length_mm: 312.0,
            end_condition: EndCondition::CantileverTip,
            transverse_force_n: 0.0,
            bend_axis: BendAxis::Y,
            torque_nmm: 40_000.0,
            axial_force_n: 0.0,
            youngs_modulus_mpa: 69_000.0,
            poisson: 0.33,
            yield_strength_mpa: Some(276.0),
        };
        let out = check_beam(&case).unwrap();
        assert!(out.applicable(), "verdict {:?}", out.verdict);
        // tau = T/(2·A_m·t) = 40000/5776 = 6.92 MPa.
        approx(out.torsional_shear_mpa, 6.925, 1e-3, "Bredt shear");
        // theta = T·L/(G·J), G = 69000/2.66 = 25940 MPa.
        let g = 69_000.0 / (2.0 * 1.33);
        let expect = (40_000.0 * 312.0 / (g * out.section.torsion_constant_mm4)).to_degrees();
        approx(out.twist_deg, expect, 1e-12, "twist");
        assert!(out.twist_deg < 0.3, "twist {} deg", out.twist_deg);
        // No transverse load, so shear is torsion alone and von Mises is
        // sqrt(3)·tau.
        approx(
            out.max_von_mises_mpa,
            3.0_f64.sqrt() * out.torsional_shear_mpa,
            1e-12,
            "pure-torsion von Mises",
        );
        let sf = out.safety_factor.expect("safety factor");
        assert!(sf > 20.0, "a 2 mm wall is comfortable at 40 N·m: sf {sf}");
    }

    #[test]
    fn stubby_member_fails_closed_and_points_at_the_lattice() {
        let case = BeamCase {
            profile: Profile::Rect {
                width_mm: 30.0,
                height_mm: 30.0,
            },
            length_mm: 60.0, // L/depth = 2
            end_condition: EndCondition::CantileverTip,
            transverse_force_n: 500.0,
            bend_axis: BendAxis::Y,
            torque_nmm: 0.0,
            axial_force_n: 0.0,
            youngs_modulus_mpa: 69_000.0,
            poisson: 0.33,
            yield_strength_mpa: Some(276.0),
        };
        let out = check_beam(&case).unwrap();
        let reasons = match &out.verdict {
            BeamVerdict::Unverifiable { reasons } => reasons.clone(),
            v => panic!("expected Unverifiable, got {v:?}"),
        };
        assert!(reasons.iter().any(|r| r.contains("analyze_structure")));
        assert_eq!(out.safety_factor, None, "no claim without applicability");
        assert!(matches!(
            predicted_claims(&out, &case),
            Err(crate::receipt::ClaimError::Unverifiable(_))
        ));
    }

    #[test]
    fn compression_that_buckles_is_unverifiable_not_safe() {
        // A slender 2 mm x 20 mm aluminum strap, 600 mm long, pushed with
        // 400 N. Yield says it is fine; Euler says it folds.
        let case = BeamCase {
            profile: Profile::Rect {
                width_mm: 2.0,
                height_mm: 20.0,
            },
            length_mm: 600.0,
            end_condition: EndCondition::SimpleCenter,
            transverse_force_n: 0.0,
            bend_axis: BendAxis::Y,
            torque_nmm: 0.0,
            axial_force_n: -400.0,
            youngs_modulus_mpa: 69_000.0,
            poisson: 0.33,
            yield_strength_mpa: Some(276.0),
        };
        let out = check_beam(&case).unwrap();
        // Pure yield check would look comfortable: 400/40 = 10 MPa.
        approx(out.max_normal_stress_mpa, 10.0, 1e-12, "axial stress");
        let margin = out.buckling_margin.expect("margin");
        assert!(margin < 1.0, "margin {margin}");
        match &out.verdict {
            BeamVerdict::Unverifiable { reasons } => {
                assert!(reasons.iter().any(|r| r.contains("buckles")));
            }
            v => panic!("expected Unverifiable, got {v:?}"),
        }
        assert_eq!(out.safety_factor, None);
    }

    #[test]
    fn open_section_torsion_is_flagged_and_claims_nothing() {
        let case = BeamCase {
            profile: Profile::IBeam {
                width_mm: 40.0,
                height_mm: 60.0,
                flange_mm: 4.0,
                web_mm: 3.0,
            },
            length_mm: 800.0,
            end_condition: EndCondition::CantileverTip,
            transverse_force_n: 0.0,
            bend_axis: BendAxis::Y,
            torque_nmm: 20_000.0,
            axial_force_n: 0.0,
            youngs_modulus_mpa: 200_000.0,
            poisson: 0.3,
            yield_strength_mpa: Some(250.0),
        };
        let out = check_beam(&case).unwrap();
        assert!(!out.section.closed_section);
        match &out.verdict {
            BeamVerdict::Unverifiable { reasons } => {
                assert!(reasons.iter().any(|r| r.contains("warping")));
            }
            v => panic!("expected Unverifiable, got {v:?}"),
        }
        // And a closed tube of comparable size is orders stiffer.
        let tube = Profile::RectTube {
            width_mm: 40.0,
            height_mm: 60.0,
            wall_mm: 3.0,
        }
        .properties()
        .unwrap();
        assert!(tube.torsion_constant_mm4 > 20.0 * out.section.torsion_constant_mm4);
    }

    #[test]
    fn validation_is_fail_closed() {
        let ok = BeamCase {
            profile: Profile::Round { diameter_mm: 10.0 },
            length_mm: 200.0,
            end_condition: EndCondition::CantileverTip,
            transverse_force_n: 10.0,
            bend_axis: BendAxis::Y,
            torque_nmm: 0.0,
            axial_force_n: 0.0,
            youngs_modulus_mpa: 69_000.0,
            poisson: 0.33,
            yield_strength_mpa: None,
        };
        assert!(check_beam(&ok).is_ok());
        let unloaded = BeamCase {
            transverse_force_n: 0.0,
            ..ok.clone()
        };
        assert!(
            check_beam(&unloaded).is_err(),
            "an unloaded member is an error"
        );
        let bad_len = BeamCase {
            length_mm: 0.0,
            ..ok.clone()
        };
        assert!(check_beam(&bad_len).is_err());
        let bad_wall = BeamCase {
            profile: Profile::RoundTube {
                diameter_mm: 10.0,
                wall_mm: 5.0,
            },
            ..ok.clone()
        };
        assert!(check_beam(&bad_wall).is_err(), "wall eats the whole bore");
        let bad_nu = BeamCase { poisson: 0.5, ..ok };
        assert!(check_beam(&bad_nu).is_err());
    }

    #[test]
    fn claims_ride_the_receipt_as_provisional() {
        let case = BeamCase {
            profile: Profile::RectTube {
                width_mm: 40.0,
                height_mm: 40.0,
                wall_mm: 2.0,
            },
            length_mm: 312.0,
            end_condition: EndCondition::SimpleCenter,
            transverse_force_n: 200.0,
            bend_axis: BendAxis::Y,
            torque_nmm: 40_000.0,
            axial_force_n: 0.0,
            youngs_modulus_mpa: 69_000.0,
            poisson: 0.33,
            yield_strength_mpa: Some(276.0),
        };
        let out = check_beam(&case).unwrap();
        assert!(out.applicable(), "verdict {:?}", out.verdict);
        let set = predicted_claims(&out, &case).unwrap();
        assert_eq!(set.schema, crate::receipt::CLAIM_SCHEMA);
        let names: Vec<&str> = set.claims.iter().map(|c| c.name.as_str()).collect();
        for want in [
            "max_von_mises_mpa",
            "max_deflection_mm",
            "twist_deg",
            "torsional_stiffness_nmm_per_deg",
            "safety_factor",
        ] {
            assert!(names.contains(&want), "missing claim {want} in {names:?}");
        }
        for c in &set.claims {
            assert_eq!(c.basis, "predicted");
            assert!(c.value.is_finite(), "non-finite {}", c.name);
            assert!(c.note.contains("closed-form"), "note: {}", c.note);
        }
        let claims = design_claims(&set);
        for c in &claims {
            assert!(c.id.starts_with("structure.beam."), "id {}", c.id);
            assert_eq!(c.basis, Some(vcad_receipt::ClaimBasis::Predicted));
        }
        let receipt = vcad_receipt::DesignReceipt::with_claims(claims);
        assert_eq!(
            receipt.verdict(),
            vcad_receipt::ReceiptVerdict::Provisional,
            "closed-form arithmetic is still not a load test"
        );
        // And the inapplicable path poisons a receipt.
        let poisoned = vcad_receipt::DesignReceipt::with_claims(design_claims_unverifiable(&[
            "stub reason".into(),
        ]));
        assert_ne!(poisoned.verdict(), vcad_receipt::ReceiptVerdict::Pass);
    }

    #[test]
    fn case_round_trips_json_with_defaults() {
        let case: BeamCase = serde_json::from_str(
            r#"{"profile":{"type":"rect_tube","width_mm":40,"height_mm":40,"wall_mm":2},
                "length_mm":312,"end_condition":"cantilever_tip","torque_nmm":40000}"#,
        )
        .unwrap();
        assert_eq!(case.bend_axis, BendAxis::Y);
        assert_eq!(case.youngs_modulus_mpa, 69_000.0);
        assert_eq!(case.transverse_force_n, 0.0);
        let out = check_beam(&case).unwrap();
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("Bredt"));
        let back: BeamCheck = serde_json::from_str(&json).unwrap();
        assert_eq!(back.verdict, out.verdict);
    }
}
