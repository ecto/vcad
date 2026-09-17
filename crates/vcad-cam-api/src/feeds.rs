//! `vcad_cam_materials`, `vcad_cam_recommend`, `vcad_cam_check_feeds`.
//!
//! Friction-log item 54, in one line: *"The app has no material setting at
//! all — feeds were typed by hand for a material that turned out to be a
//! different one."* These three calls are the table, the arithmetic, and the
//! second opinion on numbers the operator already has.
//!
//! On a router with a manual speed dial the `S` word does nothing, so a
//! recommendation that only said "13 500 rpm" would be unusable. The
//! recommendation carries the **dial position** to set by hand whenever the
//! spindle is one of those.

use serde::Deserialize;
use serde_json::{json, Value};

use vcad_kernel_cam::materials::{
    check, material, materials, recommend, Machine, MachineClass, OpKind, ToolKind, ToolSpec,
};
use vcad_kernel_cam::CamSettings;

use crate::types::{positive, MachineReq};

/// The whole material table, with the hazards and guidance attached to each
/// entry.
pub fn materials_list() -> Value {
    json!({
        "materials": materials(),
        "operations": ["slot", "profile", "pocket", "drill", "finish"],
        "machine_classes": ["hobby", "benchtop", "vmc"],
        "spindles": ["dial", "controlled"],
    })
}

/// `{ "material": "copper-c110", "op": "slot",
///    "tool": { "diameter": 2.0, "flutes": 2, "kind": "flat_end_mill", "flute_length": 6 },
///    "machine": { "class": "hobby", "spindle": "dial", "max_feed": 4000 } }`
#[derive(Debug, Clone, Deserialize)]
struct RecommendRequest {
    material: String,
    #[serde(default)]
    op: Option<String>,
    tool: ToolSpecReq,
    #[serde(default)]
    machine: MachineReq,
}

#[derive(Debug, Clone, Deserialize)]
struct ToolSpecReq {
    diameter: f64,
    #[serde(default)]
    flutes: Option<u8>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    flute_length: Option<f64>,
}

impl ToolSpecReq {
    fn build(&self) -> Result<ToolSpec, String> {
        let diameter = positive("tool.diameter", self.diameter)?;
        let flutes = self.flutes.unwrap_or(2);
        if flutes == 0 {
            return Err("tool.flutes is 0: a cutter needs at least one edge.".into());
        }
        let kind = match self
            .kind
            .as_deref()
            .map(|k| k.to_ascii_lowercase().replace([' ', '-'], "_"))
            .as_deref()
        {
            None | Some("flat_end_mill") | Some("endmill") | Some("end_mill") => {
                ToolKind::FlatEndMill
            }
            Some("ball_end_mill") | Some("ball") => ToolKind::BallEndMill,
            Some("bull_end_mill") | Some("bull") => ToolKind::BullEndMill,
            Some("v_bit") | Some("vbit") => ToolKind::VBit,
            Some("drill") => ToolKind::Drill,
            Some("face_mill") => ToolKind::FaceMill,
            Some(other) => {
                return Err(format!(
                    "tool.kind is \"{other}\": it has to be one of flat_end_mill, ball_end_mill, bull_end_mill, v_bit, drill, face_mill."
                ))
            }
        };
        let flute_length = match self.flute_length {
            Some(l) => positive("tool.flute_length", l)?,
            None => diameter * 3.0,
        };
        Ok(ToolSpec::new(diameter, flutes, kind, flute_length))
    }
}

fn op_kind(named: Option<&str>) -> Result<OpKind, String> {
    match named.map(|s| s.to_ascii_lowercase()).as_deref() {
        // A profile cut right through sheet stock is buried on both sides —
        // the kernel's own note, and the reason the copper anchor reproduces
        // only when it is called a slot.
        None | Some("slot") => Ok(OpKind::Slot),
        Some("profile") => Ok(OpKind::Profile),
        Some("pocket") => Ok(OpKind::Pocket),
        Some("drill") => Ok(OpKind::Drill),
        Some("finish") => Ok(OpKind::Finish),
        Some(other) => Err(format!(
            "op is \"{other}\": it has to be \"slot\", \"profile\", \"pocket\", \"drill\" or \"finish\"."
        )),
    }
}

