//! Copper-pour synthesis: turn a high-current net into a poured plane instead
//! of a bundle of thin traces.
//!
//! The router synthesizes *traces*. [`crate::copper_pour`] fills zones that
//! already exist. Nothing created zone **definitions** — so a board whose power
//! nets carry tens of amps got hairline copper where a human designer would
//! flood a region. This module closes that gap: it decides which nets deserve a
//! pour, draws the outline, and hands the resulting [`Zone`]s back to the router,
//! which then reaches every pad through the existing plane-stitching machinery.
//!
//! The two halves are deliberately separate, and each is inspectable on its own:
//!
//! * **Policy** — [`PourPolicy`] + [`decide_pours`]: *which* nets, on *which*
//!   layers, and *why* ([`PourReason`]). Purely a question about the netlist and
//!   the design rules; it touches no geometry. The current-carrying judgement
//!   rides the same IPC-2221 closed form the `size_trace_for_current` MCP tool
//!   uses ([`ipc2221_width_mm`]), so "this net needs a pour" is a statement about
//!   amps, not a name match — even though a name match is the fallback when a
//!   board carries no declared current (imported boards never do).
//! * **Geometry** — [`synthesize_outlines`]: given the accepted candidates, draw
//!   region floods that cover each net's pads with a margin, stay inside the
//!   board, and do not overlap copper already claimed by another pour.
//!
//! Everything downstream is existing machinery. A synthesized zone is an
//! ordinary [`Zone`]: [`crate::copper_pour::fill_zones`] voids it around
//! other-net copper (so a pour never fights the router — it yields to traces),
//! the DRC's clearance and `NetIslands` checks see the filled copper like any
//! other, and `UnstitchedPad` is the check that every pad reached the plane.

use std::collections::{BTreeMap, BTreeSet};

use vcad_ir::ecad::{Footprint, Pad, Pcb, PcbLayer, ThermalReliefStyle, Zone, ZoneFillType};
use vcad_ir::Vec2;
use vcad_kernel_math::Point2;
use vcad_kernel_sheet::poly2d::{self, Poly};

use crate::copper_pour::{
    capsule_poly, ccw, circle_poly, cw, pad_half_extents, pts_to_ring, ring_to_pts,
};

// ---------------------------------------------------------------------------
// Ampacity: IPC-2221 closed form
// ---------------------------------------------------------------------------

/// Copper weight → thickness: 1 oz/ft² ≈ 34.8 µm. Same constant as the
/// `set_stackup` / `size_trace_for_current` MCP surface.
const OZ_TO_MM: f64 = 0.0348;

/// Millimetres per mil — IPC-2221 is stated in mils.
const MM_PER_MIL: f64 = 0.0254;

/// IPC-2221 external-conductor constant.
const K_OUTER: f64 = 0.048;

/// IPC-2221 internal-conductor constant (buried copper sheds heat poorly, so
/// the same current needs roughly twice the cross-section).
const K_INNER: f64 = 0.024;

/// IPC-2221 conductor ampacity solved for width.
///
/// `I = k · ΔT^0.44 · A^0.725` with `A` the cross-section in mil² and
/// `k = 0.048` outer / `0.024` inner. This is the *same* closed form the
/// `size_trace_for_current` MCP tool evaluates — one sizing model for the whole
/// product, so "the width this current needs" means the same thing to the router
/// as it does to an agent asking the tool.
///
/// Returns the width in mm (unsnapped; the MCP tool additionally rounds up to a
/// fab grid, which only matters when the number is going to a fab file).
pub fn ipc2221_width_mm(current_a: f64, copper_oz: f64, temp_rise_c: f64, inner: bool) -> f64 {
    if current_a <= 0.0 || copper_oz <= 0.0 || temp_rise_c <= 0.0 {
        return 0.0;
    }
    let k = if inner { K_INNER } else { K_OUTER };
    let area_mil2 = (current_a / (k * temp_rise_c.powf(0.44))).powf(1.0 / 0.725);
    let thickness_mil = copper_oz * OZ_TO_MM / MM_PER_MIL;
    area_mil2 / thickness_mil * MM_PER_MIL
}

/// The inverse of [`ipc2221_width_mm`]: the current a conductor of `width_mm`
/// carries at the given temperature rise.
///
/// Used to state, in amps, what a net *would* get if it were traced rather than
/// poured — the number that makes a `PourReason` legible without a calculator.
pub fn ipc2221_current_a(width_mm: f64, copper_oz: f64, temp_rise_c: f64, inner: bool) -> f64 {
    if width_mm <= 0.0 || copper_oz <= 0.0 || temp_rise_c <= 0.0 {
        return 0.0;
    }
    let k = if inner { K_INNER } else { K_OUTER };
    let area_mil2 = (width_mm / MM_PER_MIL) * (copper_oz * OZ_TO_MM / MM_PER_MIL);
    k * temp_rise_c.powf(0.44) * area_mil2.powf(0.725)
}

// ---------------------------------------------------------------------------
// Policy
// ---------------------------------------------------------------------------

/// Why a net was selected for a copper pour.
///
/// Ordered by strength of evidence: a declared current is design intent, a wide
/// net class is the designer having already sized the copper, and a power-rail
/// name is the heuristic of last resort for boards that carry neither.
#[derive(Debug, Clone, PartialEq)]
pub enum PourReason {
    /// The design declares how much current this net carries, and IPC-2221 says
    /// that needs more copper than the router would lay as a trace.
    DeclaredCurrent {
        /// Declared continuous current (A).
        current_a: f64,
        /// Width IPC-2221 requires for it (mm).
        required_width_mm: f64,
        /// Width the router would actually route the net at (mm).
        routed_width_mm: f64,
    },
    /// The net's class already asks for copper far wider than the board default
    /// — the designer sized it for current even if they never wrote the number.
    ClassWidth {
        /// The net class's trace width (mm).
        class_width_mm: f64,
        /// The board's default trace width (mm).
        default_width_mm: f64,
        /// Current that class width carries per IPC-2221 (A).
        implied_current_a: f64,
    },
    /// The net's name reads as a power/ground rail and it is distributed across
    /// enough pads to be worth flooding. The fallback for imported boards, which
    /// carry no current annotation at all.
    PowerRailName {
        /// Pads the net has on the board.
        pads: usize,
        /// Current the routed trace width would carry per IPC-2221 (A) — what
        /// the net gets *without* a pour.
        traced_current_a: f64,
    },
}

