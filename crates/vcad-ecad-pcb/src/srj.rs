//! Simple Route JSON (SRJ) interchange — the tscircuit autorouting benchmark
//! format — mapped onto the vcad `Pcb` IR so [`crate::router::route_all`] can
//! be scored on the public tscircuit datasets (and any SRJ-emitting tool can
//! drive the vcad router).
//!
//! SRJ is deliberately tiny: a bounds rect, a layer count, rectangular
//! obstacles (optionally tied to a connection), and connections as lists of
//! points to join. See <https://github.com/tscircuit/autorouting>.
//!
//! The adapter builds a synthetic board: outline from `bounds`, a stackup of
//! `layer_count` copper layers, one single-pad footprint per connection point
//! (so the router's pad-derived netlist and MST ratsnest work unchanged), and
//! obstacles as pads of a reserved unrouteable net (`connected_to` obstacles
//! join their connection's net instead, so routes may land on them).

use serde::{Deserialize, Serialize};
use vcad_ir::ecad::{
    BoardOutline, DesignRules, Footprint, LayerStackup, Net, NetClassRules, Pad, PadShape, PadType,
    Pcb, PcbLayer, StackupLayer,
};
use vcad_ir::Vec2;

/// An SRJ point, with the layer it sits on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SrjPoint {
    /// X (mm).
    pub x: f64,
    /// Y (mm).
    pub y: f64,
    /// Layer name ("top", "bottom", "inner1"…); absent means top.
    #[serde(default)]
    pub layer: Option<String>,
}

/// A rectangular SRJ obstacle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SrjObstacle {
    /// Obstacle kind (only "rect" is used by the datasets).
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    /// Layers the obstacle occupies.
    #[serde(default)]
    pub layers: Vec<String>,
    /// Rect center.
    pub center: SrjPoint,
    /// Rect width (mm).
    pub width: f64,
    /// Rect height (mm).
    pub height: f64,
    /// Connection names whose routes may touch this obstacle (a pad of that
    /// net); empty means the obstacle blocks everything.
    #[serde(rename = "connectedTo", default)]
    pub connected_to: Vec<String>,
}

/// One SRJ connection: a set of points that must be electrically joined.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SrjConnection {
    /// Connection (net) name.
    pub name: String,
    /// The points to join.
    #[serde(rename = "pointsToConnect")]
    pub points_to_connect: Vec<SrjPoint>,
}

/// SRJ bounds rect.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SrjBounds {
    /// Minimum X (mm).
    #[serde(rename = "minX")]
    pub min_x: f64,
    /// Maximum X (mm).
    #[serde(rename = "maxX")]
    pub max_x: f64,
    /// Minimum Y (mm).
    #[serde(rename = "minY")]
    pub min_y: f64,
    /// Maximum Y (mm).
    #[serde(rename = "maxY")]
    pub max_y: f64,
}

/// A Simple Route JSON problem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimpleRouteJson {
    /// Number of copper layers (1–32).
    #[serde(rename = "layerCount", default = "default_layer_count")]
    pub layer_count: usize,
    /// Minimum trace width (mm).
    #[serde(rename = "minTraceWidth", default = "default_trace_width")]
    pub min_trace_width: f64,
    /// Obstacles.
    #[serde(default)]
    pub obstacles: Vec<SrjObstacle>,
    /// Connections to route.
    #[serde(default)]
    pub connections: Vec<SrjConnection>,
    /// Board bounds.
    pub bounds: SrjBounds,
}

fn default_layer_count() -> usize {
    2
}
fn default_trace_width() -> f64 {
    0.15
}

/// Reserved net name for obstacles not connected to any routed net. No
/// connection may use it, so obstacle copper blocks every route.
pub const OBSTACLE_NET: &str = "__srj_obstacle__";

/// The copper stack for an SRJ `layer_count`, front → back. A count outside
/// `1..=32` is an error, not a clamp (a clamped stack silently re-homes
/// every layer past the cap).
pub fn srj_layers(layer_count: usize) -> Result<Vec<PcbLayer>, String> {
    if layer_count == 0 {
        return Err("SRJ layerCount must be at least 1".into());
    }
    PcbLayer::copper_stack(layer_count).ok_or_else(|| {
        format!("SRJ layerCount {layer_count} exceeds the 32 copper layers vcad supports")
    })
}