fn machine_for(req: &MachineReq) -> Result<Machine, String> {
    let class = req.class()?;
    let mut machine = match class {
        MachineClass::Hobby => Machine::anolex_ultra2(),
        MachineClass::Benchtop => Machine::benchtop_mill(),
        MachineClass::Rigid => Machine::rigid_vmc(),
    };
    if let Some(name) = &req.name {
        machine.name = name.clone();
    }
    if let Some(f) = req.max_feed {
        machine.max_feed_mm_min = positive("machine.max_feed", f)?;
    }
    Ok(machine)
}

/// Feeds, speeds, stepdown, stepover and the dial to set.
pub fn recommend_request(input: &str) -> Result<Value, String> {
    let req: RecommendRequest = serde_json::from_str(input).map_err(|e| {
        format!("the feeds request could not be read: {e}. Expected material, tool and op.")
    })?;
    let m = material(&req.material).ok_or_else(|| {
        format!(
            "there is no material \"{}\" in the table. The ids are: {}.",
            req.material,
            materials()
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;
    let tool = req.tool.build()?;
    let op = op_kind(req.op.as_deref())?;
    let machine = machine_for(&req.machine)?;
    let spindle = req.machine.spindle()?;
    let rec = recommend(m, &tool, op, &machine, &spindle)
        .map_err(|e| format!("these feeds could not be worked out: {e}"))?;
    Ok(json!({
        "material": { "id": m.id, "name": m.name, "coolant": m.coolant },
        "machine": { "name": machine.name, "class": machine.class },
        "dial_spindle": !spindle.honours_s_word(),
        "recommendation": rec,
    }))
}

/// `{ "material": "copper-c110", "op": "slot", "tool": {…}, "machine": {…},
///    "settings": { "feed": 250, "plunge": 40, "rpm": 13500,
///                  "stepdown": 0.17, "stepover": 0.8 } }`
#[derive(Debug, Clone, Deserialize)]
struct CheckRequest {
    material: String,
    #[serde(default)]
    op: Option<String>,
    tool: ToolSpecReq,
    #[serde(default)]
    machine: MachineReq,
    settings: SettingsReq,
}

#[derive(Debug, Clone, Deserialize)]
struct SettingsReq {
    feed: f64,
    plunge: f64,
    rpm: f64,
    stepdown: f64,
    #[serde(default)]
    stepover: Option<f64>,
}

/// Second opinion on numbers the operator already has.
pub fn check_request(input: &str) -> Result<Value, String> {
    let req: CheckRequest = serde_json::from_str(input).map_err(|e| {
        format!(
            "the feed-check request could not be read: {e}. Expected material, tool and settings."
        )
    })?;
    let m = material(&req.material)
        .ok_or_else(|| format!("there is no material \"{}\" in the table.", req.material))?;
    let tool = req.tool.build()?;
    let op = op_kind(req.op.as_deref())?;
    let machine = machine_for(&req.machine)?;
    let spindle = req.machine.spindle()?;
    let settings = CamSettings {
        feed_rate: positive("settings.feed", req.settings.feed)?,
        plunge_rate: positive("settings.plunge", req.settings.plunge)?,
        spindle_rpm: positive("settings.rpm", req.settings.rpm)?,
        stepdown: positive("settings.stepdown", req.settings.stepdown)?,
        stepover: match req.settings.stepover {
            Some(s) => positive("settings.stepover", s)?,
            None => tool.diameter_mm * 0.4,
        },
        safe_z: 5.0,
        retract_z: 5.0,
    };
    let notes = check(&settings, m, &tool, op, &machine, &spindle);
    let worst = notes.iter().map(|n| n.level).max();
    Ok(json!({
        "material": { "id": m.id, "name": m.name },
        "worst_level": worst,
        // The oracle's verdict in one boolean: nothing above a caution.
        "ok": worst.is_none_or(|l| l <= vcad_kernel_cam::materials::NoteLevel::Caution),
        "notes": notes,
    }))
}