/// Which nets get poured, and how generously. The policy half of pour synthesis
/// — no geometry, no board mutation, every knob a number a human can argue with.
#[derive(Debug, Clone)]
pub struct PourPolicy {
    /// Master switch. `false` makes synthesis a no-op.
    pub enabled: bool,
    /// Design intent: net name → continuous current it must carry (A). The
    /// strongest signal, and the one an explicit design-intent surface should
    /// drive. Empty on every imported board, which is why the fallbacks exist.
    pub net_current_a: BTreeMap<String, f64>,
    /// Copper weight (oz/ft²) for the ampacity model when the stackup does not
    /// state a copper thickness.
    pub copper_oz: f64,
    /// Allowed conductor temperature rise (°C) for the ampacity model.
    pub temp_rise_c: f64,
    /// A net whose ampacity-required width is at least this multiple of the
    /// width the router would lay is poured instead of traced.
    pub width_ratio: f64,
    /// Nets with fewer pads than this — and less pad copper than
    /// `min_pad_area_mm2` — are never poured: a pour buys nothing over a trace
    /// between two pads, and this is what keeps synthesis off small
    /// point-to-point boards where the router's traces are already the answer.
    pub min_pads: usize,
    /// Total pad copper (mm²) that qualifies a net on its own, regardless of pad
    /// count. High-current nets are often *few, large* pads — a battery input on
    /// two 8x11 mm terminals is the whole reason a pad-count gate is not enough.
    pub min_pad_area_mm2: f64,
    /// Pour nets whose *name* reads as a power/ground rail (see
    /// [`crate::is_power_net`]) once they clear `min_pads`, even with no declared
    /// current. Imported boards carry no current annotation, so without this
    /// nothing on a VESC or a moteus would ever be poured.
    pub pour_power_rails: bool,
    /// Outward margin (mm) added around a net's pad cluster to form its outline.
    pub margin_mm: f64,
    /// A synthesized region covering at least this fraction of the usable board
    /// is promoted to a whole-layer flood — the shape a human would actually
    /// draw once a net is spread across the board.
    pub whole_layer_fraction: f64,
    /// Maximum copper layers one net may be poured on.
    pub max_layers_per_net: usize,
    /// Fraction of a net's pads that must already sit on a layer before it is
    /// poured there. Every pad that is *not* on the poured layer costs a
    /// stitching via, and a plane reached only through 30-odd vias is worse on
    /// via count than the traces it replaced — and each of those vias is another
    /// chance for a pad to end up unstitched on a congested board. Set to `0.0`
    /// to allow a dedicated inner plane the net does not otherwise touch.
    pub min_pad_fraction_on_layer: f64,
    /// Synthesized copper islands smaller than this (mm²) are dropped, both from
    /// the outline and via the zone's own `min_area` at fill time. Slivers carry
    /// no current and strand as `NetIslands`.
    pub min_island_mm2: f64,
}

impl Default for PourPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            net_current_a: BTreeMap::new(),
            copper_oz: 1.0,
            temp_rise_c: 10.0,
            // 2x: a net needing double the routed width has outgrown a trace.
            width_ratio: 2.0,
            // Six pads is the line between "a connection" and "a distribution
            // network". Below it the router's traces are the right answer, and
            // holding the line here is what keeps synthesis from rewriting small
            // point-to-point boards.
            min_pads: 6,
            // ~20 mm2 is several signal pads' worth of copper, or one power
            // terminal. Below it a net is not a distribution network.
            min_pad_area_mm2: 20.0,
            pour_power_rails: true,
            margin_mm: 1.0,
            whole_layer_fraction: 0.55,
            max_layers_per_net: 1,
            // Half: pour where the net already lives.
            min_pad_fraction_on_layer: 0.5,
            min_island_mm2: 1.0,
        }
    }
}

/// One accepted (net, layer) pour, with the evidence behind it.
#[derive(Debug, Clone)]
pub struct PourCandidate {
    /// Net to pour.
    pub net: String,
    /// Copper layer to pour it on.
    pub layer: PcbLayer,
    /// Why this net was selected.
    pub reason: PourReason,
    /// Pads this net has on `layer` (they flood directly; pads elsewhere need a
    /// stitching via).
    pub pads_on_layer: usize,
    /// Pads this net has on the whole board — how much copper the net has to
    /// distribute, and the tiebreak when two nets want the same layer.
    pub demand_pads: usize,
}

/// Copper thickness (oz/ft²) the stackup declares for `layer`, falling back to
/// the policy's assumption.
fn copper_oz_for(pcb: &Pcb, layer: PcbLayer, policy: &PourPolicy) -> f64 {
    pcb.stackup
        .layers
        .iter()
        .find(|l| l.layer == layer)
        .and_then(|l| l.copper_thickness)
        .filter(|t| *t > 0.0)
        .map(|t| t / OZ_TO_MM)
        .unwrap_or(policy.copper_oz)
}

/// True when `layer` is buried (not the top or bottom of the stackup) — the
/// IPC-2221 derating case.
fn is_inner(copper: &[PcbLayer], layer: PcbLayer) -> bool {
    copper.first() != Some(&layer) && copper.last() != Some(&layer)
}

/// Copper layers in the stackup, top → bottom (FCu/BCu when the stackup is
/// silent — the same fallback the router uses).
fn copper_layers(pcb: &Pcb) -> Vec<PcbLayer> {
    let v: Vec<PcbLayer> = pcb
        .stackup
        .layers
        .iter()
        .map(|l| l.layer)
        .filter(|l| l.is_copper())
        .collect();
    if v.is_empty() {
        vec![PcbLayer::FCu, PcbLayer::BCu]
    } else {
        v
    }
}

/// Every pad on the board carrying a non-empty net, as (net, footprint, pad).
fn netted_pads(pcb: &Pcb) -> Vec<(&str, &Footprint, &Pad)> {
    let mut out = Vec::new();
    for fp in &pcb.footprints {
        for pad in &fp.pads {
            if let Some(net) = pad.net.as_deref().filter(|n| !n.is_empty()) {
                out.push((net, fp, pad));
            }
        }
    }
    out
}

/// Net → the copper layers it already owns a zone on. Synthesis never
/// duplicates an existing pour; it only complements one.
fn existing_zone_layers(pcb: &Pcb) -> BTreeMap<&str, Vec<PcbLayer>> {
    let mut m: BTreeMap<&str, Vec<PcbLayer>> = BTreeMap::new();
    for z in &pcb.zones {
        if z.net.is_empty() || z.outline.len() < 3 || !z.layer.is_copper() {
            continue;
        }
        let e = m.entry(z.net.as_str()).or_default();
        if !e.contains(&z.layer) {
            e.push(z.layer);
        }
    }
    m
}

