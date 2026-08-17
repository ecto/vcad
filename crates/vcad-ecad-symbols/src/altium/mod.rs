//! Altium Designer PCB import.
//!
//! Three entry points, all landing on the same [`Pcb`] IR the KiCad and Eagle
//! importers produce:
//!
//! * [`parse_altium_ascii_pcb`] — an ASCII-exported `.PcbDoc`
//!   (*File ▸ Save As ▸ PCB ASCII*). Plain text, fully specified, and the
//!   recommended path.
//! * [`parse_altium_pcbdoc`] — a native binary `.PcbDoc`. Altium stores these
//!   as an OLE compound file (CFB) whose per-object streams are a mix of
//!   `|KEY=VALUE|` parameter blocks (nets, components, board, rules — the same
//!   vocabulary the ASCII export uses) and packed fixed-layout binary records
//!   (tracks, arcs, vias, pads, fills).
//! * [`parse_altium_pcblib`] — a `.PcbLib` footprint library, in either
//!   flavour, yielding a [`FootprintLib`].
//!
//! # Why the binary path fails closed
//!
//! The binary record layouts are not published by Altium; they are reconstructed
//! from the open-source ecosystem (KiCad's Altium plugin, `altium2kicad`). A
//! wrong field offset does not error — it silently yields plausible-looking
//! coordinates in the wrong place, which is the worst failure mode a CAD
//! importer has. So every decoded primitive is validated against plausible
//! board extents before it is accepted, and a stream whose records do not
//! validate aborts the whole import with an error pointing at the ASCII
//! export. Geometry is never silently dropped or silently invented.
//!
//! That gate has already earned its keep: the first version of this importer
//! used the wrong framing for `Pads6` and refused every real file rather than
//! importing garbage.
//!
//! The layouts are validated against real open-hardware Altium projects
//! (LimeSDR-Mini v1.0–v1.3 boards, panels and libraries; ODrive v2/v3),
//! cross-checked against the fabrication data those projects ship — board
//! outline, panel size, layer count, copper foil thickness and dielectric Er
//! all agree with the vendors' own readme files.
//!
//! # Coordinates
//!
//! Altium's internal unit is 1/10000 mil (2.54 nm); ASCII files write
//! human units (`1000mil`, `12.7mm`, or a bare number meaning mils). Both are
//! normalised to millimetres here. Altium's Y axis already points up, matching
//! the vcad convention, so no flip is applied.

mod ascii;
mod binary;

use std::collections::HashMap;

use vcad_ir::ecad::*;
use vcad_ir::Vec2;

use crate::kicad_mod::{FootprintDef, FootprintLib, GraphicDef, PadDef};

pub use ascii::parse_altium_ascii_pcb;
pub use binary::{parse_altium_pcbdoc, parse_altium_pcblib};

/// Millimetres per Altium internal unit (1/10000 mil).
pub(crate) const MM_PER_INTERNAL: f64 = 2.54e-6;

// ============================================================================
// Record model
// ============================================================================

/// One `|KEY=VALUE|…` Altium record, keys upper-cased.
///
/// Both frontends normalise to this: the ASCII reader parses it directly, and
/// the binary reader synthesises it from packed records using the same key
/// names, so there is a single record → [`Pcb`] builder to keep correct.
#[derive(Debug, Clone, Default)]
pub(crate) struct Record {
    fields: HashMap<String, String>,
}

impl Record {
    pub(crate) fn from_pairs(pairs: impl IntoIterator<Item = (String, String)>) -> Self {
        Record {
            fields: pairs
                .into_iter()
                .map(|(k, v)| (k.to_uppercase(), v))
                .collect(),
        }
    }

    pub(crate) fn set(&mut self, key: &str, value: impl Into<String>) {
        self.fields.insert(key.to_uppercase(), value.into());
    }

    pub(crate) fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(|s| s.as_str())
    }

    /// `RECORD=` kind, upper-cased (`BOARD`, `TRACK`, `PAD`, …).
    pub(crate) fn kind(&self) -> &str {
        self.get("RECORD").unwrap_or("")
    }

    /// A length field in millimetres, parsing Altium's unit suffixes.
    pub(crate) fn len_mm(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(parse_length_mm)
    }

    pub(crate) fn len_mm_or(&self, key: &str, default: f64) -> f64 {
        self.len_mm(key).unwrap_or(default)
    }

    pub(crate) fn num(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(|v| v.trim().parse().ok())
    }

    pub(crate) fn int(&self, key: &str) -> Option<i64> {
        self.get(key)
            .and_then(|v| v.trim().parse::<f64>().ok())
            .map(|v| v as i64)
    }

    pub(crate) fn flag(&self, key: &str) -> Option<bool> {
        self.get(key).map(|v| {
            let v = v.trim();
            v.eq_ignore_ascii_case("true") || v == "1" || v.eq_ignore_ascii_case("t")
        })
    }

    pub(crate) fn point_mm(&self, x_key: &str, y_key: &str) -> Option<Vec2> {
        Some(Vec2::new(self.len_mm(x_key)?, self.len_mm(y_key)?))
    }
}

