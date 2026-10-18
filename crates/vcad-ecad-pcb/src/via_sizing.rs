//! Per-span via sizing — the one choke point every via site consults.
//!
//! A board's via is not one size. An HDI board (the CM5: 1-8-1 plus
//! second-level microvias) carries laser microvias (0.21/0.12) on adjacent
//! layer pairs and mechanically drilled through / blind / buried vias
//! (0.4/0.15) everywhere else; a microvia-sized barrel through eight
//! dielectrics is undrillable, a through-sized disc on every BGA ball does not
//! fit. [`ViaSizing::spec`] picks the size from the SPAN:
//!
//! - **microvia** — two adjacent copper layers that are not the whole stack
//!   (a 2-layer board's F–B via is a through via), at the net class's via size
//!   when that meets the microvia minimums (or the caller's microvia override);
//! - **through / blind / buried** — everything else, at the class size raised
//!   to the board's mechanical floors: `drill >= min_drill`,
//!   `diameter >= drill + 2 * min_annular_ring`.
//!
//! On a board whose class via already meets the mechanical floors (most 2- and
//! 4-layer boards) every span gets the class size — nothing changes. The
//! session ([`crate::session::RouteSession::via_spec`]) and the board
//! write-back both call the same table, so a via is probed, committed and
//! written at one size.

use std::collections::HashMap;

use vcad_ir::ecad::{Pcb, PcbLayer, ViaType};

/// The geometry and construction type of one via.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViaSpec {
    /// Pad diameter (mm).
    pub diameter: f64,
    /// Drill diameter (mm).
    pub drill: f64,
    /// Construction type.
    pub via_type: ViaType,
}

impl ViaSpec {
    /// Pad radius (mm).
    pub fn r(&self) -> f64 {
        self.diameter / 2.0
    }
}

/// Per-net, per-span via size table for one board (see the module docs).
#[derive(Debug, Clone)]
pub struct ViaSizing {
    copper: Vec<PcbLayer>,
    class: HashMap<String, (f64, f64)>,
    default: (f64, f64),
    min_drill: f64,
    min_ring: f64,
    micro_floor: (f64, f64),
    micro_override: Option<(f64, f64)>,
}

impl ViaSizing {
    /// Build the table from `pcb`'s rules and stackup.
    pub fn from_pcb(pcb: &Pcb) -> Self {
        let rules = &pcb.rules;
        let mut class = HashMap::new();
        for (name, nets) in &rules.net_class_assignments {
            if let Some(c) = rules.class_rules.iter().find(|c| c.name == *name) {
                for n in nets {
                    class.insert(n.clone(), (c.via_diameter, c.via_drill));
                }
            }
        }
        let copper: Vec<PcbLayer> = PcbLayer::COPPER_STACK
            .iter()
            .copied()
            .filter(|l| {
                pcb.stackup.layers.is_empty() || pcb.stackup.layers.iter().any(|s| s.layer == *l)
            })
            .collect();
        Self {
            copper: if copper.is_empty() {
                vec![PcbLayer::FCu, PcbLayer::BCu]
            } else {
                copper
            },
            class,
            default: (
                rules.default_rules.via_diameter,
                rules.default_rules.via_drill,
            ),
            min_drill: rules.min_drill,
            min_ring: rules.min_annular_ring,
            micro_floor: (rules.min_microvia_diameter, rules.min_microvia_drill),
            // The router's microvia knob (`RouteOptions::microvia`), set for
            // the duration of a routing pass.
            micro_override: crate::router::maze::microvia(),
        }
    }

    /// Size every microvia at `size` (`(diameter, drill)`) instead of the
    /// class via — the router's explicit microvia knob.
    pub fn with_microvia(mut self, size: Option<(f64, f64)>) -> Self {
        self.micro_override = size;
        self
    }

    /// The class `(diameter, drill)` of `net` (board default when unclassed).
    pub fn class_size(&self, net: &str) -> (f64, f64) {
        self.class.get(net).copied().unwrap_or(self.default)
    }

    fn idx(&self, l: PcbLayer) -> Option<usize> {
        self.copper.iter().position(|&c| c == l)
    }

    /// Is `a`–`b` a microvia span: two adjacent copper layers, not the whole
    /// stack.
    pub fn is_micro_span(&self, a: PcbLayer, b: PcbLayer) -> bool {
        let (Some(i), Some(j)) = (self.idx(a), self.idx(b)) else {
            return false;
        };
        i.abs_diff(j) == 1 && self.copper.len() > 2
    }

    /// The microvia size for `net`, if microvias are usable: the override, or
    /// the class via when it meets the microvia minimums.
    pub fn micro_size(&self, net: &str) -> Option<(f64, f64)> {
        if self.copper.len() <= 2 {
            return None;
        }
        let (d, dr) = self.micro_override.unwrap_or_else(|| self.class_size(net));
        (dr >= self.micro_floor.1 - 1e-9 && d >= self.micro_floor.0 - 1e-9).then_some((d, dr))
    }

    /// The mechanically drilled (through / blind / buried) size for `net`:
    /// the class size raised to the drill and annular-ring floors.
    pub fn drilled_size(&self, net: &str) -> (f64, f64) {
        let (d, dr) = self.class_size(net);
        let drill = dr.max(self.min_drill);
        (d.max(drill + 2.0 * self.min_ring), drill)
    }

