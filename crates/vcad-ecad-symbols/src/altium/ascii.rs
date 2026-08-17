//! Altium ASCII `.PcbDoc` reader.
//!
//! Altium's *File ▸ Save As ▸ PCB ASCII* writes one `|KEY=VALUE|KEY=VALUE|`
//! record per line. Keys are case-insensitive and the set varies by Altium
//! version, so the reader is tolerant: unknown keys and unknown record kinds
//! are ignored, and the record → [`Pcb`] builder in the parent module decides
//! what it needs.

use vcad_ir::ecad::Pcb;

use super::{build_pcb, Record, RecordSet};

/// Parse an Altium ASCII-exported `.PcbDoc` into a [`Pcb`].
///
/// Returns an error when the text carries no Altium records at all — which is
/// almost always a binary `.PcbDoc` handed to the ASCII path by mistake.
pub fn parse_altium_ascii_pcb(text: &str) -> Result<Pcb, String> {
    let records = scan_records(text);
    if records.is_empty() {
        return Err(
            "no Altium |RECORD=…| entries found. If this is a native binary .PcbDoc, \
             import it as binary; otherwise re-export from Altium with \
             File > Save As > PCB ASCII."
                .into(),
        );
    }
    build_pcb(RecordSet { records })
}

/// Split ASCII text into records.
///
/// Records are pipe-delimited and normally one per line, but Altium also emits
/// several on one line for small object families, so the scanner splits on
/// `|RECORD=` rather than on newlines.
pub(super) fn scan_records(text: &str) -> Vec<Record> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // `split` on the marker: the first piece is any leading junk.
        let mut pieces = line.split("|RECORD=");
        let _leading = pieces.next();
        for piece in pieces {
            let mut rec = parse_pipe_fields(piece.split('|').skip(1));
            rec.set(
                "RECORD",
                piece.split('|').next().unwrap_or("").trim().to_uppercase(),
            );
            out.push(rec);
        }
    }
    out
}

/// Parse a bare `|KEY=VALUE|KEY=VALUE` block, with no `RECORD=` marker.
///
/// Native binary files carry exactly this: the record's kind is implied by the
/// stream it came from (`Nets6`, `Components6`, …) rather than written into
/// the text, so the binary reader supplies the kind separately.
pub(super) fn parse_bare_record(body: &str, kind: &str) -> Record {
    let mut rec = parse_pipe_fields(body.split('|'));
    if rec.get("RECORD").is_none() {
        rec.set("RECORD", kind.to_uppercase());
    }
    rec
}