/// Decide which nets deserve a copper pour, and on which layers.
///
/// Pure policy: reads the netlist, the design rules and the stackup, and returns
/// the accepted (net, layer) candidates with the reason for each. Deterministic
/// (nets in name order). `nets_filter`, when non-empty, restricts the decision to
/// those nets — matching the router's own filter semantics.
pub fn decide_pours(pcb: &Pcb, policy: &PourPolicy, nets_filter: &[String]) -> Vec<PourCandidate> {
    if !policy.enabled {
        return Vec::new();
    }
    let copper = copper_layers(pcb);
    let class_width = crate::drc::build_net_trace_width_map(pcb);
    let default_width = pcb.rules.default_rules.trace_width;
    let existing = existing_zone_layers(pcb);

    // Pads per net, pad copper per net, and pads per (net, layer).
    let mut pads_per_net: BTreeMap<&str, usize> = BTreeMap::new();
    let mut pad_area_per_net: BTreeMap<&str, f64> = BTreeMap::new();
    // Per-net pad extent bbox — how much of the board the net actually spans.
    let mut pad_extent_bbox: BTreeMap<&str, (Vec2, Vec2)> = BTreeMap::new();
    let mut pads_per_net_layer: BTreeMap<(&str, PcbLayer), usize> = BTreeMap::new();
    let mut pads_per_layer: BTreeMap<PcbLayer, usize> = BTreeMap::new();
    for (net, fp, pad) in netted_pads(pcb) {
        *pads_per_net.entry(net).or_default() += 1;
        let (hw, hh) = pad_half_extents(pad);
        *pad_area_per_net.entry(net).or_default() += 4.0 * hw * hh;
        let c = crate::geometry::pad_world_position(fp, pad);
        let bb = pad_extent_bbox
            .entry(net)
            .or_insert((Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN)));
        bb.0.x = bb.0.x.min(c.x - hw);
        bb.0.y = bb.0.y.min(c.y - hh);
        bb.1.x = bb.1.x.max(c.x + hw);
        bb.1.y = bb.1.y.max(c.y + hh);
        for l in pad.layers.iter().filter(|l| l.is_copper()) {
            *pads_per_net_layer.entry((net, *l)).or_default() += 1;
            *pads_per_layer.entry(*l).or_default() += 1;
        }
    }

    // --- which nets ---------------------------------------------------------
    let mut wanted: Vec<(&str, PourReason)> = Vec::new();
    for (&net, &pads) in &pads_per_net {
        // Distributed enough to be a network: many pads, or a lot of pad copper
        // (a battery input is two big terminals, not twenty small ones).
        let pad_area = pad_area_per_net.get(net).copied().unwrap_or(0.0);
        if pads < policy.min_pads && pad_area < policy.min_pad_area_mm2 {
            continue;
        }
        if !nets_filter.is_empty() && !nets_filter.iter().any(|n| n == net) {
            continue;
        }
        // Ampacity is judged on the outermost layer the net has pads on — the
        // most favourable case. If even that needs a pour, every layer does.
        let layer = copper
            .iter()
            .copied()
            .find(|l| pads_per_net_layer.contains_key(&(net, *l)))
            .unwrap_or(PcbLayer::FCu);
        let oz = copper_oz_for(pcb, layer, policy);
        let inner = is_inner(&copper, layer);
        let routed_width = class_width.get(net).copied().unwrap_or(default_width);

        if let Some(&current_a) = policy.net_current_a.get(net) {
            let required = ipc2221_width_mm(current_a, oz, policy.temp_rise_c, inner);
            if required >= routed_width * policy.width_ratio {
                wanted.push((
                    net,
                    PourReason::DeclaredCurrent {
                        current_a,
                        required_width_mm: required,
                        routed_width_mm: routed_width,
                    },
                ));
            }
            // A declared current is authoritative: if it fits in a trace, the
            // name heuristic does not get to override it.
            continue;
        }
        if routed_width >= default_width * policy.width_ratio {
            wanted.push((
                net,
                PourReason::ClassWidth {
                    class_width_mm: routed_width,
                    default_width_mm: default_width,
                    implied_current_a: ipc2221_current_a(
                        routed_width,
                        oz,
                        policy.temp_rise_c,
                        inner,
                    ),
                },
            ));
            continue;
        }
        if policy.pour_power_rails && crate::is_power_net(net) {
            wanted.push((
                net,
                PourReason::PowerRailName {
                    pads,
                    traced_current_a: ipc2221_current_a(
                        routed_width,
                        oz,
                        policy.temp_rise_c,
                        inner,
                    ),
                },
            ));
        }
    }

    // --- which layers -------------------------------------------------------
    // Most-distributed nets pick first: the net with the most copper to move
    // gets first refusal on the layer that suits it best.
    wanted.sort_by(|a, b| {
        pads_per_net
            .get(b.0)
            .cmp(&pads_per_net.get(a.0))
            .then_with(|| a.0.cmp(b.0))
    });

    // A copper layer carrying no pads *and* no zone is a bare plane layer. It is
    // a fine home for a pour, but only as a fallback: a pad on a bare inner layer
    // is a pad that needs a stitching via, and vias are the expensive part.
    let zoned_layers: BTreeSet<PcbLayer> = pcb
        .zones
        .iter()
        .filter(|z| z.layer.is_copper() && !z.net.is_empty() && z.outline.len() >= 3)
        .map(|z| z.layer)
        .collect();

    // Board bbox — the denominator for "is this net spread across the board?".
    let board_area = {
        let (mut lo, mut hi) = (Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN));
        for v in &pcb.outline.vertices {
            lo.x = lo.x.min(v.x);
            lo.y = lo.y.min(v.y);
            hi.x = hi.x.max(v.x);
            hi.y = hi.y.max(v.y);
        }
        ((hi.x - lo.x) * (hi.y - lo.y)).max(0.0)
    };
    let spread_of = |net: &str| -> bool {
        let Some(pts) = pad_extent_bbox.get(net) else {
            return false;
        };
        let a = (pts.1.x - pts.0.x) * (pts.1.y - pts.0.y);
        board_area > 0.0 && a >= board_area * policy.whole_layer_fraction
    };

    // Layers a board-spanning pour has taken. Such a pour floods the whole layer,
    // so nothing else can pour there — the geometry pass would only carve the
    // newcomer into debris. Two *local* pours may still share a layer; neither
    // claims it.
    let mut claimed: BTreeSet<PcbLayer> = BTreeSet::new();

    let mut out = Vec::new();
    for (net, reason) in wanted {
        let taken = existing.get(net).cloned().unwrap_or_default();
        // Rank by how many of this net's pads already sit on the layer: those
        // pads flood straight into the pour, and every pad that does *not* costs
        // a stitching via. Picking the layer the net already lives on is the
        // difference between 5 stitches and 43 on a 3-layer motor board.
        let mut chosen: Vec<(PcbLayer, usize)> = copper
            .iter()
            .filter(|l| !taken.contains(l) && !claimed.contains(l))
            .filter_map(|l| {
                let pads_here = pads_per_net_layer.get(&(net, *l)).copied().unwrap_or(0);
                // A layer with none of this net's pads is only worth taking when
                // it is bare — otherwise it is someone else's routing layer.
                let bare = !pads_per_layer.contains_key(l) && !zoned_layers.contains(l);
                (pads_here > 0 || bare).then_some((*l, pads_here))
            })
            .collect();
        chosen.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| format!("{:?}", a.0).cmp(&format!("{:?}", b.0)))
        });
        chosen.truncate(policy.max_layers_per_net.max(1));

        let demand_pads = pads_per_net.get(net).copied().unwrap_or(0);
        // Reject a layer the net barely touches: the pour would be reached
        // almost entirely through stitching vias, which is neither cheaper nor
        // more robust than tracing the net.
        chosen.retain(|(_, pads_here)| {
            *pads_here as f64 >= demand_pads as f64 * policy.min_pad_fraction_on_layer
        });
        let spread = spread_of(net);
        for (layer, pads_on_layer) in chosen {
            if spread {
                claimed.insert(layer);
            }
            out.push(PourCandidate {
                net: net.to_string(),
                layer,
                reason: reason.clone(),
                pads_on_layer,
                demand_pads,
            });
        }
    }
    out.sort_by(|a, b| {
        a.net
            .cmp(&b.net)
            .then_with(|| format!("{:?}", a.layer).cmp(&format!("{:?}", b.layer)))
    });
    out
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

/// The Minkowski dilation of a polygon-with-holes by a disc of radius `r`: the
/// polygon unioned with a capsule along every one of its boundary edges.
fn dilate(poly: &Poly, r: f64) -> Vec<Poly> {
    if r <= 0.0 {
        return vec![poly.clone()];
    }
    let mut parts = vec![poly.clone()];
    let rings = std::iter::once(&poly.outer).chain(poly.holes.iter());
    for ring in rings {
        for i in 0..ring.len() {
            let a = ring[i];
            let b = ring[(i + 1) % ring.len()];
            parts.push(capsule_poly(Vec2::new(a.x, a.y), Vec2::new(b.x, b.y), r));
        }
    }
    poly2d::union_all(&parts)
}

/// The Minkowski erosion of a polygon-with-holes by a disc of radius `r`.
///
/// Erosion is the polygon minus the band its own boundary sweeps: `P ⊖ disc_r =
/// P \ (∂P ⊕ disc_r)`, and `∂P ⊕ disc_r` is exactly the union of capsules along
/// its edges — so this reuses the same capsule the pour clearance already uses
/// rather than introducing a second offsetting engine.
fn erode(poly: &Poly, r: f64) -> Vec<Poly> {
    if r <= 0.0 {
        return vec![poly.clone()];
    }
    let mut band = Vec::new();
    let rings = std::iter::once(&poly.outer).chain(poly.holes.iter());
    for ring in rings {
        for i in 0..ring.len() {
            let a = ring[i];
            let b = ring[(i + 1) % ring.len()];
            band.push(capsule_poly(Vec2::new(a.x, a.y), Vec2::new(b.x, b.y), r));
        }
    }
    poly2d::difference(std::slice::from_ref(poly), &band)
}

