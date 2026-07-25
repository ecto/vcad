//! Board feature extraction: lenient `Pcb` JSON → board-local mounting holes,
//! edge connectors, and per-component Z extents.
//!
//! The structs here deliberately deserialize only the fields the extraction
//! needs and default everything else, so a partial or newer `Pcb` document
//! (extra fields, missing stackup, expression-typed values elsewhere) never
//! fails the fit check — matching the duck-typed behavior of the original TS.

use serde::{Deserialize, Serialize};

use crate::fit::{
    nearest_edge, outline_aabb, BoardOutline, ComponentExtent, ConnectorRef, MountingHole, Vec2,
};
use crate::round2;

/// A footprint pad, reduced to the fields hole extraction reads.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PadLite {
    /// Pad type discriminator (`"NPTH"` is what we look for).
    #[serde(rename = "padType", default)]
    pub pad_type: Option<String>,
    /// Pad shape (probed for `{ type: "Circle", diameter }`).
    #[serde(default)]
    pub shape: Option<serde_json::Value>,
    /// Pad position on the footprint.
    #[serde(default)]
    pub position: Option<Vec2>,
    /// Pad rotation (degrees), unused by extraction but accepted.
    #[serde(default)]
    pub rotation: Option<f64>,
    /// Drill spec (probed for `{ diameter }`).
    #[serde(default)]
    pub drill: Option<serde_json::Value>,
}

/// A footprint, reduced to the fields feature extraction reads.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FootprintLite {
    /// Designator (e.g. `J1`).
    #[serde(rename = "ref", default)]
    pub reference: String,
    /// Component value string.
    #[serde(default)]
    pub value: Option<String>,
    /// Footprint library name.
    #[serde(rename = "footprintName", default)]
    pub footprint_name: String,
    /// Footprint origin on the board.
    #[serde(default)]
    pub position: Option<Vec2>,
    /// Footprint rotation (degrees).
    #[serde(default)]
    pub rotation: Option<f64>,
    /// True for front-side placement.
    #[serde(default)]
    pub front: Option<bool>,
    /// Pads.
    #[serde(default)]
    pub pads: Vec<PadLite>,
}

/// A `Pcb` document, reduced to the footprint list.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PcbLite {
    /// Placed footprints.
    #[serde(default)]
    pub footprints: Vec<FootprintLite>,
}

/// One kernel component mesh reference: designator + flat vertex positions in
/// board-local coordinates (board bottom z = 0).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ComponentMeshRef {
    /// Footprint designator the mesh belongs to.
    pub footprint_ref: String,
    /// Flat `[x, y, z, …]` vertex positions.
    pub positions: Vec<f64>,
}

/// World (board-frame) position of a pad on its footprint.
fn pad_world(fp_pos: Vec2, fp_rot: f64, pad_pos: Vec2) -> Vec2 {
    let t = fp_rot.to_radians();
    let (sin, cos) = (t.sin(), t.cos());
    Vec2 {
        x: fp_pos.x + pad_pos.x * cos - pad_pos.y * sin,
        y: fp_pos.y + pad_pos.x * sin + pad_pos.y * cos,
    }
}

/// Case-insensitive port of `/mount(ing)?[_-]?hole|mountingpad|mounthole/`.
fn is_mount_name(s: &str) -> bool {
    let s = s.to_lowercase();
    if s.contains("mountingpad") || s.contains("mounthole") {
        return true;
    }
    let bytes = s.as_bytes();
    let mut i = 0;
    while let Some(pos) = s[i..].find("mount") {
        let mut j = i + pos + "mount".len();
        if s[j..].starts_with("ing") {
            j += 3;
        }
        if j < bytes.len() && (bytes[j] == b'_' || bytes[j] == b'-') {
            j += 1;
        }
        if s[j..].starts_with("hole") {
            return true;
        }
        i += pos + 1;
    }
    false
}

/// `drill.diameter` when drill is an object with a numeric diameter.
fn drill_diameter(drill: &Option<serde_json::Value>) -> Option<f64> {
    drill
        .as_ref()
        .and_then(|d| d.as_object())
        .and_then(|o| o.get("diameter"))
        .and_then(|v| v.as_f64())
}

/// `shape.diameter` when the shape is `{ type: "Circle", diameter }`.
fn circle_diameter(shape: &Option<serde_json::Value>) -> Option<f64> {
    let o = shape.as_ref()?.as_object()?;
    if o.get("type")?.as_str()? != "Circle" {
        return None;
    }
    o.get("diameter")?.as_f64()
}