/// Collect `KEY=VALUE` pairs from pipe-delimited fields.
fn parse_pipe_fields<'a>(parts: impl Iterator<Item = &'a str>) -> Record {
    let mut fields = Vec::new();
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        // Values may themselves contain '=' (file paths, expressions); only the
        // first separator delimits the key.
        if let Some((k, v)) = part.split_once('=') {
            let k = k.trim();
            if !k.is_empty() {
                fields.push((k.to_string(), v.trim().to_string()));
            }
        }
    }
    Record::from_pairs(fields)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::ecad::{PadType, PcbLayer};

    /// A two-layer board: outline, one resistor, one routed net, one via.
    const BOARD: &str = "\
|RECORD=Board|FILENAME=test.PcbDoc|BOARDTHICKNESS=1.6mm|VX0=0mm|VY0=0mm|VX1=20mm|VY1=0mm|VX2=20mm|VY2=15mm|VX3=0mm|VY3=15mm|
|RECORD=Net|NAME=GND|
|RECORD=Net|NAME=VCC|
|RECORD=Component|SOURCEDESIGNATOR=R1|PATTERN=0805|SOURCEFOOTPRINTLIBRARY=Std|COMMENT=10k|LAYER=TOPLAYER|X=10mm|Y=7mm|ROTATION=90.000|
|RECORD=Pad|NAME=1|COMPONENT=0|LAYER=TOPLAYER|NET=0|X=10mm|Y=6mm|XSIZE=1.2mm|YSIZE=1.4mm|SHAPE=RECTANGLE|HOLESIZE=0mm|ROTATION=90.000|
|RECORD=Pad|NAME=2|COMPONENT=0|LAYER=TOPLAYER|NET=1|X=10mm|Y=8mm|XSIZE=1.2mm|YSIZE=1.4mm|SHAPE=RECTANGLE|HOLESIZE=0mm|ROTATION=90.000|
|RECORD=Track|LAYER=TOPLAYER|NET=1|X1=10mm|Y1=8mm|X2=14mm|Y2=8mm|WIDTH=0.25mm|
|RECORD=Track|LAYER=BOTTOMLAYER|NET=0|X1=10mm|Y1=6mm|X2=4mm|Y2=6mm|WIDTH=0.3mm|
|RECORD=Via|X=14mm|Y=8mm|DIAMETER=0.6mm|HOLESIZE=0.3mm|STARTLAYER=TOPLAYER|ENDLAYER=BOTTOMLAYER|NET=1|
";

    #[test]
    fn parses_a_two_layer_board() {
        let pcb = parse_altium_ascii_pcb(BOARD).unwrap();

        assert_eq!(pcb.outline.vertices.len(), 4);
        assert!((pcb.outline.thickness - 1.6).abs() < 1e-9);
        assert_eq!(pcb.nets.len(), 2);
        assert_eq!(pcb.nets[0].name, "GND");

        assert_eq!(pcb.footprints.len(), 1);
        let fp = &pcb.footprints[0];
        assert_eq!(fp.reference, "R1");
        assert_eq!(fp.value, "10k");
        assert_eq!(fp.footprint_name, "Std:0805");
        assert!(fp.front);
        assert_eq!(fp.pads.len(), 2);

        assert_eq!(pcb.traces.len(), 2);
        assert_eq!(pcb.traces[0].layer, PcbLayer::FCu);
        assert_eq!(pcb.traces[0].net, "1");
        assert_eq!(pcb.traces[1].layer, PcbLayer::BCu);
        assert!((pcb.traces[1].width - 0.3).abs() < 1e-9);

        assert_eq!(pcb.vias.len(), 1);
        assert_eq!(pcb.vias[0].start_layer, PcbLayer::FCu);
        assert_eq!(pcb.vias[0].end_layer, PcbLayer::BCu);
    }

    /// The IR stores pads in the footprint frame and every consumer rebuilds
    /// world position as `fp + R(fp.rotation)·pad` — so that composition must
    /// reproduce the absolute coordinates Altium wrote.
    #[test]
    fn pad_positions_round_trip_through_the_footprint_frame() {
        let pcb = parse_altium_ascii_pcb(BOARD).unwrap();
        let fp = &pcb.footprints[0];
        let a = fp.rotation.to_radians();
        let (s, c) = a.sin_cos();
        let world = |p: vcad_ir::Vec2| {
            vcad_ir::Vec2::new(
                fp.position.x + p.x * c - p.y * s,
                fp.position.y + p.x * s + p.y * c,
            )
        };
        let p1 = world(fp.pads[0].position);
        let p2 = world(fp.pads[1].position);
        assert!(
            (p1.x - 10.0).abs() < 1e-9 && (p1.y - 6.0).abs() < 1e-9,
            "{p1:?}"
        );
        assert!(
            (p2.x - 10.0).abs() < 1e-9 && (p2.y - 8.0).abs() < 1e-9,
            "{p2:?}"
        );
        // Pad rotation is also relative, so absolute rotation comes back too.
        assert!((fp.rotation + fp.pads[0].rotation - 90.0).abs() < 1e-9);
    }

    #[test]
    fn bottom_side_components_round_trip_too() {
        let text = "\
|RECORD=Component|SOURCEDESIGNATOR=U2|PATTERN=SOT23|LAYER=BOTTOMLAYER|X=5mm|Y=5mm|ROTATION=180.000|
|RECORD=Pad|NAME=1|COMPONENT=0|LAYER=BOTTOMLAYER|X=6mm|Y=5.5mm|XSIZE=0.6mm|YSIZE=0.9mm|SHAPE=RECTANGLE|HOLESIZE=0mm|
";
        let pcb = parse_altium_ascii_pcb(text).unwrap();
        let fp = &pcb.footprints[0];
        assert!(!fp.front);
        let a = fp.rotation.to_radians();
        let (s, c) = a.sin_cos();
        let p = fp.pads[0].position;
        let wx = fp.position.x + p.x * c - p.y * s;
        let wy = fp.position.y + p.x * s + p.y * c;
        assert!((wx - 6.0).abs() < 1e-9 && (wy - 5.5).abs() < 1e-9);
        assert_eq!(fp.pads[0].layers, vec![PcbLayer::BCu]);
    }

    #[test]
    fn through_hole_pads_get_a_drill_and_both_copper_layers() {
        let text = "\
|RECORD=Component|SOURCEDESIGNATOR=J1|PATTERN=HDR|LAYER=TOPLAYER|X=0mm|Y=0mm|ROTATION=0|
|RECORD=Pad|NAME=1|COMPONENT=0|LAYER=MULTILAYER|X=0mm|Y=0mm|XSIZE=1.6mm|YSIZE=1.6mm|SHAPE=ROUND|HOLESIZE=0.9mm|PLATED=TRUE|
|RECORD=Pad|NAME=MH|COMPONENT=0|LAYER=MULTILAYER|X=3mm|Y=0mm|XSIZE=3.2mm|YSIZE=3.2mm|SHAPE=ROUND|HOLESIZE=3.2mm|PLATED=FALSE|
";
        let pcb = parse_altium_ascii_pcb(text).unwrap();
        let pads = &pcb.footprints[0].pads;
        assert_eq!(pads[0].pad_type, PadType::THT);
        assert!((pads[0].drill.as_ref().unwrap().diameter - 0.9).abs() < 1e-9);
        assert_eq!(pads[0].layers, vec![PcbLayer::FCu, PcbLayer::BCu]);
        assert_eq!(pads[1].pad_type, PadType::NPTH);
    }

    #[test]
    fn mechanical_1_graphics_become_the_outline_when_the_board_record_has_none() {
        let text = "\
|RECORD=Track|LAYER=MECHANICAL1|X1=0mm|Y1=0mm|X2=10mm|Y2=0mm|WIDTH=0.1mm|
|RECORD=Track|LAYER=MECHANICAL1|X1=10mm|Y1=0mm|X2=10mm|Y2=8mm|WIDTH=0.1mm|
|RECORD=Track|LAYER=MECHANICAL1|X1=10mm|Y1=8mm|X2=0mm|Y2=8mm|WIDTH=0.1mm|
|RECORD=Track|LAYER=MECHANICAL1|X1=0mm|Y1=8mm|X2=0mm|Y2=0mm|WIDTH=0.1mm|
";
        let pcb = parse_altium_ascii_pcb(text).unwrap();
        assert_eq!(pcb.outline.vertices.len(), 4);
        // Outline graphics are not copper.
        assert!(pcb.traces.is_empty());
    }

    #[test]
    fn binary_input_is_rejected_rather_than_silently_empty() {
        let err = parse_altium_ascii_pcb("\u{d0}\u{cf}\u{11}\u{e0}garbage").unwrap_err();
        assert!(err.contains("PCB ASCII"), "{err}");
    }

    #[test]
    fn unknown_nets_and_records_are_ignored_not_fatal() {
        let text = "\
|RECORD=Net|NAME=GND|
|RECORD=SomethingNew|FOO=BAR|
|RECORD=Track|LAYER=TOPLAYER|NET=99|X1=0mm|Y1=0mm|X2=1mm|Y2=0mm|WIDTH=0.2mm|
";
        let pcb = parse_altium_ascii_pcb(text).unwrap();
        assert_eq!(pcb.traces.len(), 1);
        // Out-of-range net index degrades to unnetted copper, not a bogus net.
        assert_eq!(pcb.traces[0].net, "");
    }
}