/// Map an SRJ layer name to a [`PcbLayer`] within a `layer_count` stack.
/// "top" (or no name) is FCu, "bottom" is BCu (or FCu on a 1-layer board),
/// "innerN" is the N-th inner layer. Anything else — an unknown name, or an
/// inner layer the stack doesn't have — is an error.
pub fn srj_layer(name: Option<&str>, layer_count: usize) -> Result<PcbLayer, String> {
    let stack = srj_layers(layer_count)?;
    match name.unwrap_or("top") {
        "top" => Ok(PcbLayer::FCu),
        "bottom" => Ok(*stack.last().unwrap_or(&PcbLayer::FCu)),
        other => other
            .strip_prefix("inner")
            .and_then(|d| d.parse::<usize>().ok())
            .filter(|&i| i >= 1 && i + 1 < stack.len())
            .map(|i| stack[i])
            .ok_or_else(|| {
                format!("SRJ layer {other:?} is not a layer of a {layer_count}-layer board")
            }),
    }
}

/// Build a routable [`Pcb`] from an SRJ problem.
pub fn srj_to_pcb(srj: &SimpleRouteJson) -> Result<Pcb, String> {
    let stack = srj_layers(srj.layer_count)?;
    let b = &srj.bounds;
    let w = srj.min_trace_width;

    let copper = |layer| StackupLayer {
        layer,
        copper_thickness: Some(0.035),
        dielectric_thickness: Some(1.6 / stack.len().max(2) as f64),
        dielectric_er: Some(4.5),
        material: Some("FR4".into()),
    };

    let mut footprints: Vec<Footprint> = Vec::new();
    let mut nets: Vec<Net> = Vec::new();

    // One single-pad footprint per connection point: the router's netlist and
    // MST ratsnest are pad-derived, so SRJ points become tiny same-net pads.
    for conn in &srj.connections {
        nets.push(Net {
            id: conn.name.clone(),
            name: conn.name.clone(),
        });
        for (i, p) in conn.points_to_connect.iter().enumerate() {
            let layer = srj_layer(p.layer.as_deref(), srj.layer_count)?;
            footprints.push(point_footprint(
                format!("{}__{}", conn.name, i),
                Vec2::new(p.x, p.y),
                layer,
                w,
                conn.name.clone(),
            ));
        }
    }

    // Obstacles: pads of the reserved (or connected) net, sized exactly.
    for (i, ob) in srj.obstacles.iter().enumerate() {
        let net = ob
            .connected_to
            .first()
            .cloned()
            .unwrap_or_else(|| OBSTACLE_NET.to_string());
        let layers: Vec<PcbLayer> = if ob.layers.is_empty() {
            stack.clone()
        } else {
            ob.layers
                .iter()
                .map(|l| srj_layer(Some(l), srj.layer_count))
                .collect::<Result<_, _>>()?
        };
        footprints.push(Footprint {
            reference: format!("OBS{i}"),
            value: String::new(),
            footprint_name: "srj_obstacle".into(),
            position: Vec2::new(ob.center.x, ob.center.y),
            rotation: 0.0,
            front: true,
            pads: vec![Pad {
                number: "1".into(),
                pad_type: PadType::SMD,
                shape: PadShape::Rect {
                    width: ob.width,
                    height: ob.height,
                },
                position: Vec2::new(0.0, 0.0),
                rotation: 0.0,
                drill: None,
                // The obstacle net is never in `connections`, so the router
                // never routes it — the copper just blocks.
                net: Some(net),
                layers,
            }],
            graphics: vec![],
            model_3d: None,
            properties: Default::default(),
        });
    }

    Ok(Pcb {
        outline: BoardOutline {
            vertices: vec![
                Vec2::new(b.min_x, b.min_y),
                Vec2::new(b.max_x, b.min_y),
                Vec2::new(b.max_x, b.max_y),
                Vec2::new(b.min_x, b.max_y),
            ],
            cutouts: vec![],
            thickness: 1.6,
        },
        stackup: LayerStackup {
            layers: stack.iter().copied().map(copper).collect(),
        },
        nets,
        rules: DesignRules {
            min_microvia_drill: 0.1,
            min_microvia_diameter: 0.2,
            default_rules: NetClassRules {
                name: "Default".into(),
                trace_width: w,
                // SRJ carries no clearance; tscircuit's checks use the trace
                // width as the effective spacing floor, so mirror that.
                clearance: w,
                via_diameter: (w * 2.0).max(0.6),
                via_drill: (w).max(0.3),
                diff_pair_gap: None,
                diff_pair_width: None,
                target_impedance: None,
                target_diff_impedance: None,
            },
            class_rules: vec![],
            net_class_assignments: Default::default(),
            edge_clearance: 0.0,
            hole_to_hole: w,
            min_annular_ring: 0.05,
            min_drill: 0.1,
        },
        footprints,
        traces: vec![],
        trace_arcs: vec![],
        vias: vec![],
        zones: vec![],
        keepouts: vec![],
        net_ties: vec![],
    })
}