/// Mounting holes the board declares, in board-local coords. Sourced from
/// MountingHole footprints (their origin) and from any NPTH pad (its drilled
/// position). Diameter comes from the drill spec, else the pad/footprint size.
pub fn mounting_holes_from_pcb(pcb: &PcbLite) -> Vec<MountingHole> {
    let mut holes = Vec::new();
    for fp in &pcb.footprints {
        let fp_pos = fp.position.unwrap_or(Vec2 { x: 0.0, y: 0.0 });
        let is_mount = is_mount_name(&fp.footprint_name) || is_mount_name(&fp.reference);
        if is_mount {
            // Diameter from the first pad's drill, else its outer size, else M3.
            let mut dia = 3.2;
            if let Some(pad) = fp.pads.first() {
                // A drill *object* takes the branch even without a numeric
                // diameter (the shape fallback is then skipped) — TS parity.
                if pad.drill.as_ref().is_some_and(|d| d.is_object()) {
                    if let Some(d) = drill_diameter(&pad.drill) {
                        dia = d;
                    }
                } else if let Some(d) = circle_diameter(&pad.shape) {
                    dia = d;
                }
            }
            holes.push(MountingHole {
                x: round2(fp_pos.x),
                y: round2(fp_pos.y),
                diameter: round2(dia),
                reference: Some(fp.reference.clone()),
            });
            continue;
        }
        for pad in &fp.pads {
            if pad.pad_type.as_deref() != Some("NPTH") {
                continue;
            }
            let w = pad_world(
                fp_pos,
                fp.rotation.unwrap_or(0.0),
                pad.position.unwrap_or(Vec2 { x: 0.0, y: 0.0 }),
            );
            let mut dia = 3.2;
            if let Some(d) = drill_diameter(&pad.drill) {
                dia = d;
            } else if let Some(d) = circle_diameter(&pad.shape) {
                dia = d;
            }
            holes.push(MountingHole {
                x: round2(w.x),
                y: round2(w.y),
                diameter: round2(dia),
                reference: Some(fp.reference.clone()),
            });
        }
    }
    holes
}

/// Case-insensitive port of `/^(J|CN|CON|USB|P)\d/`.
fn is_connector_ref(s: &str) -> bool {
    let s = s.to_lowercase();
    for prefix in ["j", "cn", "con", "usb", "p"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                return true;
            }
        }
    }
    false
}

/// Case-insensitive port of the connector-name regex (usb, type-c, conn, jst,
/// molex, rj45, header, terminal, socket, receptacle, barrel, dcjack, …).
fn is_connector_name(s: &str) -> bool {
    let s = s.to_lowercase();
    const NEEDLES: [&str; 16] = [
        "usb",
        "typec",
        "type-c",
        "type_c",
        "micro",
        "mini",
        "conn",
        "header",
        "jst",
        "molex",
        "rj45",
        "hdr",
        "terminal",
        "socket",
        "receptacle",
        "barrel",
    ];
    NEEDLES.iter().any(|n| s.contains(n)) || s.contains("dcjack")
}

/// Edge connectors the board declares, in board-local coords, each tagged with
/// the nearest board edge (so the cutout check knows which wall to look at).
pub fn connectors_from_pcb(pcb: &PcbLite, outline: &BoardOutline) -> Vec<ConnectorRef> {
    let aabb = outline_aabb(outline);
    let mut out = Vec::new();
    for fp in &pcb.footprints {
        let is_conn = is_connector_ref(&fp.reference)
            || is_connector_name(&fp.footprint_name)
            || is_connector_name(fp.value.as_deref().unwrap_or(""));
        if !is_conn {
            continue;
        }
        let pos = fp.position.unwrap_or(Vec2 { x: 0.0, y: 0.0 });
        out.push(ConnectorRef {
            reference: fp.reference.clone(),
            x: round2(pos.x),
            y: round2(pos.y),
            edge: Some(nearest_edge(
                pos.x, pos.y, aabb.min_x, aabb.max_x, aabb.min_y, aabb.max_y,
            )),
            height: 0.0,
        });
    }
    out
}