/// Parse an Altium ASCII length into millimetres.
///
/// Accepts `12.7mm`, `1000mil`, `500.0` (bare — Altium ASCII writes mils), and
/// tolerates surrounding whitespace. Returns `None` for anything unparseable
/// rather than guessing a magnitude.
pub(crate) fn parse_length_mm(raw: &str) -> Option<f64> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    let (num, scale) = if let Some(rest) = lower.strip_suffix("mil") {
        (rest, 0.0254)
    } else if let Some(rest) = lower.strip_suffix("mm") {
        (rest, 1.0)
    } else if let Some(rest) = lower.strip_suffix("in") {
        (rest, 25.4)
    } else if let Some(rest) = lower.strip_suffix("cm") {
        (rest, 10.0)
    } else {
        (lower.as_str(), 0.0254)
    };
    num.trim().parse::<f64>().ok().map(|v| v * scale)
}

// ============================================================================
// Layers
// ============================================================================

/// Altium layer, kept abstract so ASCII names and binary ids share one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AltiumLayer {
    Top,
    Bottom,
    /// Mid-layer 1..=30 (Altium's inner signal layers).
    Mid(u8),
    /// Internal plane 1..=16.
    Plane(u8),
    TopOverlay,
    BottomOverlay,
    TopSolder,
    BottomSolder,
    TopPaste,
    BottomPaste,
    /// Mechanical 1..=16.
    Mechanical(u8),
    KeepOut,
    MultiLayer,
    Other,
}

impl AltiumLayer {
    /// Parse an ASCII layer name (`TOPLAYER`, `MIDLAYER3`, `MECHANICAL1`, …).
    pub(crate) fn from_name(name: &str) -> AltiumLayer {
        let n = name
            .trim()
            .to_ascii_uppercase()
            .replace([' ', '_', '.'], "");
        let indexed = |prefix: &str| -> Option<u8> {
            n.strip_prefix(prefix).and_then(|r| r.parse::<u8>().ok())
        };
        match n.as_str() {
            "TOP" | "TOPLAYER" | "TOPCOPPER" => return AltiumLayer::Top,
            "BOTTOM" | "BOTTOMLAYER" | "BOTTOMCOPPER" => return AltiumLayer::Bottom,
            "TOPOVERLAY" => return AltiumLayer::TopOverlay,
            "BOTTOMOVERLAY" => return AltiumLayer::BottomOverlay,
            "TOPSOLDER" => return AltiumLayer::TopSolder,
            "BOTTOMSOLDER" => return AltiumLayer::BottomSolder,
            "TOPPASTE" => return AltiumLayer::TopPaste,
            "BOTTOMPASTE" => return AltiumLayer::BottomPaste,
            "KEEPOUT" | "KEEPOUTLAYER" => return AltiumLayer::KeepOut,
            "MULTILAYER" => return AltiumLayer::MultiLayer,
            _ => {}
        }
        if let Some(i) = indexed("MIDLAYER") {
            return AltiumLayer::Mid(i);
        }
        if let Some(i) = indexed("INTERNALPLANE") {
            return AltiumLayer::Plane(i);
        }
        if let Some(i) = indexed("MECHANICAL") {
            return AltiumLayer::Mechanical(i);
        }
        AltiumLayer::Other
    }

    /// Map a binary layer id. Altium numbers layers 1=Top, 2..=31=Mid1..30,
    /// 32=Bottom, then the overlay/mask/paste block, planes, and mechanicals.
    pub(crate) fn from_id(id: u8) -> AltiumLayer {
        match id {
            1 => AltiumLayer::Top,
            2..=31 => AltiumLayer::Mid(id - 1),
            32 => AltiumLayer::Bottom,
            33 => AltiumLayer::TopOverlay,
            34 => AltiumLayer::BottomOverlay,
            35 => AltiumLayer::TopPaste,
            36 => AltiumLayer::BottomPaste,
            37 => AltiumLayer::TopSolder,
            38 => AltiumLayer::BottomSolder,
            39..=54 => AltiumLayer::Plane(id - 38),
            56 => AltiumLayer::KeepOut,
            57..=72 => AltiumLayer::Mechanical(id - 56),
            74 => AltiumLayer::MultiLayer,
            _ => AltiumLayer::Other,
        }
    }

    /// Whether this layer is copper at all, independent of whether the board
    /// actually uses it.
    pub(crate) fn is_copper_family(self) -> bool {
        self.copper_index().is_some()
    }

    /// Copper layers only, in stack order: `None` for documentation layers.
    fn copper_index(self) -> Option<u32> {
        match self {
            AltiumLayer::Top => Some(0),
            AltiumLayer::Mid(i) => Some(i as u32),
            // Internal planes sit between the mid-layers; push them past the
            // signal mids so the two families keep a stable relative order.
            AltiumLayer::Plane(i) => Some(100 + i as u32),
            AltiumLayer::Bottom => Some(u32::MAX),
            _ => None,
        }
    }

    /// Non-copper mapping to the IR's documentation layers.
    fn doc_layer(self) -> Option<PcbLayer> {
        Some(match self {
            AltiumLayer::TopOverlay => PcbLayer::FSilkS,
            AltiumLayer::BottomOverlay => PcbLayer::BSilkS,
            AltiumLayer::TopSolder => PcbLayer::FMask,
            AltiumLayer::BottomSolder => PcbLayer::BMask,
            AltiumLayer::TopPaste => PcbLayer::FPaste,
            AltiumLayer::BottomPaste => PcbLayer::BPaste,
            AltiumLayer::KeepOut => PcbLayer::EdgeCuts,
            AltiumLayer::Mechanical(1) => PcbLayer::EdgeCuts,
            AltiumLayer::Mechanical(_) => PcbLayer::UserDrawings,
            _ => return None,
        })
    }
}