/// `a ∩ b`, via `a \ (a \ b)` — poly2d exposes union and difference, and the
/// identity is exact for the same snap-rounded arithmetic.
fn intersect(a: &[Poly], b: &[Poly]) -> Vec<Poly> {
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    let outside = poly2d::difference(a, b);
    if outside.is_empty() {
        return a.to_vec();
    }
    poly2d::difference(a, &outside)
}

/// The board area a pour may legally occupy: the outline (minus its cutouts)
/// eroded by the edge clearance.
pub fn usable_area(pcb: &Pcb) -> Vec<Poly> {
    if pcb.outline.vertices.len() < 3 {
        return Vec::new();
    }
    let board = Poly {
        outer: ccw(ring_to_pts(&pcb.outline.vertices)),
        holes: pcb
            .outline
            .cutouts
            .iter()
            .filter(|c| c.len() >= 3)
            .map(|c| cw(ring_to_pts(c)))
            .collect(),
    };
    erode(&board, pcb.rules.edge_clearance.max(0.0))
}

/// Total area of a set of polygons (outer minus holes).
fn total_area(polys: &[Poly]) -> f64 {
    polys.iter().map(|p| p.area()).sum()
}

/// The four world-space corners of a pad's bounding box.
fn pad_corners(fp: &Footprint, pad: &Pad) -> Vec<Vec2> {
    let c = crate::geometry::pad_world_position(fp, pad);
    let ang = (fp.rotation + pad.rotation).to_radians();
    let (ca, sa) = (ang.cos(), ang.sin());
    let (hw, hh) = pad_half_extents(pad);
    [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)]
        .iter()
        .map(|(x, y)| Vec2::new(c.x + x * ca - y * sa, c.y + x * sa + y * ca))
        .collect()
}

/// The axis-aligned box `min`..`max` as a CCW polygon.
fn box_poly(min: Vec2, max: Vec2) -> Poly {
    Poly::new(ccw(vec![
        Point2::new(min.x, min.y),
        Point2::new(max.x, min.y),
        Point2::new(max.x, max.y),
        Point2::new(min.x, max.y),
    ]))
}

/// Draw the region flood for one net: the bounding box of its pads grown by
/// `margin`, clipped to the usable board area, and promoted to the whole usable
/// area once it covers `whole_layer_fraction` of it.
///
/// A rectangle (or the whole layer) is deliberate — the brief is region floods,
/// not exotic shapes. It is also what a designer draws by hand, it keeps the
/// vertex count low enough that filling it stays cheap on a dense board, and a
/// spread-out net promotes itself to the whole layer rather than producing a
/// rectangle that is a whole-layer flood in all but name.
pub fn synthesize_region(
    pad_points: &[Vec2],
    usable: &[Poly],
    margin: f64,
    whole_layer_fraction: f64,
) -> Vec<Poly> {
    if pad_points.is_empty() || usable.is_empty() {
        return Vec::new();
    }
    let mut min = Vec2::new(f64::MAX, f64::MAX);
    let mut max = Vec2::new(f64::MIN, f64::MIN);
    for p in pad_points {
        min.x = min.x.min(p.x);
        min.y = min.y.min(p.y);
        max.x = max.x.max(p.x);
        max.y = max.y.max(p.y);
    }
    let rect = box_poly(
        Vec2::new(min.x - margin, min.y - margin),
        Vec2::new(max.x + margin, max.y + margin),
    );
    let usable_area = total_area(usable);
    let clipped = intersect(&[rect], usable);
    if usable_area > 0.0 && total_area(&clipped) >= usable_area * whole_layer_fraction {
        return usable.to_vec();
    }
    clipped
}