    /// The via for `net` spanning copper layers `a`..`b` (either order).
    pub fn spec(&self, net: &str, a: PcbLayer, b: PcbLayer) -> ViaSpec {
        if self.is_micro_span(a, b) {
            if let Some((diameter, drill)) = self.micro_size(net) {
                return ViaSpec {
                    diameter,
                    drill,
                    via_type: ViaType::Micro,
                };
            }
        }
        let (diameter, drill) = self.drilled_size(net);
        let outer = |l: PcbLayer| self.copper.first() == Some(&l) || self.copper.last() == Some(&l);
        ViaSpec {
            diameter,
            drill,
            via_type: if outer(a) && outer(b) {
                ViaType::Through
            } else {
                ViaType::Blind
            },
        }
    }

    /// The largest via `net` can get on any span (the through size): the
    /// conservative radius for a site that reserves room before it knows the
    /// span.
    pub fn max_spec(&self, net: &str) -> ViaSpec {
        let (Some(&a), Some(&b)) = (self.copper.first(), self.copper.last()) else {
            unreachable!("copper stack is never empty")
        };
        let t = self.spec(net, a, b);
        match self.micro_size(net) {
            Some((d, dr)) if d > t.diameter => ViaSpec {
                diameter: d,
                drill: dr.max(t.drill),
                via_type: t.via_type,
            },
            _ => t,
        }
    }

    /// One line for the run log: the sizes this board's vias get.
    pub fn describe(&self) -> String {
        let (dd, ddr) = self.drilled_size("");
        match self.micro_size("") {
            Some((md, mdr)) => format!(
                "via sizing: microvia (adjacent pair) {md}/{mdr}, through/blind/buried {dd:.3}/{ddr:.3} \
                 (class {}/{}, floors drill {} ring {}, micro floors {}/{})",
                self.default.0, self.default.1, self.min_drill, self.min_ring,
                self.micro_floor.0, self.micro_floor.1
            ),
            None => format!(
                "via sizing: every span {dd:.3}/{ddr:.3} (class {}/{}, floors drill {} ring {}; no microvias)",
                self.default.0, self.default.1, self.min_drill, self.min_ring
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board(layers: usize, via: (f64, f64), min_drill: f64, ring: f64) -> ViaSizing {
        let copper: Vec<PcbLayer> = PcbLayer::COPPER_STACK
            .iter()
            .copied()
            .filter(|&l| l != PcbLayer::BCu)
            .take(layers - 1)
            .chain(std::iter::once(PcbLayer::BCu))
            .collect();
        ViaSizing {
            copper,
            class: HashMap::new(),
            default: via,
            min_drill,
            min_ring: ring,
            micro_floor: (0.2, 0.1),
            micro_override: None,
        }
    }

    #[test]
    fn hdi_board_splits_micro_and_drilled() {
        // CM5: class 0.21/0.12, inferred floors 0.15 drill / 0.125 ring.
        let s = board(10, (0.21, 0.12), 0.15, 0.125);
        let m = s.spec("X", PcbLayer::FCu, PcbLayer::In1Cu);
        assert_eq!(m.via_type, ViaType::Micro);
        assert_eq!((m.diameter, m.drill), (0.21, 0.12));
        let m2 = s.spec("X", PcbLayer::In8Cu, PcbLayer::In7Cu);
        assert_eq!(m2.via_type, ViaType::Micro, "second-level microvia");
        let t = s.spec("X", PcbLayer::FCu, PcbLayer::BCu);
        assert_eq!(t.via_type, ViaType::Through);
        assert!((t.drill - 0.15).abs() < 1e-12 && (t.diameter - 0.40).abs() < 1e-12);
        let b = s.spec("X", PcbLayer::In1Cu, PcbLayer::In8Cu);
        assert_eq!(b.via_type, ViaType::Blind);
        assert!((b.diameter - 0.40).abs() < 1e-12);
        assert!((s.max_spec("X").diameter - 0.40).abs() < 1e-12);
    }

    #[test]
    fn ordinary_board_keeps_class_size_everywhere() {
        let s = board(4, (0.6, 0.3), 0.2, 0.13);
        for (a, b) in [
            (PcbLayer::FCu, PcbLayer::BCu),
            (PcbLayer::FCu, PcbLayer::In1Cu),
            (PcbLayer::In1Cu, PcbLayer::In2Cu),
        ] {
            let v = s.spec("N", a, b);
            assert_eq!((v.diameter, v.drill), (0.6, 0.3), "{a:?}-{b:?}");
        }
        // A 2-layer board's F-B via is a through via, never a microvia.
        let s2 = board(2, (0.6, 0.3), 0.2, 0.13);
        assert_eq!(
            s2.spec("N", PcbLayer::FCu, PcbLayer::BCu).via_type,
            ViaType::Through
        );
    }

    #[test]
    fn class_below_micro_floor_gets_no_microvias() {
        let s = board(6, (0.18, 0.08), 0.2, 0.13);
        let v = s.spec("N", PcbLayer::FCu, PcbLayer::In1Cu);
        assert_ne!(v.via_type, ViaType::Micro);
        assert!(v.drill >= 0.2 && v.diameter >= 0.46 - 1e-12);
    }
}