/// Resolves Altium copper layers onto the IR's ten-deep copper vocabulary.
///
/// Only layers the board actually uses are mapped, in stack order, so a
/// four-layer board lands on FCu/In1/In2/BCu rather than leaving holes.
#[derive(Debug, Default)]
pub(crate) struct LayerMap {
    /// Used copper layers, sorted by [`AltiumLayer::copper_index`].
    used: Vec<AltiumLayer>,
}

/// One copper layer as the file's own layer stack declares it.
#[derive(Debug, Clone)]
pub(crate) struct StackEntry {
    /// Altium layer id (1 = Top, 2..=31 = Mid1..30, 32 = Bottom).
    pub id: u8,
    pub copper_mm: Option<f64>,
    pub dielectric_mm: Option<f64>,
    pub er: Option<f64>,
    pub material: Option<String>,
}

/// Read the board's declared layer stack.
///
/// `Board6` stores it as a linked list — `LAYER{n}PREV` / `LAYER{n}NEXT` over
/// `LAYER{n}NAME` / `COPTHICK` / `DIELCONST` / `DIELHEIGHT` / `DIELMATERIAL` —
/// where `n` is the same layer id primitives use. The chain is authoritative
/// and the table is not contiguous (a real 8-layer board chains
/// 1→2→3→4→5→10→8→32), so it must be walked rather than enumerated, and it is
/// the only way to see a plane layer that carries no tracks.
pub(crate) fn parse_layer_stack(boards: &[&Record]) -> Vec<StackEntry> {
    let field = |n: u8, suffix: &str| -> Option<&str> {
        boards
            .iter()
            .find_map(|b| b.get(&format!("LAYER{n}{suffix}")))
    };
    let mut out = Vec::new();
    let mut seen = Vec::new();
    let mut node: u8 = 1; // Altium's Top is always layer id 1.
    while node != 0 && !seen.contains(&node) && out.len() < 64 {
        seen.push(node);
        if field(node, "NAME").is_none() {
            break;
        }
        let len = |suffix: &str| field(node, suffix).and_then(parse_length_mm);
        out.push(StackEntry {
            id: node,
            copper_mm: len("COPTHICK"),
            dielectric_mm: len("DIELHEIGHT"),
            er: field(node, "DIELCONST").and_then(|v| v.trim().parse().ok()),
            material: field(node, "DIELMATERIAL")
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .map(str::to_string),
        });
        node = field(node, "NEXT")
            .and_then(|v| v.trim().parse::<u8>().ok())
            .unwrap_or(0);
    }
    // A one-entry "chain" is a file without a usable stack, not a 1-layer board.
    if out.len() < 2 {
        out.clear();
    }
    out
}

impl LayerMap {
    /// Build from the declared stack, unioned with any copper layer that
    /// primitives actually use.
    ///
    /// The declared stack wins on ordering — it is the physical truth, and it
    /// includes plane layers that carry no tracks. A used layer missing from
    /// the stack is still mapped rather than dropped, since silently discarding
    /// copper is worse than an imperfect stack.
    pub(crate) fn build_with_stack(
        stack: &[StackEntry],
        seen: impl IntoIterator<Item = AltiumLayer>,
    ) -> Result<LayerMap, String> {
        if stack.is_empty() {
            return LayerMap::build(seen);
        }
        let mut used: Vec<AltiumLayer> = stack
            .iter()
            .map(|e| AltiumLayer::from_id(e.id))
            .filter(|l| l.copper_index().is_some())
            .collect();
        let mut extra: Vec<AltiumLayer> = seen
            .into_iter()
            .filter(|l| l.copper_index().is_some() && !used.contains(l))
            .collect();
        extra.sort_by_key(|l| l.copper_index().unwrap());
        extra.dedup();
        // Splice strays in ahead of Bottom so the back copper stays last.
        let tail = usize::from(used.last() == Some(&AltiumLayer::Bottom));
        let at = used.len() - tail;
        for (k, l) in extra.into_iter().enumerate() {
            used.insert(at + k, l);
        }
        if used.len() > 10 {
            return Err(format!(
                "board declares {} copper layers; the IR supports at most 10 \
                 (FCu + In1..In8 + BCu)",
                used.len()
            ));
        }
        Ok(LayerMap { used })
    }

    pub(crate) fn build(seen: impl IntoIterator<Item = AltiumLayer>) -> Result<LayerMap, String> {
        let mut used: Vec<AltiumLayer> = seen
            .into_iter()
            .filter(|l| l.copper_index().is_some())
            .collect();
        used.sort_by_key(|l| l.copper_index().unwrap());
        used.dedup();
        if !used.contains(&AltiumLayer::Top) {
            used.insert(0, AltiumLayer::Top);
        }
        if !used.contains(&AltiumLayer::Bottom) {
            used.push(AltiumLayer::Bottom);
        }
        // Fail closed rather than merging layers: collapsing two copper layers
        // into one fabricates connectivity that DRC would then bless.
        if used.len() > 10 {
            return Err(format!(
                "board uses {} copper layers; the IR supports at most 10 \
                 (FCu + In1..In8 + BCu)",
                used.len()
            ));
        }
        Ok(LayerMap { used })
    }

