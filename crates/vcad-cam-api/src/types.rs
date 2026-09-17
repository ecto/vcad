//! Shared request pieces and the validation every CAM entry point runs first.
//!
//! The rule the whole surface follows: a number that is not usable is refused
//! *by name*, before any geometry is built. "Invalid input" sends a machinist
//! back to guess which of thirty fields was wrong; "stepdown must be greater
//! than zero, not 0" does not.

use serde::Deserialize;

use crate::placement::Placement;
use vcad_kernel_cam::materials::{MachineClass, Spindle};
use vcad_kernel_cam::verify2d::{PartRegion, TravelLimits};
use vcad_kernel_cam::{
    CamSettings, Contour, MachineLimits, Point2D, Spoilboard, Stock, Tool, ToolEntry, ToolGeometry,
    ToolHolder,
};

// ---------------------------------------------------------------------------
// Number and geometry validation
// ---------------------------------------------------------------------------

/// A number that has to be usable arithmetic.
pub fn finite(what: &str, v: f64) -> Result<f64, String> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(format!("{what} must be a finite number, not {v}."))
    }
}

/// A number that has to be usable arithmetic and strictly above zero.
pub fn positive(what: &str, v: f64) -> Result<f64, String> {
    let v = finite(what, v)?;
    if v > 0.0 {
        Ok(v)
    } else {
        Err(format!("{what} must be greater than zero, not {v}."))
    }
}

/// A number that has to be usable arithmetic and not below zero.
pub fn non_negative(what: &str, v: f64) -> Result<f64, String> {
    let v = finite(what, v)?;
    if v >= 0.0 {
        Ok(v)
    } else {
        Err(format!("{what} must not be negative, not {v}."))
    }
}

/// A closed polyline: at least three finite points, duplicate closing point
/// dropped. The stock frame is not enforced here — an outline that sits where
/// the part was modelled is a placement problem the caller is told about by
/// the envelope check, not a parse error.
pub fn loop_points(what: &str, points: &[[f64; 2]]) -> Result<Vec<[f64; 2]>, String> {
    for (i, p) in points.iter().enumerate() {
        if !p[0].is_finite() || !p[1].is_finite() {
            return Err(format!(
                "{what}: point {i} is ({}, {}); every coordinate has to be a finite number.",
                p[0], p[1]
            ));
        }
    }
    let mut out = points.to_vec();
    // A DXF-shaped loop repeats its first point; the kernel's loops do not.
    while out.len() > 1 {
        let (first, last) = (out[0], out[out.len() - 1]);
        if (first[0] - last[0]).hypot(first[1] - last[1]) <= 1e-9 {
            out.pop();
        } else {
            break;
        }
    }
    if out.len() < 3 {
        return Err(format!(
            "{what} needs at least three distinct points, and this one has {}.",
            out.len()
        ));
    }
    Ok(out)
}

/// A closed polyline as the kernel's [`Contour`].
pub fn contour_from(what: &str, points: &[[f64; 2]]) -> Result<Contour, String> {
    let pts = loop_points(what, points)?;
    let mut c = Contour::new(Point2D::new(pts[0][0], pts[0][1]));
    for p in pts.iter().skip(1) {
        c.line_to(Point2D::new(p[0], p[1]));
    }
    c.line_to(Point2D::new(pts[0][0], pts[0][1]));
    Ok(c)
}

// ---------------------------------------------------------------------------
// Stock
// ---------------------------------------------------------------------------

/// `stock { thickness, margin | bbox, spoilboard? }`.
#[derive(Debug, Clone, Deserialize)]
pub struct StockReq {
    /// Thickness of the material, mm. The underside sits at `-thickness`.
    pub thickness: f64,
    /// How far the blank stands proud of the part when `bbox` is not given.
    #[serde(default)]
    pub margin: Option<f64>,
    /// Explicit blank outline `[min_x, min_y, max_x, max_y]`.
    #[serde(default)]
    pub bbox: Option<[f64; 4]>,
    /// Sacrificial board under the stock, mm. Required before any cut may go
    /// past the underside.
    #[serde(default)]
    pub spoilboard: Option<f64>,
}