/// Turn accepted candidates into zone outlines that do not fight each other.
///
/// Three rules, in order:
///
/// 1. **A whole-layer flood owns its layer.** When two board-spanning nets both
///    want to flood one layer, the most distributed one takes it and the other
///    keeps its traces. Carving one board-wide flood out of another leaves
///    confetti, which is not a plane.
/// 2. **Local regions on a shared layer are claimed smallest-first**, so a tight
///    pour (a phase node around its FETs) keeps its copper and a broader one
///    fills in around it — the priority order a human gives overlapping zones.
///    Each later region is cut back by what is already claimed on that layer,
///    inflated by clearance, so two pours never occupy the same copper. The one
///    exception is a disc around the net's own pads: a neighbouring pour is
///    *guaranteed* to be voided there anyway (a pour always clears other-net
///    pads by its clearance), so overlapping exactly there costs no legality and
///    keeps a pad inside a neighbour's box connected to its own plane.
/// 3. **One plane per candidate, and every pad must reach it.** Existing zones
///    can cut a region into pieces; only the largest survives, and if any of the
///    net's pads cannot reach it the whole candidate is dropped. The router
///    treats a poured net as connected *through* its plane and drops its
///    ratsnest, so a pour that misses a pad would silently strand it. Fail
///    closed — that net keeps its traces.
pub fn synthesize_outlines(
    pcb: &Pcb,
    candidates: &[PourCandidate],
    policy: &PourPolicy,
) -> Vec<Zone> {
    let usable = usable_area(pcb);
    if usable.is_empty() {
        return Vec::new();
    }
    let clearance_map = crate::drc::build_net_clearance_map(pcb);
    let default_clearance = pcb.rules.default_rules.clearance;
    let clearance_of = |net: &str| clearance_map.get(net).copied().unwrap_or(default_clearance);

    // Pad geometry per net. Pads on *every* layer count — an off-layer pad
    // reaches the plane through a stitching via dropped at or just beside it,
    // and that via has to land inside the pour.
    //
    // `pad_pts` are the extent points the outline is drawn around; `pad_probes`
    // groups them per pad (centre first) for the reach test; `pad_discs` are the
    // footprints a neighbouring pour is guaranteed to void anyway.
    let mut pad_pts: BTreeMap<&str, Vec<Vec2>> = BTreeMap::new();
    let mut pad_probes: BTreeMap<&str, Vec<Vec<Vec2>>> = BTreeMap::new();
    let mut pad_discs: BTreeMap<&str, Vec<Poly>> = BTreeMap::new();
    let wanted: BTreeSet<&str> = candidates.iter().map(|c| c.net.as_str()).collect();
    for (net, fp, pad) in netted_pads(pcb) {
        if !wanted.contains(net) {
            continue;
        }
        let corners = pad_corners(fp, pad);
        let c = crate::geometry::pad_world_position(fp, pad);
        pad_pts
            .entry(net)
            .or_default()
            .extend(corners.iter().copied());
        let mut probes = Vec::with_capacity(corners.len() + 1);
        probes.push(c);
        probes.extend(corners.iter().copied());
        pad_probes.entry(net).or_default().push(probes);
        let (hw, hh) = pad_half_extents(pad);
        pad_discs
            .entry(net)
            .or_default()
            .push(circle_poly(c, hw.max(hh) + clearance_of(net)));
    }

    // Raw regions first, so the claim order can be "smallest first".
    struct Raw<'a> {
        cand: &'a PourCandidate,
        region: Vec<Poly>,
        area: f64,
        /// This region is the whole usable layer, not a local box.
        whole_layer: bool,
    }
    let usable_total = total_area(&usable);
    let mut raws: Vec<Raw<'_>> = Vec::new();
    for cand in candidates {
        let Some(pts) = pad_pts.get(cand.net.as_str()) else {
            continue;
        };
        let region = synthesize_region(pts, &usable, policy.margin_mm, policy.whole_layer_fraction);
        let area = total_area(&region);
        if area < policy.min_island_mm2 {
            continue;
        }
        let whole_layer = (area - usable_total).abs() <= usable_total * 1e-9;
        raws.push(Raw {
            cand,
            region,
            area,
            whole_layer,
        });
    }

    // A whole-layer flood owns its layer. Two board-spanning nets cannot both
    // flood one layer: carving one against the other leaves confetti, which is
    // neither a plane nor legal (pad-less islands strand as `NetIslands`). The
    // most-distributed net keeps the layer and the loser keeps its traces —
    // that is the same call a designer makes when they hand a layer to GND.
    let mut flooded: BTreeMap<PcbLayer, (&str, usize)> = BTreeMap::new();
    for raw in raws.iter().filter(|r| r.whole_layer) {
        let claim = (raw.cand.net.as_str(), raw.cand.demand_pads);
        match flooded.get(&raw.cand.layer) {
            Some(&(_, pads)) if pads >= claim.1 => {}
            _ => {
                flooded.insert(raw.cand.layer, claim);
            }
        }
    }
    raws.retain(|raw| match flooded.get(&raw.cand.layer) {
        Some(&(owner, _)) => raw.cand.net == owner,
        None => true,
    });

    // Local regions on a shared layer are claimed smallest-first, so a tight
    // pour (a phase node around its FETs) keeps its copper and a broader one
    // fills in around it — the priority order a human gives overlapping zones.
    raws.sort_by(|a, b| {
        a.area
            .total_cmp(&b.area)
            .then_with(|| b.cand.demand_pads.cmp(&a.cand.demand_pads))
            .then_with(|| a.cand.net.cmp(&b.cand.net))
    });

    // Copper already spoken for on each layer: zones the board already carries
    // (synthesis complements them, it never overlaps them) plus every region
    // accepted so far.
    let mut claimed: BTreeMap<PcbLayer, Vec<Poly>> = BTreeMap::new();
    for z in &pcb.zones {
        if z.outline.len() < 3 || !z.layer.is_copper() {
            continue;
        }
        claimed.entry(z.layer).or_default().push(Poly {
            outer: ccw(ring_to_pts(&z.outline)),
            holes: z
                .holes
                .iter()
                .filter(|h| h.len() >= 3)
                .map(|h| cw(ring_to_pts(h)))
                .collect(),
        });
    }

    let mut zones = Vec::new();
    for (rank, raw) in raws.iter().enumerate() {
        let net = raw.cand.net.as_str();
        let clearance = clearance_of(net);
        let layer_claimed = claimed.get(&raw.cand.layer).cloned().unwrap_or_default();
        let mut region = raw.region.clone();
        if !layer_claimed.is_empty() {
            let mut blocked = Vec::new();
            for p in &layer_claimed {
                blocked.extend(dilate(p, clearance));
            }
            region = poly2d::difference(&region, &blocked);
            // Give back the net's own pad footprints: a neighbouring pour is
            // voided there regardless, so this overlap is free.
            if let Some(discs) = pad_discs.get(net) {
                let keep = intersect(&raw.region, &discs.clone());
                if !keep.is_empty() {
                    region.extend(keep);
                    region = poly2d::union_all(&region);
                }
            }
        }
        region.retain(|p| p.area() >= policy.min_island_mm2);
        // A pour is one plane, not a scatter. Existing zones can cut a region
        // into pieces; the largest is the plane and the rest are debris that
        // would strand as `NetIslands` (a pad-less piece) or leave the net in
        // disjoint pad groups (the router drops a poured net's ratsnest, so
        // nothing would ever join them). Keep the plane; the reach test below
        // then decides whether it is good enough to use at all.
        if region.len() > 1 {
            let best = region
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.area().total_cmp(&b.1.area()))
                .map(|(i, _)| i)
                .unwrap_or(0);
            region = vec![region.swap_remove(best)];
        }
        if region.is_empty() {
            continue;
        }

        // Fail closed: the router treats a poured net as connected *through* its
        // plane and drops the net's ratsnest, so a pad the pour cannot reach
        // would be silently stranded. Every pad must therefore reach this
        // region — reach, not containment: a pad whose copper overlaps the pour
        // at all is galvanically part of it, which is also how the DRC's
        // connectivity graph reads it. (Containment is the wrong test: the pour
        // stops an edge-clearance ring short of the board rim, so a connector
        // pad near the edge is never fully inside it.) A candidate with an
        // unreachable pad is dropped and that net keeps its traces.
        let unreached = pad_probes
            .get(net)
            .map(|pads| {
                pads.iter()
                    .filter(|probes| {
                        !probes.iter().any(|p| {
                            region
                                .iter()
                                .any(|r| poly2d::contains_point(r, Point2::new(p.x, p.y)))
                        })
                    })
                    .count()
            })
            .unwrap_or(usize::MAX);
        if unreached > 0 {
            log::debug!(
                "pour synthesis: dropping {net} on {:?} — {unreached} pad(s) out of reach",
                raw.cand.layer
            );
            continue;
        }

        for piece in &region {
            zones.push(Zone {
                outline: pts_to_ring(&piece.outer),
                holes: piece.holes.iter().map(|h| pts_to_ring(h)).collect(),
                net: net.to_string(),
                layer: raw.cand.layer,
                clearance,
                // Prune slivers at fill time too: the outline is convex-ish, but
                // voids around dense other-net copper can fracture the fill.
                min_area: policy.min_island_mm2,
                fill_type: ZoneFillType::Solid,
                thermal_relief: ThermalReliefStyle::Relief,
                thermal_gap: None,
                thermal_spoke_width: None,
                // Smallest (highest-priority) region claimed first, so a local
                // pour outranks the broad plane it sits inside.
                priority: (raws.len() - rank) as u32,
            });
        }
        claimed.entry(raw.cand.layer).or_default().extend(region);
    }
    zones
}