    /// Copper layers in stack order, front to back.
    pub(crate) fn copper_layers(&self) -> Vec<PcbLayer> {
        (0..self.used.len())
            .map(|i| Self::slot(i, self.used.len()))
            .collect()
    }

    fn slot(i: usize, n: usize) -> PcbLayer {
        if i == 0 {
            return PcbLayer::FCu;
        }
        if i + 1 >= n {
            return PcbLayer::BCu;
        }
        match i {
            1 => PcbLayer::In1Cu,
            2 => PcbLayer::In2Cu,
            3 => PcbLayer::In3Cu,
            4 => PcbLayer::In4Cu,
            5 => PcbLayer::In5Cu,
            6 => PcbLayer::In6Cu,
            7 => PcbLayer::In7Cu,
            _ => PcbLayer::In8Cu,
        }
    }

    /// Map one Altium layer to the IR, copper or documentation.
    pub(crate) fn map(&self, layer: AltiumLayer) -> Option<PcbLayer> {
        if layer.copper_index().is_some() {
            let i = self.used.iter().position(|&u| u == layer)?;
            return Some(Self::slot(i, self.used.len()));
        }
        layer.doc_layer()
    }
}

// ============================================================================
// Record set → Pcb
// ============================================================================

/// Everything a frontend hands the builder.
pub(crate) struct RecordSet {
    pub records: Vec<Record>,
}

/// Board extents (mm) declared by the `Board` record, used to sanity-check
/// binary-decoded geometry.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Extents {
    pub min: Vec2,
    pub max: Vec2,
}

impl Extents {
    /// A generous default: Altium's own maximum workspace is about 100 inches
    /// square, so anything outside this is a decode error, not a big board.
    pub(crate) fn permissive() -> Extents {
        Extents {
            min: Vec2::new(-2540.0, -2540.0),
            max: Vec2::new(2540.0, 2540.0),
        }
    }

    pub(crate) fn contains(&self, p: Vec2) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }
}