/// The net names an SRJ problem asks to route (its connection names).
pub fn srj_net_filter(srj: &SimpleRouteJson) -> Vec<String> {
    srj.connections.iter().map(|c| c.name.clone()).collect()
}

fn point_footprint(
    reference: String,
    at: Vec2,
    layer: PcbLayer,
    width: f64,
    net: String,
) -> Footprint {
    Footprint {
        reference,
        value: String::new(),
        footprint_name: "srj_point".into(),
        position: at,
        rotation: 0.0,
        front: layer == PcbLayer::FCu,
        pads: vec![Pad {
            number: "1".into(),
            pad_type: PadType::SMD,
            shape: PadShape::Circle { diameter: width },
            position: Vec2::new(0.0, 0.0),
            rotation: 0.0,
            drill: None,
            net: Some(net),
            layers: vec![layer],
        }],
        graphics: vec![],
        model_3d: None,
        properties: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::route_all;

    fn srj(json: &str) -> SimpleRouteJson {
        serde_json::from_str(json).expect("valid SRJ")
    }

    #[test]
    fn parses_and_routes_a_minimal_problem() {
        let s = srj(r#"{
            "layerCount": 2,
            "minTraceWidth": 0.2,
            "obstacles": [
                {"type": "rect", "layers": ["top"], "center": {"x": 5, "y": 0},
                 "width": 1.0, "height": 8.0, "connectedTo": []}
            ],
            "connections": [
                {"name": "A", "pointsToConnect": [
                    {"x": 0, "y": 0, "layer": "top"},
                    {"x": 10, "y": 0, "layer": "top"}
                ]}
            ],
            "bounds": {"minX": -2, "maxX": 12, "minY": -6, "maxY": 6}
        }"#);
        let pcb = srj_to_pcb(&s).unwrap();
        assert_eq!(pcb.stackup.layers.len(), 2);
        assert_eq!(pcb.footprints.len(), 3); // 2 points + 1 obstacle

        let r = route_all(&pcb, s.min_trace_width, &srj_net_filter(&s));
        assert_eq!(
            r.routed_nets,
            vec!["A".to_string()],
            "must route around/under the wall"
        );
        assert!(r.unrouted_nets.is_empty());
    }

    #[test]
    fn ten_layer_stack_maps_top_bottom_and_inners() {
        let stack = srj_layers(10).unwrap();
        assert_eq!(stack.len(), 10);
        assert_eq!(stack[0], PcbLayer::FCu);
        assert_eq!(*stack.last().unwrap(), PcbLayer::BCu);
        assert_eq!(srj_layer(Some("inner1"), 10), Ok(PcbLayer::In1Cu));
        assert_eq!(srj_layer(Some("inner8"), 10), Ok(PcbLayer::In8Cu));
        assert_eq!(srj_layer(Some("bottom"), 10), Ok(PcbLayer::BCu));
        // On a 1-layer board "bottom" folds onto the only copper there is.
        assert_eq!(srj_layer(Some("bottom"), 1), Ok(PcbLayer::FCu));
    }

    #[test]
    fn unknown_or_out_of_stack_layers_are_errors() {
        // inner9 on a 10-layer board used to resolve to stack[9] = BCu.
        assert!(srj_layer(Some("inner9"), 10).is_err());
        assert!(srj_layer(Some("inner0"), 10).is_err());
        assert!(srj_layer(Some("middle"), 4).is_err());
        assert_eq!(srj_layer(Some("inner20"), 32), Ok(PcbLayer::In20Cu));
        assert_eq!(srj_layers(32).unwrap().len(), 32);
        assert!(srj_layers(33).is_err());
        assert!(srj_layers(0).is_err());
    }
}