impl StockReq {
    /// Validate and convert.
    pub fn build(&self) -> Result<Stock, String> {
        let mut stock = Stock::new(positive("stock.thickness", self.thickness)?);
        if let Some(m) = self.margin {
            stock = stock.with_margin(non_negative("stock.margin", m)?);
        }
        if let Some(b) = self.bbox {
            for (i, v) in b.iter().enumerate() {
                finite(&format!("stock.bbox[{i}]"), *v)?;
            }
            if b[2] <= b[0] || b[3] <= b[1] {
                return Err(format!(
                    "stock.bbox is [{}, {}, {}, {}]: it has to run min_x, min_y, max_x, max_y with a positive size.",
                    b[0], b[1], b[2], b[3]
                ));
            }
            stock = stock.with_bbox(b);
        }
        if let Some(s) = self.spoilboard {
            stock = stock.over(Spoilboard::new(positive("stock.spoilboard", s)?));
        }
        Ok(stock)
    }
}

// ---------------------------------------------------------------------------
// Machine
// ---------------------------------------------------------------------------

/// `{ min: [x,y,z], max: [x,y,z] }` in machine coordinates.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct TravelReq {
    /// Lowest reachable machine position.
    pub min: [f64; 3],
    /// Highest reachable machine position.
    pub max: [f64; 3],
}

/// `machine { name?, travel?, work_offset?, max_feed?, max_accel?, spindle }`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MachineReq {
    /// Free text, carried into the program's first comment.
    #[serde(default)]
    pub name: Option<String>,
    /// Soft limits, for the envelope check.
    #[serde(default)]
    pub travel: Option<TravelReq>,
    /// Where the work offset sits in machine coordinates, for the same check.
    #[serde(default)]
    pub work_offset: Option<[f64; 3]>,
    /// Rapid ceiling, mm/min. Drives the acceleration-aware time estimate.
    #[serde(default)]
    pub max_feed: Option<f64>,
    /// Acceleration, mm/s². Same.
    #[serde(default)]
    pub max_accel: Option<f64>,
    /// `"dial"` (a trim router whose `S` word does nothing) or
    /// `"controlled"`.
    #[serde(default)]
    pub spindle: Option<String>,
    /// Rigidity class for the feeds table: hobby, benchtop, vmc.
    #[serde(default)]
    pub class: Option<String>,
}

impl MachineReq {
    /// Travel and work offset, validated.
    pub fn travel_limits(&self) -> Result<(Option<TravelLimits>, Option<[f64; 3]>), String> {
        let travel = match self.travel {
            Some(t) => {
                for i in 0..3 {
                    finite(&format!("machine.travel.min[{i}]"), t.min[i])?;
                    finite(&format!("machine.travel.max[{i}]"), t.max[i])?;
                    if t.max[i] <= t.min[i] {
                        return Err(format!(
                            "machine.travel is empty on axis {i}: max {} is not above min {}.",
                            t.max[i], t.min[i]
                        ));
                    }
                }
                Some(TravelLimits {
                    min: t.min,
                    max: t.max,
                })
            }
            None => None,
        };
        let offset = match self.work_offset {
            Some(o) => {
                for (i, v) in o.iter().enumerate() {
                    finite(&format!("machine.work_offset[{i}]"), *v)?;
                }
                Some(o)
            }
            None => None,
        };
        Ok((travel, offset))
    }

    /// Acceleration limits for the time estimate. Defaults to the machine the
    /// first real job ran on.
    pub fn limits(&self) -> Result<MachineLimits, String> {
        let base = MachineLimits::anolex_ultra_2();
        let accel = match self.max_accel {
            Some(a) => positive("machine.max_accel", a)?,
            None => base.max_accel,
        };
        let rate = match self.max_feed {
            Some(f) => positive("machine.max_feed", f)?,
            None => base.max_rate,
        };
        Ok(MachineLimits::new(accel, rate).with_junction_deviation(base.junction_deviation))
    }

    /// The spindle, for the feeds table. A machine that does not say gets the
    /// dial router: assuming the `S` word works when it does not is the
    /// failure that leaves a cutter at the wrong speed with no warning.
    pub fn spindle(&self) -> Result<Spindle, String> {
        let named = self.spindle.as_deref().map(str::to_ascii_lowercase);
        match named.as_deref() {
            None | Some("dial") | Some("router") | Some("router_dial") => {
                Ok(Spindle::router_dial())
            }
            Some("controlled") | Some("vfd") | Some("spindle") => Ok(Spindle::vfd_spindle()),
            Some(other) => Err(format!(
                "machine.spindle is \"{other}\": it has to be \"dial\" (a router with a manual speed dial, whose S word does nothing) or \"controlled\"."
            )),
        }
    }