/// Build a [`Pcb`] from a normalised record set.
pub(crate) fn build_pcb(set: RecordSet) -> Result<Pcb, String> {
    let records = set.records;

    // --- Nets, in file order: Altium references them by index. ---
    let net_names: Vec<String> = records
        .iter()
        .filter(|r| r.kind() == "NET")
        .map(|r| r.get("NAME").unwrap_or("").to_string())
        .collect();
    let nets: Vec<Net> = net_names
        .iter()
        .enumerate()
        .map(|(i, name)| Net {
            id: format!("{i}"),
            name: name.clone(),
        })
        .collect();
    // Altium writes net *indices*; some third-party exporters write the name.
    let by_name: HashMap<&str, usize> = net_names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    let net_of = |r: &Record| -> Option<NetId> {
        let raw = r.get("NET")?.trim();
        if raw.is_empty() {
            return None;
        }
        if let Ok(idx) = raw.parse::<i64>() {
            // -1 (and Altium's 0xFFFF, normalised to -1 by the binary reader)
            // mean "no net".
            if idx < 0 || idx as usize >= net_names.len() {
                return None;
            }
            return Some(format!("{idx}"));
        }
        by_name.get(raw).map(|i| format!("{i}"))
    };

    // --- Copper layers actually in use. ---
    let mut seen: Vec<AltiumLayer> = Vec::new();
    for r in &records {
        for key in ["LAYER", "STARTLAYER", "ENDLAYER"] {
            if let Some(name) = r.get(key) {
                seen.push(AltiumLayer::from_name(name));
            }
        }
    }
    // A file can carry more than one `Board` record (binary `Board6` splits the
    // outline and the layer stack across two), so consult all of them.
    let boards: Vec<&Record> = records.iter().filter(|r| r.kind() == "BOARD").collect();
    let stack = parse_layer_stack(&boards);
    let layers = LayerMap::build_with_stack(&stack, seen)?;

    // --- Components, in file order: pads reference them by index. ---
    let comp_records: Vec<&Record> = records.iter().filter(|r| r.kind() == "COMPONENT").collect();
    let mut footprints: Vec<Footprint> = comp_records
        .iter()
        .map(|r| {
            let front = !matches!(
                AltiumLayer::from_name(r.get("LAYER").unwrap_or("TOPLAYER")),
                AltiumLayer::Bottom
            );
            Footprint {
                reference: r
                    .get("SOURCEDESIGNATOR")
                    .or_else(|| r.get("DESIGNATOR"))
                    .or_else(|| r.get("NAME"))
                    .unwrap_or("")
                    .to_string(),
                value: r
                    .get("COMMENT")
                    .or_else(|| r.get("SOURCEVALUE"))
                    .unwrap_or("")
                    .to_string(),
                footprint_name: match (r.get("SOURCEFOOTPRINTLIBRARY"), r.get("PATTERN")) {
                    (Some(lib), Some(pat)) if !lib.is_empty() => format!("{lib}:{pat}"),
                    (_, Some(pat)) => pat.to_string(),
                    _ => String::new(),
                },
                position: r.point_mm("X", "Y").unwrap_or(Vec2::new(0.0, 0.0)),
                rotation: r.num("ROTATION").unwrap_or(0.0),
                front,
                pads: vec![],
                graphics: vec![],
                model_3d: None,
                properties: HashMap::new(),
            }
        })
        .collect();

    // --- Pads. Altium stores absolute board coordinates; the IR stores the
    // footprint-local frame, and every consumer reconstitutes world position
    // as `fp.position + R(fp.rotation)·pad.position`. Inverting that exactly
    // (rather than re-deriving a mirror rule per side) makes the round-trip
    // exact for both sides of the board.
    let mut loose_pads: Vec<Pad> = Vec::new();
    for r in records.iter().filter(|r| r.kind() == "PAD") {
        let Some(abs) = r.point_mm("X", "Y") else {
            continue;
        };
        let layer = AltiumLayer::from_name(r.get("LAYER").unwrap_or("TOPLAYER"));
        let hole = r.len_mm_or("HOLESIZE", 0.0);
        let plated = r.flag("PLATED").unwrap_or(true);
        let through = hole > 0.0 || layer == AltiumLayer::MultiLayer;
        let pad_type = if !through {
            PadType::SMD
        } else if plated {
            PadType::THT
        } else {
            PadType::NPTH
        };
        let w = r.len_mm_or("XSIZE", r.len_mm_or("TOPXSIZE", 0.0));
        let h = r.len_mm_or("YSIZE", r.len_mm_or("TOPYSIZE", 0.0));
        if w <= 0.0 || h <= 0.0 {
            continue;
        }
        let shape = match r
            .get("SHAPE")
            .or_else(|| r.get("TOPSHAPE"))
            .unwrap_or("ROUND")
            .trim()
            .to_ascii_uppercase()
            .as_str()
        {
            "RECTANGLE" | "RECT" | "SQUARE" => PadShape::Rect {
                width: w,
                height: h,
            },
            "ROUNDEDRECTANGLE" | "ROUNDRECT" => PadShape::RoundRect {
                width: w,
                height: h,
                corner_ratio: r.num("CORNERRADIUS").map(|v| v / 100.0).unwrap_or(0.25),
            },
            // Altium's ROUND is a circle when square, a stadium otherwise.
            _ if (w - h).abs() < 1e-9 => PadShape::Circle { diameter: w },
            _ => PadShape::Oval {
                width: w,
                height: h,
            },
        };
        let pad_layers = if through {
            layers
                .copper_layers()
                .into_iter()
                .filter(|l| l.is_copper())
                .collect()
        } else {
            vec![layers.map(layer).unwrap_or(PcbLayer::FCu)]
        };
        let abs_rot = r.num("ROTATION").unwrap_or(0.0);
        let pad = Pad {
            number: r.get("NAME").unwrap_or("").to_string(),
            pad_type,
            shape,
            position: abs,
            rotation: abs_rot,
            drill: through.then(|| DrillSpec {
                diameter: hole.max(0.05),
                oval: r.int("HOLETYPE") == Some(2),
                oval_height: (r.int("HOLETYPE") == Some(2))
                    .then(|| r.len_mm("HOLEWIDTH"))
                    .flatten(),
            }),
            net: net_of(r),
            layers: pad_layers,
        };
        match r.int("COMPONENT") {
            Some(i) if i >= 0 && (i as usize) < footprints.len() => {
                let fp = &mut footprints[i as usize];
                let (dx, dy) = (
                    pad.position.x - fp.position.x,
                    pad.position.y - fp.position.y,
                );
                let a = -fp.rotation.to_radians();
                let (s, c) = a.sin_cos();
                let mut pad = pad;
                pad.position = Vec2::new(dx * c - dy * s, dx * s + dy * c);
                pad.rotation = abs_rot - fp.rotation;
                fp.pads.push(pad);
            }
            // A free pad with no owning component: Altium allows these, the IR
            // does not, so wrap each in its own single-pad footprint below.
            _ => loose_pads.push(pad),
        }
    }
    for (i, mut pad) in loose_pads.into_iter().enumerate() {
        let position = pad.position;
        pad.position = Vec2::new(0.0, 0.0);
        let front = pad.layers.first() != Some(&PcbLayer::BCu);
        footprints.push(Footprint {
            reference: format!("FREEPAD{}", i + 1),
            value: String::new(),
            footprint_name: "altium:free_pad".into(),
            position,
            rotation: 0.0,
            front,
            pads: vec![pad],
            graphics: vec![],
            model_3d: None,
            properties: HashMap::new(),
        });
    }

    // --- Copper: tracks, arcs, vias. ---
    let mut traces = Vec::new();
    let mut trace_arcs = Vec::new();
    let mut vias = Vec::new();
    let mut outline_segs: Vec<(Vec2, Vec2)> = Vec::new();
    let mut graphics: Vec<(usize, FootprintGraphic)> = Vec::new();

    for r in &records {
        match r.kind() {
            "TRACK" => {
                let (Some(start), Some(end)) = (r.point_mm("X1", "Y1"), r.point_mm("X2", "Y2"))
                else {
                    continue;
                };
                let al = AltiumLayer::from_name(r.get("LAYER").unwrap_or(""));
                let width = r.len_mm_or("WIDTH", 0.15).max(0.01);
                let Some(layer) = layers.map(al) else {
                    continue;
                };
                if layer.is_copper() {
                    let Some(net) = net_of(r) else {
                        // Copper with no net still occupies space; keep it on a
                        // synthetic unnamed net rather than dropping it.
                        traces.push(Trace {
                            start,
                            end,
                            width,
                            layer,
                            net: String::new(),
                            source: Some(CopperSource::Manual),
                        });
                        continue;
                    };
                    traces.push(Trace {
                        start,
                        end,
                        width,
                        layer,
                        net,
                        source: Some(CopperSource::Manual),
                    });
                } else {
                    if layer == PcbLayer::EdgeCuts {
                        outline_segs.push((start, end));
                    }
                    if let Some(ci) = r.int("COMPONENT").filter(|&i| i >= 0) {
                        if (ci as usize) < footprints.len() {
                            graphics.push((
                                ci as usize,
                                FootprintGraphic::Line {
                                    start,
                                    end,
                                    width,
                                    layer,
                                },
                            ));
                        }
                    }
                }
            }
            "ARC" => {
                let Some(center) = r
                    .point_mm("LOCATION.X", "LOCATION.Y")
                    .or_else(|| r.point_mm("X", "Y"))
                else {
                    continue;
                };
                let radius = r.len_mm_or("RADIUS", 0.0);
                if radius <= 0.0 {
                    continue;
                }
                let al = AltiumLayer::from_name(r.get("LAYER").unwrap_or(""));
                let width = r.len_mm_or("WIDTH", 0.15).max(0.01);
                let start_angle = r.num("STARTANGLE").unwrap_or(0.0);
                let end_angle = r.num("ENDANGLE").unwrap_or(360.0);
                let Some(layer) = layers.map(al) else {
                    continue;
                };
                if layer.is_copper() {
                    if let Some(net) = net_of(r) {
                        trace_arcs.push(TraceArc {
                            center,
                            radius,
                            start_angle,
                            end_angle,
                            width,
                            layer,
                            net,
                        });
                    }
                } else {
                    if layer == PcbLayer::EdgeCuts {
                        // Chord-approximate outline arcs so the profile closes.
                        for (a, b) in arc_segments(center, radius, start_angle, end_angle) {
                            outline_segs.push((a, b));
                        }
                    }
                    if let Some(ci) = r.int("COMPONENT").filter(|&i| i >= 0) {
                        if (ci as usize) < footprints.len() {
                            graphics.push((
                                ci as usize,
                                FootprintGraphic::Arc {
                                    center,
                                    radius,
                                    start_angle,
                                    end_angle,
                                    width,
                                    layer,
                                },
                            ));
                        }
                    }
                }
            }
            "VIA" => {
                let Some(position) = r.point_mm("X", "Y") else {
                    continue;
                };
                let drill = r.len_mm_or("HOLESIZE", 0.3).max(0.05);
                let diameter = r.len_mm_or("DIAMETER", drill * 2.0).max(drill + 0.05);
                let span = layers.copper_layers();
                let start_layer = r
                    .get("STARTLAYER")
                    .map(AltiumLayer::from_name)
                    .and_then(|l| layers.map(l))
                    .filter(|l| l.is_copper())
                    .unwrap_or(PcbLayer::FCu);
                let end_layer = r
                    .get("ENDLAYER")
                    .map(AltiumLayer::from_name)
                    .and_then(|l| layers.map(l))
                    .filter(|l| l.is_copper())
                    .unwrap_or(*span.last().unwrap_or(&PcbLayer::BCu));
                vias.push(Via {
                    position,
                    diameter,
                    drill,
                    start_layer,
                    end_layer,
                    net: net_of(r).unwrap_or_default(),
                    source: Some(CopperSource::Manual),
                });
            }
            _ => {}
        }
    }
    for (i, g) in graphics {
        // Footprint graphics are stored in the component's local frame.
        let g = localise(g, &footprints[i]);
        footprints[i].graphics.push(g);
    }

    // --- Outline: the Board record's vertex list wins; otherwise chain the
    // mechanical/keepout graphics; otherwise fall back to the copper bbox.
    let mut vertices = boards
        .iter()
        .map(|b| board_outline(b))
        .find(|v| v.len() >= 3)
        .unwrap_or_default();
    if vertices.len() < 3 {
        vertices = chain_outline(&outline_segs);
    }
    if vertices.len() < 3 {
        vertices = bbox_outline(&traces, &vias, &footprints);
    }
    let thickness = boards
        .iter()
        .find_map(|b| b.len_mm("BOARDTHICKNESS"))
        .filter(|t| *t > 0.0)
        .unwrap_or(1.6);

    let copper = layers.copper_layers();
    let n = copper.len();
    // Prefer the file's own copper thickness / dielectric height / Er per
    // layer; only synthesise plausible values for layers it doesn't describe.
    let stackup = LayerStackup {
        layers: copper
            .iter()
            .enumerate()
            .map(|(i, &layer)| {
                let declared = stack
                    .iter()
                    .find(|e| layers.map(AltiumLayer::from_id(e.id)) == Some(layer));
                let last = i + 1 >= n;
                StackupLayer {
                    layer,
                    copper_thickness: declared
                        .and_then(|e| e.copper_mm)
                        .filter(|t| *t > 0.0)
                        .or(Some(0.035)),
                    dielectric_thickness: (!last).then(|| {
                        declared
                            .and_then(|e| e.dielectric_mm)
                            .filter(|t| *t > 0.0)
                            .unwrap_or_else(|| {
                                (thickness - 0.035 * n as f64).max(0.05) / (n - 1).max(1) as f64
                            })
                    }),
                    dielectric_er: (!last).then(|| {
                        declared
                            .and_then(|e| e.er)
                            .filter(|v| *v > 1.0)
                            .unwrap_or(4.5)
                    }),
                    material: (!last).then(|| {
                        declared
                            .and_then(|e| e.material.clone())
                            .unwrap_or_else(|| "FR4".to_string())
                    }),
                }
            })
            .collect(),
    };

    Ok(Pcb {
        outline: BoardOutline {
            vertices,
            cutouts: vec![],
            thickness,
        },
        stackup,
        nets,
        rules: default_rules(&records),
        footprints,
        traces,
        trace_arcs,
        vias,
        zones: vec![],
        keepouts: vec![],
        net_ties: vec![],
    })
}