/// Map kernel component meshes (board-local, board bottom z=0) to
/// per-component Z extents. `front` comes from the matching footprint (default
/// front), and decides whether the part rises toward the lid or dips toward
/// the floor.
pub fn component_extents_from_meshes(
    meshes: &[ComponentMeshRef],
    pcb: &PcbLite,
) -> Vec<ComponentExtent> {
    let mut out = Vec::new();
    for m in meshes {
        let mut min_z = f64::INFINITY;
        let mut max_z = f64::NEG_INFINITY;
        let mut i = 2;
        while i < m.positions.len() {
            let z = m.positions[i];
            min_z = min_z.min(z);
            max_z = max_z.max(z);
            i += 3;
        }
        if !min_z.is_finite() {
            continue;
        }
        let front = pcb
            .footprints
            .iter()
            .find(|fp| fp.reference == m.footprint_ref)
            .map(|fp| fp.front.unwrap_or(true))
            .unwrap_or(true);
        out.push(ComponentExtent {
            reference: m.footprint_ref.clone(),
            front,
            top_z: round2(max_z),
            bottom_z: round2(min_z),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_pcb() -> PcbLite {
        serde_json::from_value(serde_json::json!({
            "footprints": [
                {
                    "ref": "H1",
                    "value": "M3",
                    "footprintName": "MountingHole_3.2mm_M3",
                    "position": { "x": 2.75, "y": 2.75 },
                    "pads": [{
                        "number": "1",
                        "padType": "NPTH",
                        "shape": { "type": "Circle", "diameter": 3.2 },
                        "position": { "x": 0.0, "y": 0.0 },
                        "layers": [],
                        "drill": { "diameter": 3.2 }
                    }]
                },
                {
                    "ref": "J1",
                    "value": "USB-C",
                    "footprintName": "USB_C_Receptacle",
                    "position": { "x": 36.0, "y": 18.0 },
                    "pads": []
                },
                {
                    "ref": "U1",
                    "value": "STM32F405",
                    "footprintName": "QFN-48",
                    "position": { "x": 18.0, "y": 18.0 },
                    "pads": []
                }
            ]
        }))
        .unwrap()
    }

    fn outline() -> BoardOutline {
        BoardOutline {
            vertices: vec![
                Vec2 { x: 0.0, y: 0.0 },
                Vec2 { x: 36.0, y: 0.0 },
                Vec2 { x: 36.0, y: 36.0 },
                Vec2 { x: 0.0, y: 36.0 },
            ],
            thickness: 1.6,
        }
    }

    #[test]
    fn pulls_mounting_holes_from_footprints() {
        let holes = mounting_holes_from_pcb(&fixture_pcb());
        assert_eq!(holes.len(), 1);
        assert_eq!(holes[0].reference.as_deref(), Some("H1"));
        assert!((holes[0].diameter - 3.2).abs() < 0.05);
        assert!((holes[0].x - 2.75).abs() < 1e-9);
    }

    #[test]
    fn identifies_connectors_but_not_the_mcu() {
        let conns = connectors_from_pcb(&fixture_pcb(), &outline());
        assert_eq!(conns.len(), 1);
        assert_eq!(conns[0].reference, "J1");
        assert_eq!(conns[0].edge, Some(crate::WallEdge::MaxX));
    }

    #[test]
    fn derives_component_extents_from_meshes() {
        let meshes = vec![ComponentMeshRef {
            footprint_ref: "U1".into(),
            positions: vec![0.0, 0.0, 1.6, 1.0, 1.0, 2.6, 2.0, 2.0, 1.6],
        }];
        let ext = component_extents_from_meshes(&meshes, &fixture_pcb());
        assert_eq!(ext.len(), 1);
        assert!((ext[0].top_z - 2.6).abs() < 1e-9);
        assert!((ext[0].bottom_z - 1.6).abs() < 1e-9);
        assert!(ext[0].front);
    }

    #[test]
    fn mount_and_connector_matchers_track_the_ts_regexes() {
        assert!(is_mount_name("MountingHole_3.2mm"));
        assert!(is_mount_name("mount-hole"));
        assert!(is_mount_name("MOUNT_HOLE"));
        assert!(is_mount_name("MountingPad_x"));
        assert!(!is_mount_name("mounting_bracket"));
        assert!(is_connector_ref("J1"));
        assert!(is_connector_ref("CON3"));
        assert!(is_connector_ref("usb2"));
        assert!(!is_connector_ref("R1"));
        assert!(!is_connector_ref("CO5"));
        assert!(is_connector_name("USB_C_Receptacle"));
        assert!(is_connector_name("PinHeader_2x05"));
        assert!(is_connector_name("BarrelJack"));
        assert!(!is_connector_name("QFN-48"));
    }

    #[test]
    fn partial_pcb_json_is_tolerated() {
        // Missing pads/position/value on a footprint must not fail.
        let pcb: PcbLite = serde_json::from_value(serde_json::json!({
            "footprints": [{ "ref": "J9", "footprintName": "x" }],
            "unknown_extra": 1
        }))
        .unwrap();
        let conns = connectors_from_pcb(&pcb, &outline());
        assert_eq!(conns.len(), 1);
    }
}