    /// Rigidity class for the feeds table.
    pub fn class(&self) -> Result<MachineClass, String> {
        let named = self.class.as_deref().map(str::to_ascii_lowercase);
        match named.as_deref() {
            None | Some("hobby") | Some("router") => Ok(MachineClass::Hobby),
            Some("benchtop") | Some("benchtop_mill") => Ok(MachineClass::Benchtop),
            Some("vmc") | Some("rigid") | Some("rigid_vmc") => Ok(MachineClass::Rigid),
            Some(other) => Err(format!(
                "machine.class is \"{other}\": it has to be \"hobby\", \"benchtop\" or \"vmc\"."
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

/// `{ diameter, length, taper_angle? }`.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct HolderReq {
    /// Holder diameter, mm.
    pub diameter: f64,
    /// Holder length, mm.
    pub length: f64,
    /// Taper half-angle in degrees; 0 is a plain cylinder.
    #[serde(default)]
    pub taper_angle: f64,
}

/// One entry of the job's tool library.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolReq {
    /// `T<number>`, as the program will say it.
    pub number: u32,
    /// Free text for the tool-change prompt.
    #[serde(default)]
    pub name: Option<String>,
    /// `flat_end_mill`, `ball_end_mill`, `bull_end_mill`, `v_bit`, `drill`,
    /// `face_mill`.
    #[serde(default)]
    pub kind: Option<String>,
    /// Cutting diameter, mm.
    pub diameter: f64,
    /// Cutting edges.
    #[serde(default)]
    pub flutes: Option<u8>,
    /// Usable cutting length, mm. Defaults to three diameters.
    #[serde(default)]
    pub flute_length: Option<f64>,
    /// How far the tool stands out of the holder, mm.
    #[serde(default)]
    pub stickout: Option<f64>,
    /// Shank diameter, mm, when it differs from the cutting diameter.
    #[serde(default)]
    pub shank_diameter: Option<f64>,
    /// Whether the tool cuts across its own centre (can plunge).
    #[serde(default)]
    pub centre_cutting: Option<bool>,
    /// The holder, for the "will it hit the stock" check.
    #[serde(default)]
    pub holder: Option<HolderReq>,
    /// Corner radius for a bull mill, mm.
    #[serde(default)]
    pub corner_radius: Option<f64>,
    /// Included angle for a V-bit, or point angle for a drill, degrees.
    #[serde(default)]
    pub angle: Option<f64>,
}

impl ToolReq {
    /// Validate and convert to a tool-library entry.
    pub fn build(&self) -> Result<ToolEntry, String> {
        let what = format!("tool T{}", self.number);
        let diameter = positive(&format!("{what}.diameter"), self.diameter)?;
        let flutes = self.flutes.unwrap_or(2);
        let flute_length = match self.flute_length {
            Some(l) => positive(&format!("{what}.flute_length"), l)?,
            None => diameter * 3.0,
        };
        let kind = self
            .kind
            .as_deref()
            .unwrap_or("flat_end_mill")
            .to_ascii_lowercase()
            .replace([' ', '-'], "_");
        let tool = match kind.as_str() {
            "flat_end_mill" | "endmill" | "end_mill" | "flat" => Tool::FlatEndMill {
                diameter,
                flute_length,
                flutes,
            },
            "ball_end_mill" | "ball" => Tool::BallEndMill {
                diameter,
                flute_length,
                flutes,
            },
            "bull_end_mill" | "bull" => Tool::BullEndMill {
                diameter,
                corner_radius: non_negative(
                    &format!("{what}.corner_radius"),
                    self.corner_radius.unwrap_or(diameter * 0.1),
                )?,
                flute_length,
                flutes,
            },
            "v_bit" | "vbit" | "engraver" => Tool::VBit {
                diameter,
                angle: positive(&format!("{what}.angle"), self.angle.unwrap_or(60.0))?,
            },
            "drill" => Tool::Drill {
                diameter,
                point_angle: positive(&format!("{what}.angle"), self.angle.unwrap_or(118.0))?,
            },
            "face_mill" => Tool::FaceMill {
                diameter,
                inserts: flutes,
            },
            other => {
                return Err(format!(
                    "{what}.kind is \"{other}\": it has to be one of flat_end_mill, ball_end_mill, bull_end_mill, v_bit, drill, face_mill."
                ))
            }
        };
        if flutes == 0 && !matches!(tool, Tool::VBit { .. } | Tool::Drill { .. }) {
            return Err(format!(
                "{what}.flutes is 0: a cutter needs at least one edge."
            ));
        }

        let mut geometry = ToolGeometry::new().with_flute_length(flute_length);
        if let Some(s) = self.stickout {
            geometry = geometry.with_stickout(positive(&format!("{what}.stickout"), s)?);
        }
        if let Some(s) = self.shank_diameter {
            geometry =
                geometry.with_shank_diameter(positive(&format!("{what}.shank_diameter"), s)?);
        }
        if let Some(c) = self.centre_cutting {
            geometry = geometry.with_centre_cutting(c);
        }
        if let Some(h) = self.holder {
            let holder = ToolHolder::with_taper(
                positive(&format!("{what}.holder.diameter"), h.diameter)?,
                positive(&format!("{what}.holder.length"), h.length)?,
                non_negative(&format!("{what}.holder.taper_angle"), h.taper_angle)?,
            );
            geometry = geometry.with_holder(holder);
        }

        let name = self
            .name
            .clone()
            .unwrap_or_else(|| format!("Ø{diameter:.3} {}", tool.kind_name()));
        Ok(ToolEntry::new(self.number, name, tool).with_geometry(geometry))
    }
}

// ---------------------------------------------------------------------------
// Part region (for verification)
// ---------------------------------------------------------------------------

/// `{ outer: [[x,y]…], holes: [[[x,y]…]…] }`.
#[derive(Debug, Clone, Deserialize)]
pub struct PartReq {
    /// The part's outer boundary.
    pub outer: Vec<[f64; 2]>,
    /// Openings inside it.
    #[serde(default)]
    pub holes: Vec<Vec<[f64; 2]>>,
}

impl PartReq {
    /// Validate and convert.
    pub fn build(&self) -> Result<PartRegion, String> {
        self.placed(&Placement::identity())
    }

    /// Validate and convert, moved onto the stock by `placement`.
    ///
    /// The part is stated in the part's own frame, so it travels with the
    /// operations rather than staying where it was drawn — otherwise a job
    /// placed on skewed stock would be checked against a part that is not
    /// where the cutter is, and every cut would read as a gouge.
    pub fn placed(&self, placement: &Placement) -> Result<PartRegion, String> {
        let outer = placement.apply_loop(&loop_points("part.outer", &self.outer)?);
        let mut holes = Vec::with_capacity(self.holes.len());
        for (i, h) in self.holes.iter().enumerate() {
            holes.push(placement.apply_loop(&loop_points(&format!("part.holes[{i}]"), h)?));
        }
        PartRegion::new(outer, holes).map_err(|e| format!("the part outline is unusable: {e}"))
    }
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// Per-operation feeds and step sizes, with the clearance heights the job sets.
#[allow(clippy::too_many_arguments)]
pub fn settings(
    what: &str,
    feed: f64,
    plunge: f64,
    rpm: f64,
    stepdown: f64,
    stepover: Option<f64>,
    tool_diameter: f64,
    safe_z: f64,
    retract_z: f64,
) -> Result<CamSettings, String> {
    let stepover = match stepover {
        Some(s) => positive(&format!("{what}.stepover"), s)?,
        None => tool_diameter * 0.4,
    };
    if stepover > tool_diameter + 1e-9 {
        return Err(format!(
            "{what}.stepover is {stepover} mm on a Ø{tool_diameter} cutter: a pass cannot step over further than the cutter is wide, or it leaves uncut ribs."
        ));
    }
    Ok(CamSettings {
        stepover,
        stepdown: positive(&format!("{what}.stepdown"), stepdown)?,
        feed_rate: positive(&format!("{what}.feed"), feed)?,
        plunge_rate: positive(&format!("{what}.plunge"), plunge)?,
        spindle_rpm: positive(&format!("{what}.rpm"), rpm)?,
        safe_z,
        retract_z,
    })
}