/// Rotate a footprint graphic from board coordinates into the footprint frame.
fn localise(g: FootprintGraphic, fp: &Footprint) -> FootprintGraphic {
    let a = -fp.rotation.to_radians();
    let (s, c) = a.sin_cos();
    let to_local = |p: Vec2| {
        let (dx, dy) = (p.x - fp.position.x, p.y - fp.position.y);
        Vec2::new(dx * c - dy * s, dx * s + dy * c)
    };
    match g {
        FootprintGraphic::Line {
            start,
            end,
            width,
            layer,
        } => FootprintGraphic::Line {
            start: to_local(start),
            end: to_local(end),
            width,
            layer,
        },
        FootprintGraphic::Arc {
            center,
            radius,
            start_angle,
            end_angle,
            width,
            layer,
        } => FootprintGraphic::Arc {
            center: to_local(center),
            radius,
            start_angle: start_angle - fp.rotation,
            end_angle: end_angle - fp.rotation,
            width,
            layer,
        },
        other => other,
    }
}

/// Altium's `Board` record carries the outline as `VX{i}`/`VY{i}` pairs
/// (older exports spell them `OUTLINEVERTEX{i}X`/`…Y`).
fn board_outline(board: &Record) -> Vec<Vec2> {
    let mut out = Vec::new();
    for i in 0..10_000 {
        let p = board
            .point_mm(&format!("VX{i}"), &format!("VY{i}"))
            .or_else(|| {
                board.point_mm(&format!("OUTLINEVERTEX{i}X"), &format!("OUTLINEVERTEX{i}Y"))
            });
        match p {
            Some(p) => out.push(p),
            None => break,
        }
    }
    if out.len() > 2 && (out[0] - *out.last().unwrap()).length() < 1e-6 {
        out.pop();
    }
    out
}