/// Synthesize copper pours for the high-current nets of `pcb`.
///
/// Policy then geometry: [`decide_pours`] picks the nets and layers,
/// [`synthesize_outlines`] draws them. Returns zones to *add* to the board —
/// nets that already own a pour on a layer are left alone, so running this on a
/// board with hand-authored planes complements them rather than duplicating them.
pub fn synthesize_pours(pcb: &Pcb, policy: &PourPolicy, nets_filter: &[String]) -> Vec<Zone> {
    if !policy.enabled {
        return Vec::new();
    }
    let candidates = decide_pours(pcb, policy, nets_filter);
    if candidates.is_empty() {
        return Vec::new();
    }
    synthesize_outlines(pcb, &candidates, policy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::ecad::*;

    fn rules() -> DesignRules {
        DesignRules {
            default_rules: NetClassRules {
                name: "Default".into(),
                trace_width: 0.25,
                clearance: 0.2,
                via_diameter: 0.8,
                via_drill: 0.4,
                diff_pair_gap: None,
                diff_pair_width: None,
            },
            class_rules: vec![],
            net_class_assignments: Default::default(),
            edge_clearance: 0.5,
            hole_to_hole: 0.5,
            min_annular_ring: 0.15,
            min_drill: 0.2,
        }
    }

    fn board(w: f64, h: f64) -> Pcb {
        Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(w, 0.0),
                    Vec2::new(w, h),
                    Vec2::new(0.0, h),
                ],
                cutouts: vec![],
                thickness: 1.6,
            },
            stackup: LayerStackup {
                layers: vec![
                    StackupLayer {
                        layer: PcbLayer::FCu,
                        copper_thickness: Some(0.035),
                        dielectric_thickness: Some(1.5),
                        dielectric_er: Some(4.5),
                        material: Some("FR4".into()),
                    },
                    StackupLayer {
                        layer: PcbLayer::BCu,
                        copper_thickness: Some(0.035),
                        dielectric_thickness: None,
                        dielectric_er: None,
                        material: None,
                    },
                ],
            },
            nets: vec![],
            rules: rules(),
            footprints: vec![],
            traces: vec![],
            trace_arcs: vec![],
            vias: vec![],
            zones: vec![],
            keepouts: vec![],
            net_ties: vec![],
        }
    }

    /// A one-pad footprint at `pos` on `layer`, carrying `net`.
    fn pad_fp(reference: &str, pos: Vec2, net: &str, layer: PcbLayer) -> Footprint {
        Footprint {
            reference: reference.into(),
            value: String::new(),
            footprint_name: "pad".into(),
            position: pos,
            rotation: 0.0,
            front: layer == PcbLayer::FCu,
            pads: vec![Pad {
                number: "1".into(),
                pad_type: PadType::SMD,
                shape: PadShape::Rect {
                    width: 1.0,
                    height: 1.0,
                },
                position: Vec2::new(0.0, 0.0),
                rotation: 0.0,
                drill: None,
                net: Some(net.into()),
                layers: vec![layer],
            }],
            graphics: vec![],
            model_3d: None,
            properties: Default::default(),
        }
    }

    /// `n` pads of `net` in a row starting at `x0`, spaced 2mm.
    fn pad_row(pcb: &mut Pcb, net: &str, n: usize, x0: f64, y: f64, layer: PcbLayer) {
        for i in 0..n {
            pcb.footprints.push(pad_fp(
                &format!("{net}{i}"),
                Vec2::new(x0 + 2.0 * i as f64, y),
                net,
                layer,
            ));
        }
    }

    // --- ampacity -----------------------------------------------------------

    #[test]
    fn ipc2221_matches_the_shared_sizing_model() {
        // Ground truth: the `size_trace_for_current` MCP tool's own closed form
        // evaluated at 1 oz / 10 C rise. If these drift apart, the router and
        // the tool are answering "how wide for this current?" differently.
        for (current, outer, inner) in [
            (
                1.0,
                0.302_112_880_148_438_8_f64,
                0.785_928_482_810_401_7_f64,
            ),
            (10.0, 7.235_683_901_965_999_5, 18.823_196_377_372_895),
        ] {
            let w = ipc2221_width_mm(current, 1.0, 10.0, false);
            assert!(
                (w - outer).abs() < 1e-9,
                "{current} A outer: expected {outer}, got {w}"
            );
            let wi = ipc2221_width_mm(current, 1.0, 10.0, true);
            assert!(
                (wi - inner).abs() < 1e-9,
                "{current} A inner: expected {inner}, got {wi}"
            );
            // Buried copper derates: the same current needs ~2.6x the width.
            assert!(wi > w * 2.0);
        }
    }

    #[test]
    fn ampacity_round_trips_through_its_inverse() {
        for current in [0.5, 2.0, 15.0] {
            let w = ipc2221_width_mm(current, 1.0, 10.0, false);
            let back = ipc2221_current_a(w, 1.0, 10.0, false);
            assert!(
                (back - current).abs() < current * 1e-6,
                "{current} A -> {w} mm -> {back} A"
            );
        }
        assert_eq!(ipc2221_width_mm(0.0, 1.0, 10.0, false), 0.0);
        assert_eq!(ipc2221_current_a(-1.0, 1.0, 10.0, false), 0.0);
    }

    // --- policy -------------------------------------------------------------

    #[test]
    fn declared_current_beyond_a_trace_selects_a_pour() {
        let mut pcb = board(40.0, 30.0);
        pad_row(&mut pcb, "MOTOR_A", 8, 5.0, 10.0, PcbLayer::FCu);
        let mut policy = PourPolicy::default();
        policy.net_current_a.insert("MOTOR_A".into(), 25.0);
        let d = decide_pours(&pcb, &policy, &[]);
        assert!(!d.is_empty(), "25 A must not be routed as a 0.25 mm trace");
        assert!(matches!(
            d[0].reason,
            PourReason::DeclaredCurrent { current_a, .. } if current_a == 25.0
        ));
    }

    #[test]
    fn declared_current_that_fits_a_trace_is_left_alone() {
        let mut pcb = board(40.0, 30.0);
        // A power-rail *name* — the fallback would fire if the declared current
        // did not take precedence.
        pad_row(&mut pcb, "+3V3", 8, 5.0, 10.0, PcbLayer::FCu);
        let mut policy = PourPolicy::default();
        policy.net_current_a.insert("+3V3".into(), 0.2);
        assert!(
            decide_pours(&pcb, &policy, &[]).is_empty(),
            "0.2 A fits in a trace; a declared current is authoritative"
        );
    }

    #[test]
    fn wide_net_class_selects_a_pour_without_a_declared_current() {
        let mut pcb = board(40.0, 30.0);
        pad_row(&mut pcb, "BUS", 8, 5.0, 10.0, PcbLayer::FCu);
        pcb.rules.class_rules.push(NetClassRules {
            name: "power".into(),
            trace_width: 1.5,
            clearance: 0.3,
            via_diameter: 0.8,
            via_drill: 0.4,
            diff_pair_gap: None,
            diff_pair_width: None,
        });
        pcb.rules
            .net_class_assignments
            .insert("power".into(), vec!["BUS".into()]);
        let d = decide_pours(&pcb, &PourPolicy::default(), &[]);
        assert!(matches!(d[0].reason, PourReason::ClassWidth { .. }));
    }

    #[test]
    fn power_rail_name_is_the_fallback_and_respects_min_pads() {
        let mut pcb = board(40.0, 30.0);
        pad_row(&mut pcb, "GND", 8, 5.0, 10.0, PcbLayer::FCu);
        pad_row(&mut pcb, "SIG", 8, 5.0, 20.0, PcbLayer::FCu);
        let d = decide_pours(&pcb, &PourPolicy::default(), &[]);
        assert!(d.iter().any(|c| c.net == "GND"));
        assert!(
            !d.iter().any(|c| c.net == "SIG"),
            "a signal net is not a plane"
        );

        // Below min_pads nothing is poured — small point-to-point boards keep
        // the router's traces.
        let mut small = board(40.0, 30.0);
        pad_row(&mut small, "GND", 3, 5.0, 10.0, PcbLayer::FCu);
        assert!(decide_pours(&small, &PourPolicy::default(), &[]).is_empty());
    }

    #[test]
    fn nets_filter_and_disable_switch_are_honored() {
        let mut pcb = board(40.0, 30.0);
        pad_row(&mut pcb, "GND", 8, 5.0, 10.0, PcbLayer::FCu);
        pad_row(&mut pcb, "+5V", 8, 5.0, 20.0, PcbLayer::FCu);
        let all = decide_pours(&pcb, &PourPolicy::default(), &[]);
        assert_eq!(
            all.iter().map(|c| c.net.as_str()).collect::<BTreeSet<_>>(),
            BTreeSet::from(["+5V", "GND"])
        );
        let only_gnd = decide_pours(&pcb, &PourPolicy::default(), &["GND".into()]);
        assert!(only_gnd.iter().all(|c| c.net == "GND"));

        let off = PourPolicy {
            enabled: false,
            ..Default::default()
        };
        assert!(decide_pours(&pcb, &off, &[]).is_empty());
        assert!(synthesize_pours(&pcb, &off, &[]).is_empty());
    }

    #[test]
    fn an_existing_zone_is_complemented_never_duplicated() {
        let mut pcb = board(40.0, 30.0);
        pad_row(&mut pcb, "GND", 8, 5.0, 10.0, PcbLayer::FCu);
        pad_row(&mut pcb, "GND_B", 8, 5.0, 20.0, PcbLayer::BCu);
        pcb.zones.push(Zone {
            outline: vec![
                Vec2::new(0.5, 0.5),
                Vec2::new(39.5, 0.5),
                Vec2::new(39.5, 29.5),
                Vec2::new(0.5, 29.5),
            ],
            holes: vec![],
            net: "GND".into(),
            layer: PcbLayer::FCu,
            clearance: 0.2,
            min_area: 0.0,
            fill_type: ZoneFillType::Solid,
            thermal_relief: ThermalReliefStyle::Relief,
            thermal_gap: None,
            thermal_spoke_width: None,
            priority: 0,
        });
        let d = decide_pours(&pcb, &PourPolicy::default(), &[]);
        assert!(
            !d.iter().any(|c| c.net == "GND" && c.layer == PcbLayer::FCu),
            "GND already owns FCu: {d:?}"
        );
    }

    // --- geometry -----------------------------------------------------------

    #[test]
    fn a_local_cluster_becomes_a_margined_region_not_the_whole_layer() {
        let pcb = board(60.0, 60.0);
        let usable = usable_area(&pcb);
        let pts = vec![
            Vec2::new(10.0, 10.0),
            Vec2::new(14.0, 10.0),
            Vec2::new(14.0, 14.0),
            Vec2::new(10.0, 14.0),
        ];
        let region = synthesize_region(&pts, &usable, 1.0, 0.55);
        assert_eq!(region.len(), 1);
        // 4x4 cluster + 1mm margin on each side = 6x6.
        assert!(
            (region[0].area() - 36.0).abs() < 0.5,
            "expected a ~36 mm^2 region, got {}",
            region[0].area()
        );
        // The cluster and its margin are inside; a far corner is not.
        assert!(poly2d::contains_point(&region[0], Point2::new(12.0, 12.0)));
        assert!(poly2d::contains_point(&region[0], Point2::new(9.5, 9.5)));
        assert!(!poly2d::contains_point(&region[0], Point2::new(50.0, 50.0)));
    }

    #[test]
    fn a_spread_net_promotes_itself_to_a_whole_layer_flood() {
        let pcb = board(60.0, 60.0);
        let usable = usable_area(&pcb);
        let usable_a = total_area(&usable);
        let pts = vec![
            Vec2::new(3.0, 3.0),
            Vec2::new(57.0, 3.0),
            Vec2::new(57.0, 57.0),
            Vec2::new(3.0, 57.0),
        ];
        let region = synthesize_region(&pts, &usable, 1.0, 0.55);
        assert!(
            (total_area(&region) - usable_a).abs() < 1e-6,
            "a board-spanning cluster should flood the whole usable layer"
        );
    }

    #[test]
    fn a_region_never_crosses_the_board_edge_or_a_cutout() {
        let mut pcb = board(30.0, 30.0);
        pcb.outline.cutouts.push(vec![
            Vec2::new(12.0, 12.0),
            Vec2::new(18.0, 12.0),
            Vec2::new(18.0, 18.0),
            Vec2::new(12.0, 18.0),
        ]);
        let usable = usable_area(&pcb);
        // Pads spanning the whole board: the flood must still respect the
        // edge-clearance ring and the cutout's clearance ring.
        let pts = vec![Vec2::new(1.0, 1.0), Vec2::new(29.0, 29.0)];
        let region = synthesize_region(&pts, &usable, 2.0, 2.0);
        assert!(!region.is_empty());
        for p in &region {
            // Outside the board and inside the cutout are both forbidden.
            assert!(!poly2d::contains_point(p, Point2::new(0.1, 15.0)));
            assert!(!poly2d::contains_point(p, Point2::new(15.0, 15.0)));
        }
        // 30x30 eroded by 0.5 = 29x29 (841), minus the 6x6 cutout grown by 0.5
        // (49) = ~792.
        let a = total_area(&region);
        assert!((a - 792.0).abs() < 6.0, "usable area {a} mm^2");
    }

    #[test]
    fn a_net_of_few_large_pads_qualifies_on_pad_area() {
        // A battery input: two big terminals, well under `min_pads`. Pad count
        // alone would leave the highest-current net on the board un-poured.
        let mut pcb = board(40.0, 30.0);
        for (i, x) in [8.0_f64, 30.0].iter().enumerate() {
            let mut fp = pad_fp(
                &format!("J{i}"),
                Vec2::new(*x, 15.0),
                "V_SUPPLY",
                PcbLayer::FCu,
            );
            fp.pads[0].shape = PadShape::Rect {
                width: 8.0,
                height: 11.0,
            };
            pcb.footprints.push(fp);
        }
        // A few more so the net is a network, still below min_pads.
        pad_row(&mut pcb, "V_SUPPLY", 3, 14.0, 8.0, PcbLayer::FCu);
        // Something on the back copper, so BCu is not a free plane layer.
        pcb.footprints
            .push(pad_fp("SIGB", Vec2::new(20.0, 25.0), "SIG", PcbLayer::BCu));
        let d = decide_pours(&pcb, &PourPolicy::default(), &[]);
        assert!(
            d.iter().any(|c| c.net == "V_SUPPLY"),
            "5 pads but 179 mm^2 of pad copper is a power net: {d:?}"
        );

        // The same net with small pads stays below both gates.
        let mut small = board(40.0, 30.0);
        pad_row(&mut small, "V_SUPPLY", 5, 8.0, 15.0, PcbLayer::FCu);
        assert!(decide_pours(&small, &PourPolicy::default(), &[]).is_empty());
    }

    #[test]
    fn a_board_spanning_flood_takes_its_layer_exclusively() {
        // Two rails, each spread across the board, on a three-layer stackup.
        // Both would flood a whole layer, and two whole-layer floods cannot
        // share one — so they end up on different layers, most-distributed net
        // first.
        let mut pcb = board(40.0, 30.0);
        pcb.stackup.layers = [PcbLayer::FCu, PcbLayer::In1Cu, PcbLayer::BCu]
            .into_iter()
            .map(|layer| StackupLayer {
                layer,
                copper_thickness: Some(0.035),
                dielectric_thickness: Some(0.2),
                dielectric_er: Some(4.5),
                material: Some("FR4".into()),
            })
            .collect();
        for (i, x) in [3.0_f64, 20.0, 37.0].iter().enumerate() {
            for (j, y) in [3.0_f64, 15.0, 27.0].iter().enumerate() {
                pcb.footprints.push(pad_fp(
                    &format!("G{i}{j}"),
                    Vec2::new(*x, *y),
                    "GND",
                    PcbLayer::FCu,
                ));
                if i + j < 4 {
                    pcb.footprints.push(pad_fp(
                        &format!("P{i}{j}"),
                        Vec2::new(x + 1.5, y + 1.5),
                        "+5V",
                        PcbLayer::BCu,
                    ));
                }
            }
        }
        let d = decide_pours(&pcb, &PourPolicy::default(), &[]);
        let gnd = d.iter().find(|c| c.net == "GND").expect("GND poured");
        let v5 = d.iter().find(|c| c.net == "+5V").expect("+5V poured");
        assert_eq!(
            gnd.layer,
            PcbLayer::FCu,
            "GND's pads are all on FCu: flooding it costs zero stitching vias"
        );
        assert_ne!(v5.layer, gnd.layer, "a board-spanning flood owns its layer");
    }

    #[test]
    fn the_layer_chosen_is_the_one_that_needs_the_fewest_stitches() {
        // A bare inner layer looks like a tempting dedicated plane, but every
        // pad on another layer then costs a stitching via. The layer the net
        // already lives on wins.
        let mut pcb = board(40.0, 30.0);
        pcb.stackup.layers = [PcbLayer::FCu, PcbLayer::In1Cu, PcbLayer::BCu]
            .into_iter()
            .map(|layer| StackupLayer {
                layer,
                copper_thickness: Some(0.035),
                dielectric_thickness: Some(0.2),
                dielectric_er: Some(4.5),
                material: Some("FR4".into()),
            })
            .collect();
        pad_row(&mut pcb, "GND", 10, 4.0, 8.0, PcbLayer::FCu);
        pcb.footprints
            .push(pad_fp("GB", Vec2::new(30.0, 20.0), "GND", PcbLayer::BCu));
        let d = decide_pours(&pcb, &PourPolicy::default(), &[]);
        let gnd = d.iter().find(|c| c.net == "GND").expect("GND poured");
        assert_eq!(gnd.layer, PcbLayer::FCu, "10 pads flood, 1 stitches");
        assert_eq!(gnd.pads_on_layer, 10);
    }

    #[test]
    fn geometry_refuses_to_carve_one_whole_layer_flood_out_of_another() {
        // The geometry pass is the backstop when two candidates land on one
        // layer anyway: the more distributed net keeps it rather than both
        // ending up as debris.
        let mut pcb = board(40.0, 30.0);
        pad_row(&mut pcb, "GND", 12, 3.0, 5.0, PcbLayer::FCu);
        pcb.footprints
            .push(pad_fp("GNDX", Vec2::new(36.0, 26.0), "GND", PcbLayer::FCu));
        pad_row(&mut pcb, "+5V", 8, 3.0, 15.0, PcbLayer::FCu);
        pcb.footprints
            .push(pad_fp("V5X", Vec2::new(36.0, 24.0), "+5V", PcbLayer::FCu));
        let reason = PourReason::PowerRailName {
            pads: 0,
            traced_current_a: 0.0,
        };
        let cands = vec![
            PourCandidate {
                net: "GND".into(),
                layer: PcbLayer::FCu,
                reason: reason.clone(),
                pads_on_layer: 13,
                demand_pads: 13,
            },
            PourCandidate {
                net: "+5V".into(),
                layer: PcbLayer::FCu,
                reason,
                pads_on_layer: 9,
                demand_pads: 9,
            },
        ];
        let zones = synthesize_outlines(&pcb, &cands, &PourPolicy::default());
        let nets: BTreeSet<&str> = zones.iter().map(|z| z.net.as_str()).collect();
        assert_eq!(
            nets,
            BTreeSet::from(["GND"]),
            "GND has more pads, so it keeps the layer: {zones:?}"
        );
    }

    #[test]
    fn a_candidate_carved_to_scraps_by_an_existing_zone_is_dropped() {
        // A hand-authored plane already owns most of the layer. What is left is
        // not a plane for the second net, and some of its pads cannot reach it —
        // so the net keeps its traces rather than shipping stranded copper.
        let mut pcb = board(60.0, 20.0);
        pad_row(&mut pcb, "+5V", 8, 4.0, 10.0, PcbLayer::FCu);
        pcb.footprints
            .push(pad_fp("SIGB", Vec2::new(30.0, 15.0), "SIG", PcbLayer::BCu));
        pcb.zones.push(Zone {
            outline: vec![
                Vec2::new(6.0, 0.5),
                Vec2::new(59.5, 0.5),
                Vec2::new(59.5, 19.5),
                Vec2::new(6.0, 19.5),
            ],
            holes: vec![],
            net: "GND".into(),
            layer: PcbLayer::FCu,
            clearance: 0.2,
            min_area: 0.0,
            fill_type: ZoneFillType::Solid,
            thermal_relief: ThermalReliefStyle::Relief,
            thermal_gap: None,
            thermal_spoke_width: None,
            priority: 0,
        });
        assert!(
            decide_pours(&pcb, &PourPolicy::default(), &[])
                .iter()
                .any(|c| c.net == "+5V"),
            "the policy still wants it"
        );
        assert!(
            synthesize_pours(&pcb, &PourPolicy::default(), &[]).is_empty(),
            "but the geometry cannot give it a plane every pad reaches"
        );
    }

    #[test]
    fn two_pours_on_one_layer_do_not_overlap() {
        let mut pcb = board(60.0, 30.0);
        pad_row(&mut pcb, "+12V", 6, 4.0, 8.0, PcbLayer::FCu);
        pad_row(&mut pcb, "+5V", 6, 40.0, 8.0, PcbLayer::FCu);
        // Wide margin so the raw boxes would overlap without the claim pass.
        let policy = PourPolicy {
            margin_mm: 12.0,
            whole_layer_fraction: 2.0,
            ..Default::default()
        };
        let zones = synthesize_pours(&pcb, &policy, &[]);
        assert!(zones.len() >= 2, "both rails should pour: {zones:?}");
        let polys: Vec<(String, Poly)> = zones
            .iter()
            .map(|z| {
                (
                    z.net.clone(),
                    Poly {
                        outer: ccw(ring_to_pts(&z.outline)),
                        holes: z.holes.iter().map(|h| cw(ring_to_pts(h))).collect(),
                    },
                )
            })
            .collect();
        for (i, (net_a, a)) in polys.iter().enumerate() {
            for (net_b, b) in polys.iter().skip(i + 1) {
                if net_a == net_b {
                    continue;
                }
                let overlap =
                    total_area(&intersect(std::slice::from_ref(a), std::slice::from_ref(b)));
                // Only the pad-disc give-back may overlap, and each pad disc is
                // under 2 mm^2 here.
                assert!(
                    overlap < 6.0,
                    "{net_a} and {net_b} overlap by {overlap} mm^2"
                );
            }
        }
    }

    #[test]
    fn every_pad_of_a_poured_net_is_inside_its_pour() {
        let mut pcb = board(60.0, 40.0);
        pad_row(&mut pcb, "GND", 8, 5.0, 8.0, PcbLayer::FCu);
        // An off-layer pad: it reaches the plane through a stitching via, which
        // has to land inside the pour.
        pcb.footprints
            .push(pad_fp("GNDB", Vec2::new(30.0, 30.0), "GND", PcbLayer::BCu));
        let zones = synthesize_pours(&pcb, &PourPolicy::default(), &[]);
        assert!(!zones.is_empty());
        let fcu: Vec<Poly> = zones
            .iter()
            .filter(|z| z.layer == PcbLayer::FCu)
            .map(|z| Poly::new(ccw(ring_to_pts(&z.outline))))
            .collect();
        assert!(!fcu.is_empty(), "GND should pour on FCu");
        for fp in &pcb.footprints {
            for pad in &fp.pads {
                let c = crate::geometry::pad_world_position(fp, pad);
                assert!(
                    fcu.iter()
                        .any(|p| poly2d::contains_point(p, Point2::new(c.x, c.y))),
                    "pad {} at ({}, {}) is outside the GND pour",
                    fp.reference,
                    c.x,
                    c.y
                );
            }
        }
    }

    #[test]
    fn synthesized_zones_fill_and_carry_their_net() {
        let mut pcb = board(40.0, 30.0);
        pad_row(&mut pcb, "GND", 8, 5.0, 10.0, PcbLayer::FCu);
        let zones = synthesize_pours(&pcb, &PourPolicy::default(), &[]);
        assert!(!zones.is_empty());
        pcb.zones = zones;
        let filled = crate::copper_pour::fill_zones(&pcb);
        assert_eq!(filled.len(), pcb.zones.len());
        for f in &filled {
            assert_eq!(f.net, "GND");
            assert!(!f.polygons.is_empty(), "a synthesized zone must fill");
        }
    }
}