/// Axis-aligned fallback profile when the file carries no outline at all.
fn bbox_outline(traces: &[Trace], vias: &[Via], footprints: &[Footprint]) -> Vec<Vec2> {
    let mut pts: Vec<Vec2> = Vec::new();
    for t in traces {
        pts.push(t.start);
        pts.push(t.end);
    }
    pts.extend(vias.iter().map(|v| v.position));
    pts.extend(footprints.iter().map(|f| f.position));
    if pts.is_empty() {
        return vec![];
    }
    let (mut lo, mut hi) = (pts[0], pts[0]);
    for p in &pts {
        lo = Vec2::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = Vec2::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    let m = 1.0;
    vec![
        Vec2::new(lo.x - m, lo.y - m),
        Vec2::new(hi.x + m, lo.y - m),
        Vec2::new(hi.x + m, hi.y + m),
        Vec2::new(lo.x - m, hi.y + m),
    ]
}

/// Chord-approximate an arc for outline chaining (1° steps, ≥1 segment).
fn arc_segments(center: Vec2, radius: f64, start: f64, end: f64) -> Vec<(Vec2, Vec2)> {
    let sweep = {
        let mut d = end - start;
        while d <= 0.0 {
            d += 360.0;
        }
        d.min(360.0)
    };
    let steps = (sweep.ceil() as usize).max(1);
    let at = |deg: f64| {
        let r = deg.to_radians();
        Vec2::new(center.x + radius * r.cos(), center.y + radius * r.sin())
    };
    (0..steps)
        .map(|i| {
            let a = start + sweep * i as f64 / steps as f64;
            let b = start + sweep * (i + 1) as f64 / steps as f64;
            (at(a), at(b))
        })
        .collect()
}

/// Chain unordered outline segments into a single vertex loop (greedy
/// nearest-end, same approach the Eagle importer uses).
fn chain_outline(segs: &[(Vec2, Vec2)]) -> Vec<Vec2> {
    if segs.is_empty() {
        return vec![];
    }
    let mut rest: Vec<(Vec2, Vec2)> = segs.to_vec();
    let (a0, b0) = rest.remove(0);
    let mut out = vec![a0, b0];
    while !rest.is_empty() {
        let tail = *out.last().unwrap();
        let mut best: Option<(usize, bool, f64)> = None;
        for (i, (a, b)) in rest.iter().enumerate() {
            let da = (tail - *a).length();
            let db = (tail - *b).length();
            if best.map(|(_, _, d)| da < d).unwrap_or(true) {
                best = Some((i, false, da));
            }
            if best.map(|(_, _, d)| db < d).unwrap_or(true) {
                best = Some((i, true, db));
            }
        }
        let Some((i, flip, d)) = best else { break };
        if d > 1.0 {
            break;
        }
        let (a, b) = rest.remove(i);
        out.push(if flip { a } else { b });
    }
    if out.len() > 2 && (*out.first().unwrap() - *out.last().unwrap()).length() < 1e-3 {
        out.pop();
    }
    out
}

/// Design rules: read Altium's `Rule` records where they map cleanly, else the
/// same conservative defaults the other importers use.
fn default_rules(records: &[Record]) -> DesignRules {
    let rule = |kind: &str, key: &str| -> Option<f64> {
        records
            .iter()
            .filter(|r| r.kind() == "RULE")
            .find(|r| {
                r.get("RULEKIND")
                    .map(|k| k.eq_ignore_ascii_case(kind))
                    .unwrap_or(false)
            })
            .and_then(|r| r.len_mm(key))
    };
    DesignRules {
        default_rules: NetClassRules {
            name: "Default".into(),
            trace_width: rule("Width", "MINLIMIT").unwrap_or(0.25),
            clearance: rule("Clearance", "GAP").unwrap_or(0.15),
            via_diameter: rule("RoutingVias", "MINWIDTH").unwrap_or(0.6),
            via_drill: rule("RoutingVias", "MINHOLEWIDTH").unwrap_or(0.3),
            diff_pair_gap: None,
            diff_pair_width: None,
            target_impedance: None,
            target_diff_impedance: None,
        },
        class_rules: vec![],
        net_class_assignments: HashMap::new(),
        edge_clearance: 0.25,
        hole_to_hole: rule("HoleToHoleClearance", "GAP").unwrap_or(0.25),
        min_annular_ring: 0.13,
        min_drill: rule("HoleSize", "MINLIMIT").unwrap_or(0.2),
    }
}

// ============================================================================
// Footprint libraries
// ============================================================================

/// Group the pads/graphics of a `.PcbLib` record set into footprint definitions.
///
/// A PcbLib is a board file whose "components" are library patterns: each
/// component record names one footprint, and its pads carry absolute
/// coordinates that we re-express in the pattern's own frame.
pub(crate) fn build_footprint_lib(set: RecordSet) -> Result<FootprintLib, String> {
    let pcb = build_pcb(set)?;
    let footprints = pcb
        .footprints
        .iter()
        .filter(|f| !f.pads.is_empty())
        .map(|f| FootprintDef {
            name: if f.footprint_name.is_empty() {
                f.reference.clone()
            } else {
                f.footprint_name.clone()
            },
            pads: f
                .pads
                .iter()
                .map(|p| PadDef {
                    number: p.number.clone(),
                    pad_type: p.pad_type,
                    shape: p.shape.clone(),
                    position: (p.position.x, p.position.y),
                    rotation: p.rotation,
                    layers: p.layers.iter().map(|l| format!("{l:?}")).collect(),
                    drill: p.drill.clone(),
                })
                .collect(),
            graphics: f
                .graphics
                .iter()
                .filter_map(|g| match g {
                    FootprintGraphic::Line {
                        start,
                        end,
                        width,
                        layer,
                    } => Some(GraphicDef::Line {
                        start: (start.x, start.y),
                        end: (end.x, end.y),
                        width: *width,
                        layer: format!("{layer:?}"),
                    }),
                    _ => None,
                })
                .collect(),
            model_3d: None,
        })
        .collect::<Vec<_>>();
    if footprints.is_empty() {
        return Err("no footprint patterns with pads found in the library".into());
    }
    Ok(FootprintLib { footprints })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_carry_units() {
        assert!((parse_length_mm("12.7mm").unwrap() - 12.7).abs() < 1e-12);
        assert!((parse_length_mm("1000mil").unwrap() - 25.4).abs() < 1e-12);
        assert!((parse_length_mm("1in").unwrap() - 25.4).abs() < 1e-12);
        // Bare numbers are mils in Altium ASCII.
        assert!((parse_length_mm("100").unwrap() - 2.54).abs() < 1e-12);
        assert!(parse_length_mm("").is_none());
        assert!(parse_length_mm("junk").is_none());
    }

    #[test]
    fn layer_names_and_ids_agree() {
        assert_eq!(AltiumLayer::from_name("TOPLAYER"), AltiumLayer::Top);
        assert_eq!(AltiumLayer::from_id(1), AltiumLayer::Top);
        assert_eq!(AltiumLayer::from_name("BOTTOMLAYER"), AltiumLayer::Bottom);
        assert_eq!(AltiumLayer::from_id(32), AltiumLayer::Bottom);
        assert_eq!(AltiumLayer::from_name("MIDLAYER3"), AltiumLayer::Mid(3));
        assert_eq!(AltiumLayer::from_id(4), AltiumLayer::Mid(3));
        assert_eq!(
            AltiumLayer::from_name("MECHANICAL1"),
            AltiumLayer::Mechanical(1)
        );
        assert_eq!(AltiumLayer::from_id(57), AltiumLayer::Mechanical(1));
    }

    #[test]
    fn four_layer_stack_maps_without_holes() {
        let map = LayerMap::build([
            AltiumLayer::Top,
            AltiumLayer::Mid(2),
            AltiumLayer::Mid(5),
            AltiumLayer::Bottom,
        ])
        .unwrap();
        assert_eq!(map.map(AltiumLayer::Top), Some(PcbLayer::FCu));
        assert_eq!(map.map(AltiumLayer::Mid(2)), Some(PcbLayer::In1Cu));
        assert_eq!(map.map(AltiumLayer::Mid(5)), Some(PcbLayer::In2Cu));
        assert_eq!(map.map(AltiumLayer::Bottom), Some(PcbLayer::BCu));
    }

    #[test]
    fn too_many_copper_layers_fails_closed() {
        let mids = (1..=12).map(AltiumLayer::Mid);
        assert!(LayerMap::build(mids).is_err());
    }
}
