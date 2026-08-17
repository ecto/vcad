//! Native binary Altium `.PcbDoc` / `.PcbLib` reader.
//!
//! Altium's native files are OLE compound files (CFB). Inside, each object
//! family gets a storage with a `Data` stream, in one of two flavours:
//!
//! * **Parameter streams** (`Board6`, `Nets6`, `Components6`, `Rules6`,
//!   `Classes6`) hold a sequence of `[u32 length][ASCII, NUL-terminated]`
//!   blocks whose text is exactly the `|KEY=VALUE|` vocabulary the ASCII
//!   export uses. These are decoded losslessly.
//! * **Primitive streams** (`Tracks6`, `Arcs6`, `Vias6`, `Pads6`, `Fills6`)
//!   hold packed fixed-layout records: a type byte, a `u32` payload length,
//!   then the payload. The layouts below are reconstructed from the
//!   open-source ecosystem (KiCad's Altium plugin, `altium2kicad`) — Altium
//!   publishes no specification.
//!
//! Because a wrong offset in a reconstructed layout yields *plausible* garbage
//! rather than an error, every decoded primitive is checked against
//! [`Extents`] and a plausible-dimension range before it is accepted. A stream
//! that produces implausible geometry aborts the import (see [`Validation`])
//! instead of handing the rest of the toolchain a board that is quietly wrong.

use std::io::{Cursor, Read, Seek};

use vcad_ir::ecad::Pcb;

use super::{
    ascii::{parse_bare_record, scan_records},
    build_footprint_lib, build_pcb, Extents, Record, RecordSet, MM_PER_INTERNAL,
};
use crate::kicad_mod::FootprintLib;

/// Altium primitive record type byte, as used in `.PcbLib` mixed streams and
/// as the leading byte of each `.PcbDoc` primitive record.
mod rec {
    pub const ARC: u8 = 1;
    pub const PAD: u8 = 2;
    pub const VIA: u8 = 3;
    pub const TRACK: u8 = 4;
    pub const TEXT: u8 = 5;
    pub const FILL: u8 = 6;
}

/// Maximum plausible copper dimension (mm). Altium's largest workspace is
/// about 100 inches; a trace wider than 100 mm is a decode error.
const MAX_DIM_MM: f64 = 100.0;

/// How many implausible records a stream may produce before the import is
/// declared a decode failure rather than a board with odd geometry.
///
/// Zero: a reconstructed layout that is right produces no implausible records
/// at all, so any failure means the offsets are wrong for this file's Altium
/// version, and everything decoded from that stream is suspect.
struct Validation {
    extents: Extents,
    stream: &'static str,
}

impl Validation {
    fn reject(&self, what: &str) -> String {
        format!(
            "could not decode the binary `{}` stream ({what}). This file's Altium \
             version uses a record layout this importer does not recognise. \
             Re-export from Altium with File > Save As > PCB ASCII and import that \
             instead — geometry is not imported partially.",
            self.stream
        )
    }

    fn point(&self, p: vcad_ir::Vec2) -> Result<vcad_ir::Vec2, String> {
        if !self.extents.contains(p) {
            return Err(self.reject("coordinate outside the board workspace"));
        }
        Ok(p)
    }

    fn dim(&self, v: f64, what: &str) -> Result<f64, String> {
        if !(v.is_finite() && v > 0.0 && v <= MAX_DIM_MM) {
            return Err(self.reject(what));
        }
        Ok(v)
    }
}

// ============================================================================
// Byte reader
// ============================================================================

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let slice = self.buf.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn i32(&mut self) -> Option<i32> {
        self.u32().map(|v| v as i32)
    }

    fn f64(&mut self) -> Option<f64> {
        self.take(8)
            .map(|b| f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
    }

    fn skip(&mut self, n: usize) -> Option<()> {
        self.take(n).map(|_| ())
    }

    /// One Altium internal-unit coordinate, in millimetres.
    fn coord_mm(&mut self) -> Option<f64> {
        self.i32().map(|v| v as f64 * MM_PER_INTERNAL)
    }

    fn point_mm(&mut self) -> Option<vcad_ir::Vec2> {
        Some(vcad_ir::Vec2::new(self.coord_mm()?, self.coord_mm()?))
    }
}

// ============================================================================
// CFB access
// ============================================================================

fn open_cfb(bytes: &[u8]) -> Result<cfb::CompoundFile<Cursor<Vec<u8>>>, String> {
    // Cheap discrimination before handing bytes to the CFB reader, so an ASCII
    // file misrouted here gets a useful message.
    if !bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        return Err(
            "not an OLE compound file — this is not a native binary Altium file. \
             If it is an ASCII export, import it as ASCII."
                .into(),
        );
    }
    cfb::CompoundFile::open(Cursor::new(bytes.to_vec()))
        .map_err(|e| format!("could not open the Altium compound file: {e}"))
}

fn read_stream<F: Read + Seek>(cf: &mut cfb::CompoundFile<F>, path: &str) -> Option<Vec<u8>> {
    let mut s = cf.open_stream(path).ok()?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// Read a `<name>/Data` stream, tolerating the leading-slash spellings CFB
/// writers vary on.
fn read_data<F: Read + Seek>(cf: &mut cfb::CompoundFile<F>, name: &str) -> Option<Vec<u8>> {
    read_stream(cf, &format!("/{name}/Data")).or_else(|| read_stream(cf, &format!("{name}/Data")))
}

// ============================================================================
// Parameter streams
// ============================================================================

/// Decode a parameter stream: repeated `[u32 len][len bytes of ASCII]`.
///
/// The text inside is the same `|KEY=VALUE|` vocabulary the ASCII export uses,
/// with one difference that matters: binary records carry **no** `RECORD=`
/// key, because the stream they live in already says what they are. `kind` is
/// that stream's record kind, stamped onto every block so the shared builder
/// sees the ASCII shape.
fn parse_param_stream(buf: &[u8], kind: &str) -> Vec<Record> {
    let mut out = Vec::new();
    let mut r = Reader::new(buf);
    while r.remaining() > 4 {
        let Some(len) = r.u32() else { break };
        let len = len as usize;
        if len == 0 || len > r.remaining() {
            break;
        }
        let Some(bytes) = r.take(len) else { break };
        let text = String::from_utf8_lossy(bytes);
        let text = text.trim_end_matches('\0');
        // A block can carry BOTH shapes: `Board6` opens with the bare board
        // record (which is where the outline vertices live) and then appends a
        // `|RECORD=Board|` sub-record for the layer stack. Dropping the lead-in
        // because a `|RECORD=` appears later loses the board outline, so parse
        // both halves.
        let (lead, rest) = match text.find("|RECORD=") {
            Some(i) => (&text[..i], &text[i..]),
            None => (text, ""),
        };
        if lead.contains('=') {
            out.push(parse_bare_record(lead, kind));
        }
        if !rest.is_empty() {
            out.extend(scan_records(rest));
        }
    }
    out
}

// ============================================================================
// Primitive streams
// ============================================================================

/// The 13-byte header every PCB primitive record shares: layer, two flag
/// bytes, net index, an unused pair, the owning component index, and four
/// reserved bytes.
struct Header {
    layer: u8,
    net: i64,
    component: i64,
}

fn read_header(r: &mut Reader<'_>) -> Option<Header> {
    let layer = r.u8()?;
    r.skip(2)?; // flags
    let net = r.u16()?;
    r.skip(2)?;
    let component = r.u16()?;
    r.skip(4)?;
    Some(Header {
        layer,
        // Altium writes 0xFFFF for "no net"/"no component".
        net: if net == 0xFFFF { -1 } else { net as i64 },
        component: if component == 0xFFFF {
            -1
        } else {
            component as i64
        },
    })
}

/// Common record framing: `[u8 type][u32 payload length][payload]`.
///
/// Returns `(type, payload)` pairs. A stream that runs out mid-record is a
/// decode failure, not a truncated-but-usable import.
fn split_records(buf: &[u8], v: &Validation) -> Result<Vec<(u8, Vec<u8>)>, String> {
    let mut out = Vec::new();
    let mut r = Reader::new(buf);
    while r.remaining() >= 5 {
        let ty = r.u8().ok_or_else(|| v.reject("truncated record type"))?;
        let len = r.u32().ok_or_else(|| v.reject("truncated record length"))? as usize;
        if len > r.remaining() {
            return Err(v.reject("record length past the end of the stream"));
        }
        let payload = r
            .take(len)
            .ok_or_else(|| v.reject("truncated record payload"))?;
        out.push((ty, payload.to_vec()));
    }
    Ok(out)
}

fn layer_name(id: u8) -> String {
    // The builder re-parses layer names, so emit the ASCII spellings rather
    // than threading a second layer vocabulary through it.
    use super::AltiumLayer::*;
    match super::AltiumLayer::from_id(id) {
        Top => "TOPLAYER".into(),
        Bottom => "BOTTOMLAYER".into(),
        Mid(i) => format!("MIDLAYER{i}"),
        Plane(i) => format!("INTERNALPLANE{i}"),
        TopOverlay => "TOPOVERLAY".into(),
        BottomOverlay => "BOTTOMOVERLAY".into(),
        TopSolder => "TOPSOLDER".into(),
        BottomSolder => "BOTTOMSOLDER".into(),
        TopPaste => "TOPPASTE".into(),
        BottomPaste => "BOTTOMPASTE".into(),
        Mechanical(i) => format!("MECHANICAL{i}"),
        KeepOut => "KEEPOUTLAYER".into(),
        MultiLayer => "MULTILAYER".into(),
        Other => "UNKNOWN".into(),
    }
}

fn mm(v: f64) -> String {
    format!("{v}mm")
}

fn decode_track(payload: &[u8], v: &Validation) -> Result<Record, String> {
    let mut r = Reader::new(payload);
    let h = read_header(&mut r).ok_or_else(|| v.reject("short track record"))?;
    let start = r.point_mm().ok_or_else(|| v.reject("short track record"))?;
    let end = r.point_mm().ok_or_else(|| v.reject("short track record"))?;
    let width = r.coord_mm().ok_or_else(|| v.reject("short track record"))?;
    let (start, end) = (v.point(start)?, v.point(end)?);
    let width = v.dim(width, "implausible track width")?;
    let mut rec = Record::default();
    rec.set("RECORD", "TRACK");
    rec.set("LAYER", layer_name(h.layer));
    rec.set("NET", h.net.to_string());
    rec.set("COMPONENT", h.component.to_string());
    rec.set("X1", mm(start.x));
    rec.set("Y1", mm(start.y));
    rec.set("X2", mm(end.x));
    rec.set("Y2", mm(end.y));
    rec.set("WIDTH", mm(width));
    Ok(rec)
}

fn decode_arc(payload: &[u8], v: &Validation) -> Result<Record, String> {
    let mut r = Reader::new(payload);
    let h = read_header(&mut r).ok_or_else(|| v.reject("short arc record"))?;
    let center = r.point_mm().ok_or_else(|| v.reject("short arc record"))?;
    let radius = r.coord_mm().ok_or_else(|| v.reject("short arc record"))?;
    let start_angle = r.f64().ok_or_else(|| v.reject("short arc record"))?;
    let end_angle = r.f64().ok_or_else(|| v.reject("short arc record"))?;
    let width = r.coord_mm().ok_or_else(|| v.reject("short arc record"))?;
    let center = v.point(center)?;
    let radius = v.dim(radius, "implausible arc radius")?;
    let width = v.dim(width, "implausible arc width")?;
    if !(start_angle.is_finite() && end_angle.is_finite())
        || start_angle.abs() > 3600.0
        || end_angle.abs() > 3600.0
    {
        return Err(v.reject("implausible arc angles"));
    }
    let mut rec = Record::default();
    rec.set("RECORD", "ARC");
    rec.set("LAYER", layer_name(h.layer));
    rec.set("NET", h.net.to_string());
    rec.set("COMPONENT", h.component.to_string());
    rec.set("LOCATION.X", mm(center.x));
    rec.set("LOCATION.Y", mm(center.y));
    rec.set("RADIUS", mm(radius));
    rec.set("STARTANGLE", start_angle.to_string());
    rec.set("ENDANGLE", end_angle.to_string());
    rec.set("WIDTH", mm(width));
    Ok(rec)
}

fn decode_via(payload: &[u8], v: &Validation) -> Result<Record, String> {
    let mut r = Reader::new(payload);
    let h = read_header(&mut r).ok_or_else(|| v.reject("short via record"))?;
    let position = r.point_mm().ok_or_else(|| v.reject("short via record"))?;
    let diameter = r.coord_mm().ok_or_else(|| v.reject("short via record"))?;
    let drill = r.coord_mm().ok_or_else(|| v.reject("short via record"))?;
    let start_layer = r.u8().ok_or_else(|| v.reject("short via record"))?;
    let end_layer = r.u8().ok_or_else(|| v.reject("short via record"))?;
    let position = v.point(position)?;
    let diameter = v.dim(diameter, "implausible via diameter")?;
    let drill = v.dim(drill, "implausible via drill")?;
    // Deliberately loose. `drill == diameter` is real data — library patterns
    // use ring-less "vias" as mounting holes — so treating it as a decode
    // failure rejects valid files. Only a gross mismatch indicates that the
    // fields were read from the wrong offsets; ordinary bounds checks above
    // are what actually detect a misaligned layout.
    if drill > diameter * 4.0 {
        return Err(v.reject("via drill grossly larger than its pad"));
    }
    let mut rec = Record::default();
    rec.set("RECORD", "VIA");
    rec.set("NET", h.net.to_string());
    rec.set("X", mm(position.x));
    rec.set("Y", mm(position.y));
    rec.set("DIAMETER", mm(diameter));
    rec.set("HOLESIZE", mm(drill));
    // Verified against real files: these two bytes hold the span's copper layer
    // ids (1 = Top, 32 = Bottom for a through via). A non-copper id here means
    // the layout is wrong, so reject rather than quietly calling a blind via a
    // through via — the span drives DRC and the fabrication drill files.
    for (id, what) in [(start_layer, "start"), (end_layer, "end")] {
        if id != 0 && super::AltiumLayer::from_id(id).is_copper_family() {
            continue;
        }
        if id != 0 {
            return Err(v.reject(&format!("via {what} layer is not a copper layer")));
        }
    }
    rec.set("STARTLAYER", layer_name(start_layer));
    rec.set("ENDLAYER", layer_name(end_layer));
    Ok(rec)
}

fn decode_fill(payload: &[u8], v: &Validation) -> Result<Record, String> {
    let mut r = Reader::new(payload);
    let h = read_header(&mut r).ok_or_else(|| v.reject("short fill record"))?;
    let a = r.point_mm().ok_or_else(|| v.reject("short fill record"))?;
    let b = r.point_mm().ok_or_else(|| v.reject("short fill record"))?;
    let (a, b) = (v.point(a)?, v.point(b)?);
    // A fill is a filled rectangle; the IR has no board-level fill primitive,
    // so express it as its four boundary tracks at hairline width. That keeps
    // the copper visible to DRC/render without inventing a zone the file
    // does not describe.
    let mut rec = Record::default();
    rec.set("RECORD", "FILL");
    rec.set("LAYER", layer_name(h.layer));
    rec.set("NET", h.net.to_string());
    rec.set("X1", mm(a.x));
    rec.set("Y1", mm(a.y));
    rec.set("X2", mm(b.x));
    rec.set("Y2", mm(b.y));
    Ok(rec)
}

/// Altium pad shape codes.
fn pad_shape_name(code: u8) -> &'static str {
    match code {
        2 => "RECTANGLE",
        3 => "OCTAGONAL",
        9 => "ROUNDEDRECTANGLE",
        _ => "ROUND",
    }
}

/// Pads are the one primitive that does **not** carry a whole-record length.
///
/// A pad record is a `0x02` type byte followed by six consecutive
/// length-prefixed sub-blocks (`[u32 len][len bytes]`). Sub-block 0 is the pad
/// name (`[u8 namelen][chars]`); sub-block 4 is the geometry. Treating the
/// first `u32` as a record length — the framing every other primitive uses —
/// walks straight off the end of the stream, which is exactly what real
/// Altium files did before this was corrected.
///
/// Geometry layout, relative to sub-block 4's first byte: layer(0), net(3,
/// i16), component(7, i16), x(13), y(17), top size(21,25), mid size(29,33),
/// bottom size(37,41), hole(45), top/mid/bottom shape(49,50,51),
/// rotation(52, f64), plated(60).
fn split_pad_record(r: &mut Reader<'_>, v: &Validation) -> Result<(String, Vec<u8>), String> {
    let ty = r.u8().ok_or_else(|| v.reject("truncated pad record"))?;
    if ty != rec::PAD {
        return Err(v.reject("pad record does not start with the 0x02 type byte"));
    }
    let mut blocks: Vec<Vec<u8>> = Vec::with_capacity(6);
    for _ in 0..6 {
        let len = r
            .u32()
            .ok_or_else(|| v.reject("truncated pad sub-block length"))? as usize;
        if len > r.remaining() {
            return Err(v.reject("pad sub-block length past the end of the stream"));
        }
        blocks.push(
            r.take(len)
                .ok_or_else(|| v.reject("truncated pad sub-block"))?
                .to_vec(),
        );
    }
    let name = {
        let b = &blocks[0];
        let n = *b.first().unwrap_or(&0) as usize;
        String::from_utf8_lossy(b.get(1..(1 + n).min(b.len())).unwrap_or(&[]))
            .trim_end_matches('\0')
            .to_string()
    };
    Ok((name, blocks.swap_remove(4)))
}

fn decode_pad(name: String, geom: &[u8], v: &Validation) -> Result<Record, String> {
    let mut g = Reader::new(geom);
    let h = read_header(&mut g).ok_or_else(|| v.reject("short pad geometry"))?;
    let position = g.point_mm().ok_or_else(|| v.reject("short pad geometry"))?;
    let top = g.point_mm().ok_or_else(|| v.reject("short pad geometry"))?;
    let _mid = g.point_mm().ok_or_else(|| v.reject("short pad geometry"))?;
    let _bot = g.point_mm().ok_or_else(|| v.reject("short pad geometry"))?;
    let hole = g.coord_mm().ok_or_else(|| v.reject("short pad geometry"))?;
    let top_shape = g.u8().ok_or_else(|| v.reject("short pad geometry"))?;
    g.skip(2).ok_or_else(|| v.reject("short pad geometry"))?;
    let rotation = g.f64().ok_or_else(|| v.reject("short pad geometry"))?;
    let plated = g.u8().ok_or_else(|| v.reject("short pad geometry"))? != 0;

    let position = v.point(position)?;
    let w = v.dim(top.x, "implausible pad width")?;
    let hgt = v.dim(top.y, "implausible pad height")?;
    if !rotation.is_finite() || rotation.abs() > 3600.0 {
        return Err(v.reject("implausible pad rotation"));
    }
    if !(hole.is_finite() && (0.0..=MAX_DIM_MM).contains(&hole)) {
        return Err(v.reject("implausible pad hole size"));
    }

    let mut rec = Record::default();
    rec.set("RECORD", "PAD");
    rec.set("NAME", name);
    rec.set("LAYER", layer_name(h.layer));
    rec.set("NET", h.net.to_string());
    rec.set("COMPONENT", h.component.to_string());
    rec.set("X", mm(position.x));
    rec.set("Y", mm(position.y));
    rec.set("XSIZE", mm(w));
    rec.set("YSIZE", mm(hgt));
    rec.set("SHAPE", pad_shape_name(top_shape));
    rec.set("HOLESIZE", mm(hole));
    rec.set("ROTATION", rotation.to_string());
    rec.set("PLATED", if plated { "TRUE" } else { "FALSE" });
    Ok(rec)
}

/// Walk a whole `Pads6` stream, which is a run of pad records back to back.
fn decode_pad_stream(buf: &[u8], v: &Validation) -> Result<Vec<Record>, String> {
    let mut r = Reader::new(buf);
    let mut out = Vec::new();
    while r.remaining() > 25 {
        let (name, geom) = split_pad_record(&mut r, v)?;
        out.push(decode_pad(name, &geom, v)?);
    }
    Ok(out)
}

/// Walk a stream that mixes primitive families, as `.PcbLib` footprint streams
/// do: pads use the pad framing, everything else uses `[u8 type][u32 len]`.
fn decode_mixed_records(buf: &[u8], v: &Validation) -> Result<Vec<Record>, String> {
    let mut r = Reader::new(buf);
    let mut out = Vec::new();
    while r.remaining() >= 5 {
        // Peek the type byte without consuming it; the pad reader wants it.
        let ty = r.buf[r.pos];
        if ty == rec::PAD {
            let (name, geom) = split_pad_record(&mut r, v)?;
            out.push(decode_pad(name, &geom, v)?);
            continue;
        }
        r.u8();
        let len = r.u32().ok_or_else(|| v.reject("truncated record length"))? as usize;
        if len > r.remaining() {
            return Err(v.reject("record length past the end of the stream"));
        }
        let payload = r
            .take(len)
            .ok_or_else(|| v.reject("truncated record payload"))?
            .to_vec();
        // Text records are followed by a second length-prefixed block holding
        // the string itself. Skipping it desynchronises the whole walk, which
        // is how a library first failed to import.
        if ty == rec::TEXT {
            let extra = r
                .u32()
                .ok_or_else(|| v.reject("truncated text string length"))?
                as usize;
            if extra > r.remaining() {
                return Err(v.reject("text string length past the end of the stream"));
            }
            r.skip(extra);
        }
        if let Some(prim) = decode_primitive(ty, &payload, v)? {
            out.push(prim);
        }
    }
    Ok(out)
}

/// Decode one primitive by its type byte. Pads are absent here: they use their
/// own framing and are handled by [`decode_pad_stream`]. Unknown types are
/// skipped — Altium adds object families across versions and an unrecognised
/// one is not a reason to refuse the copper we do understand.
fn decode_primitive(ty: u8, payload: &[u8], v: &Validation) -> Result<Option<Record>, String> {
    Ok(match ty {
        rec::TRACK => Some(decode_track(payload, v)?),
        rec::ARC => Some(decode_arc(payload, v)?),
        rec::VIA => Some(decode_via(payload, v)?),
        rec::FILL => Some(decode_fill(payload, v)?),
        _ => None,
    })
}

/// Read one per-type primitive stream from a `.PcbDoc`.
fn read_primitive_stream<F: Read + Seek>(
    cf: &mut cfb::CompoundFile<F>,
    storage: &'static str,
    ty: u8,
    extents: Extents,
) -> Result<Vec<Record>, String> {
    let Some(buf) = read_data(cf, storage) else {
        return Ok(vec![]);
    };
    let v = Validation {
        extents,
        stream: storage,
    };
    if ty == rec::PAD {
        return decode_pad_stream(&buf, &v);
    }
    let mut out = Vec::new();
    for (rty, payload) in split_records(&buf, &v)? {
        if let Some(r) = decode_primitive(rty, &payload, &v)? {
            out.push(r);
        }
    }
    Ok(out)
}

// ============================================================================
// Public entry points
// ============================================================================

/// Parse a native binary Altium `.PcbDoc` into a [`Pcb`].
///
/// Fails closed: any primitive stream whose reconstructed layout does not
/// decode to plausible geometry aborts the import with a message pointing at
/// the ASCII export path, rather than returning a partially-correct board.
pub fn parse_altium_pcbdoc(bytes: &[u8]) -> Result<Pcb, String> {
    let mut cf = open_cfb(bytes)?;

    let mut records: Vec<Record> = Vec::new();
    // Parameter streams first, and in this order: the builder indexes nets and
    // components by their position in the record list.
    for (storage, kind) in [
        ("Board6", "Board"),
        ("Nets6", "Net"),
        ("Components6", "Component"),
        ("Rules6", "Rule"),
    ] {
        if let Some(buf) = read_data(&mut cf, storage) {
            records.extend(parse_param_stream(&buf, kind));
        }
    }
    if records.is_empty() {
        return Err(
            "this compound file has no Altium PCB parameter streams (Board6/Nets6/\
             Components6). It may be a schematic (.SchDoc) or a newer file format."
                .into(),
        );
    }

    let extents = Extents::permissive();
    for (storage, ty) in [
        ("Tracks6", rec::TRACK),
        ("Arcs6", rec::ARC),
        ("Vias6", rec::VIA),
        ("Pads6", rec::PAD),
        ("Fills6", rec::FILL),
    ] {
        records.extend(read_primitive_stream(&mut cf, storage, ty, extents)?);
    }

    build_pcb(RecordSet { records })
}

/// Parse an Altium `.PcbLib` footprint library.
///
/// Handles both flavours: an ASCII-exported library is scanned as text, and a
/// native binary library is read from its CFB storages — one storage per
/// footprint pattern, each holding a header parameter block followed by mixed
/// primitive records tagged with their type byte.
pub fn parse_altium_pcblib(bytes: &[u8]) -> Result<FootprintLib, String> {
    // ASCII export: no CFB signature, plain `|RECORD=` text.
    if !bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
        let text = String::from_utf8_lossy(bytes);
        let records = scan_records(&text);
        if records.is_empty() {
            return Err(
                "not a recognisable Altium library: neither an OLE compound file nor \
                 ASCII |RECORD=…| text."
                    .into(),
            );
        }
        return build_footprint_lib(RecordSet { records });
    }

    let mut cf = open_cfb(bytes)?;
    let extents = Extents::permissive();
    let v = Validation {
        extents,
        stream: "PcbLib footprint",
    };

    // Footprint storages are every child of the root except the bookkeeping
    // ones Altium reserves.
    let reserved = [
        "FileHeader",
        "Library",
        "FileVersionInfo",
        "ComponentParamsTOC",
        "Textures",
        "Models",
    ];
    let names: Vec<String> = cf
        .read_storage("/")
        .map_err(|e| format!("could not list the library's storages: {e}"))?
        .filter(|e| e.is_storage())
        .map(|e| e.name().to_string())
        .filter(|n| !reserved.iter().any(|r| r.eq_ignore_ascii_case(n)))
        .collect();

    let mut records: Vec<Record> = Vec::new();
    let mut patterns = 0usize;
    for name in names {
        let Some(buf) = read_data(&mut cf, &name) else {
            continue;
        };
        // A footprint stream opens with `[u32 len][u8 namelen][name]` — a
        // Pascal string, not a parameter block — and the primitives follow.
        let mut r = Reader::new(&buf);
        let header_len = r.u32().unwrap_or(0) as usize;
        if header_len == 0 || header_len > r.remaining() {
            continue;
        }
        let header = r.take(header_len).unwrap_or(&[]);
        let pattern = {
            let n = *header.first().unwrap_or(&0) as usize;
            String::from_utf8_lossy(header.get(1..(1 + n).min(header.len())).unwrap_or(&[]))
                .trim_end_matches('\0')
                .trim()
                .to_string()
        };
        let pattern = if pattern.is_empty() {
            name.clone()
        } else {
            pattern
        };

        // The library's "component" anchors the pattern at the origin; its
        // primitives carry coordinates in that same frame, so the builder's
        // absolute → local conversion is the identity here.
        let component_index = patterns;
        let mut comp = Record::default();
        comp.set("RECORD", "COMPONENT");
        comp.set("SOURCEDESIGNATOR", pattern.clone());
        comp.set("PATTERN", pattern);
        comp.set("LAYER", "TOPLAYER");
        comp.set("X", "0mm");
        comp.set("Y", "0mm");
        comp.set("ROTATION", "0");
        records.push(comp);
        patterns += 1;

        let body = buf[r.pos..].to_vec();
        for mut prim in decode_mixed_records(&body, &v)? {
            // Library primitives have no board component index of their own;
            // bind them to the pattern they were found in.
            prim.set("COMPONENT", component_index.to_string());
            // Library geometry carries no netlist.
            prim.set("NET", "-1");
            records.push(prim);
        }
    }

    if patterns == 0 {
        return Err("no footprint pattern storages found in the library".into());
    }
    build_footprint_lib(RecordSet { records })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn le32(v: i32) -> [u8; 4] {
        (v as u32).to_le_bytes()
    }

    /// Millimetres → Altium internal units.
    fn iu(mm: f64) -> i32 {
        (mm / MM_PER_INTERNAL).round() as i32
    }

    fn header_bytes(layer: u8, net: u16, component: u16) -> Vec<u8> {
        let mut b = vec![layer, 0, 0];
        b.extend(net.to_le_bytes());
        b.extend([0, 0]);
        b.extend(component.to_le_bytes());
        b.extend([0, 0, 0, 0]);
        b
    }

    fn track_payload(layer: u8, net: u16, a: (f64, f64), c: (f64, f64), w: f64) -> Vec<u8> {
        let mut b = header_bytes(layer, net, 0xFFFF);
        for v in [a.0, a.1, c.0, c.1, w] {
            b.extend(le32(iu(v)));
        }
        b
    }

    fn framed(ty: u8, payload: &[u8]) -> Vec<u8> {
        let mut b = vec![ty];
        b.extend((payload.len() as u32).to_le_bytes());
        b.extend(payload);
        b
    }

    fn validation() -> Validation {
        Validation {
            extents: Extents::permissive(),
            stream: "Tracks6",
        }
    }

    #[test]
    fn decodes_a_track_record() {
        let payload = track_payload(1, 3, (1.0, 2.0), (5.0, 2.0), 0.25);
        let rec = decode_track(&payload, &validation()).unwrap();
        assert_eq!(rec.kind(), "TRACK");
        assert_eq!(rec.get("LAYER"), Some("TOPLAYER"));
        assert_eq!(rec.get("NET"), Some("3"));
        assert!((rec.len_mm("X1").unwrap() - 1.0).abs() < 1e-6);
        assert!((rec.len_mm("X2").unwrap() - 5.0).abs() < 1e-6);
        assert!((rec.len_mm("WIDTH").unwrap() - 0.25).abs() < 1e-6);
    }

    #[test]
    fn no_net_sentinel_becomes_minus_one() {
        let payload = track_payload(1, 0xFFFF, (0.0, 0.0), (1.0, 0.0), 0.2);
        let rec = decode_track(&payload, &validation()).unwrap();
        assert_eq!(rec.get("NET"), Some("-1"));
    }

    /// The whole point of the validation layer: a misaligned layout yields
    /// coordinates far outside any real workspace, and that must be an error
    /// rather than a board full of plausible-looking garbage.
    #[test]
    fn out_of_workspace_coordinates_fail_closed() {
        let mut payload = header_bytes(1, 0, 0xFFFF);
        for v in [i32::MAX / 2, 0, 0, 0, iu(0.25)] {
            payload.extend(le32(v));
        }
        let err = decode_track(&payload, &validation()).unwrap_err();
        assert!(err.contains("PCB ASCII"), "{err}");
    }

    #[test]
    fn zero_width_copper_fails_closed() {
        let payload = track_payload(1, 0, (0.0, 0.0), (1.0, 0.0), 0.0);
        assert!(decode_track(&payload, &validation()).is_err());
    }

    #[test]
    fn truncated_records_fail_closed() {
        let good = framed(
            rec::TRACK,
            &track_payload(1, 0, (0.0, 0.0), (1.0, 0.0), 0.2),
        );
        let mut buf = good.clone();
        // Claim a payload far longer than what follows.
        buf.push(rec::TRACK);
        buf.extend(9999u32.to_le_bytes());
        buf.extend([0u8; 8]);
        assert!(split_records(&buf, &validation()).is_err());
    }

    #[test]
    fn splits_a_well_formed_stream() {
        let mut buf = framed(
            rec::TRACK,
            &track_payload(1, 0, (0.0, 0.0), (1.0, 0.0), 0.2),
        );
        buf.extend(framed(
            rec::TRACK,
            &track_payload(32, 1, (0.0, 1.0), (1.0, 1.0), 0.3),
        ));
        let recs = split_records(&buf, &validation()).unwrap();
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].0, rec::TRACK);
    }

    #[test]
    fn parses_a_parameter_stream() {
        let text = "|RECORD=Net|NAME=GND|\0";
        let mut buf = (text.len() as u32).to_le_bytes().to_vec();
        buf.extend(text.as_bytes());
        let recs = parse_param_stream(&buf, "Net");
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].kind(), "NET");
        assert_eq!(recs[0].get("NAME"), Some("GND"));
    }

    #[test]
    fn non_compound_input_is_rejected() {
        let err = parse_altium_pcbdoc(b"|RECORD=Board|").unwrap_err();
        assert!(err.contains("not an OLE compound file"), "{err}");
    }

    /// Build a real CFB file with the streams a `.PcbDoc` carries and read it
    /// back — the only test that exercises the whole binary path end to end,
    /// including the compound-file layer and the record → `Pcb` builder.
    fn synth_pcbdoc() -> Vec<u8> {
        fn param(blocks: &[&str]) -> Vec<u8> {
            let mut out = Vec::new();
            for b in blocks {
                let mut text = b.to_string();
                text.push('\0');
                out.extend((text.len() as u32).to_le_bytes());
                out.extend(text.as_bytes());
            }
            out
        }

        let mut cf = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        let mut write = |storage: &str, data: Vec<u8>| {
            cf.create_storage(format!("/{storage}")).unwrap();
            let mut s = cf.create_stream(format!("/{storage}/Data")).unwrap();
            s.write_all(&data).unwrap();
            s.flush().unwrap();
        };

        write(
            "Board6",
            param(&["|RECORD=Board|BOARDTHICKNESS=1.6mm|VX0=0mm|VY0=0mm|VX1=20mm|VY1=0mm|VX2=20mm|VY2=15mm|VX3=0mm|VY3=15mm|"]),
        );
        write(
            "Nets6",
            param(&["|RECORD=Net|NAME=GND|", "|RECORD=Net|NAME=VCC|"]),
        );
        write(
            "Components6",
            param(&["|RECORD=Component|SOURCEDESIGNATOR=R1|PATTERN=0805|LAYER=TOPLAYER|X=10mm|Y=7mm|ROTATION=0|"]),
        );

        let mut tracks = framed(
            rec::TRACK,
            &track_payload(1, 1, (10.0, 8.0), (14.0, 8.0), 0.25),
        );
        tracks.extend(framed(
            rec::TRACK,
            &track_payload(32, 0, (10.0, 6.0), (4.0, 6.0), 0.3),
        ));
        write("Tracks6", tracks);

        let mut via = header_bytes(1, 1, 0xFFFF);
        for v in [iu(14.0), iu(8.0), iu(0.6), iu(0.3)] {
            via.extend(le32(v));
        }
        via.push(1); // start layer: top
        via.push(32); // end layer: bottom
        write("Vias6", framed(rec::VIA, &via));

        write("Pads6", pad_stream());

        cf.flush().unwrap();
        cf.into_inner().into_inner()
    }

    /// Two SMD pads on component 0, in Altium's real pad framing: a `0x02`
    /// type byte followed by six length-prefixed sub-blocks, with **no**
    /// whole-record length. Getting this wrong is what broke every real file
    /// on the first attempt, so the fixture encodes the corrected shape.
    fn pad_stream() -> Vec<u8> {
        let mut out = Vec::new();
        for (name, x, net) in [("1", 10.0f64, 0u16), ("2", 10.0, 1)] {
            let mut geom = header_bytes(1, net, 0);
            let y = if name == "1" { 6.0 } else { 8.0 };
            for v in [iu(x), iu(y)] {
                geom.extend(le32(v));
            }
            // top / mid / bottom sizes
            for _ in 0..3 {
                geom.extend(le32(iu(1.2)));
                geom.extend(le32(iu(1.4)));
            }
            geom.extend(le32(0)); // hole size: SMD
            geom.extend([2u8, 2, 2]); // shapes: rectangular
            geom.extend(0.0f64.to_le_bytes()); // rotation
            geom.push(0); // not plated
            assert!(
                geom.len() >= 61,
                "geometry subrecord too short: {}",
                geom.len()
            );

            let mut name_block = vec![name.len() as u8];
            name_block.extend(name.as_bytes());

            out.push(rec::PAD);
            for block in [name_block, vec![0], vec![0; 5], vec![0], geom, vec![]] {
                out.extend((block.len() as u32).to_le_bytes());
                out.extend(block);
            }
        }
        out
    }

    #[test]
    fn reads_a_synthesized_binary_pcbdoc_end_to_end() {
        let pcb = parse_altium_pcbdoc(&synth_pcbdoc()).unwrap();

        assert_eq!(pcb.outline.vertices.len(), 4);
        assert!((pcb.outline.thickness - 1.6).abs() < 1e-9);
        assert_eq!(pcb.nets.len(), 2);
        assert_eq!(pcb.nets[1].name, "VCC");

        assert_eq!(pcb.traces.len(), 2);
        assert_eq!(pcb.traces[0].layer, vcad_ir::ecad::PcbLayer::FCu);
        assert_eq!(pcb.traces[0].net, "1");
        assert!((pcb.traces[0].width - 0.25).abs() < 1e-6);
        assert_eq!(pcb.traces[1].layer, vcad_ir::ecad::PcbLayer::BCu);

        assert_eq!(pcb.vias.len(), 1);
        assert!((pcb.vias[0].drill - 0.3).abs() < 1e-6);
        assert_eq!(pcb.vias[0].end_layer, vcad_ir::ecad::PcbLayer::BCu);

        assert_eq!(pcb.footprints.len(), 1);
        let fp = &pcb.footprints[0];
        assert_eq!(fp.reference, "R1");
        assert_eq!(fp.pads.len(), 2, "both pads bound to their component");
        assert_eq!(fp.pads[0].number, "1");
        // Unrotated component: local == absolute - origin.
        assert!(
            (fp.pads[0].position.y + 1.0).abs() < 1e-6,
            "{:?}",
            fp.pads[0].position
        );
        assert_eq!(fp.pads[1].net.as_deref(), Some("1"));
    }

    /// The guard that matters: a file whose primitive stream does not match the
    /// reconstructed layout must abort, not import a plausible-looking board.
    #[test]
    fn a_misaligned_primitive_stream_aborts_the_whole_import() {
        let mut bytes = synth_pcbdoc();
        // Re-open and corrupt Tracks6 by shifting every payload one byte.
        let mut cf = cfb::CompoundFile::open(Cursor::new(bytes.clone())).unwrap();
        let mut buf = Vec::new();
        cf.open_stream("/Tracks6/Data")
            .unwrap()
            .read_to_end(&mut buf)
            .unwrap();
        let mut shifted = framed(rec::TRACK, &{
            let mut p = vec![0u8];
            p.extend(track_payload(1, 1, (10.0, 8.0), (14.0, 8.0), 0.25));
            // Drop the last byte so the coordinates land off by one byte,
            // which is exactly what a wrong-version layout looks like.
            p.truncate(p.len() - 1);
            p
        });
        shifted.truncate(shifted.len());
        {
            let mut s = cf.open_stream("/Tracks6/Data").unwrap();
            s.set_len(0).unwrap();
            s.seek(std::io::SeekFrom::Start(0)).unwrap();
            s.write_all(&shifted).unwrap();
            s.flush().unwrap();
        }
        cf.flush().unwrap();
        bytes = cf.into_inner().into_inner();

        let err = parse_altium_pcbdoc(&bytes).unwrap_err();
        assert!(err.contains("PCB ASCII"), "{err}");
    }

    /// A `Text` record carries a second length-prefixed block after its
    /// payload. Skipping it desynchronises the whole walk — this is what made
    /// a real `.PcbLib` fail to import.
    #[test]
    fn a_text_record_does_not_desynchronise_a_mixed_stream() {
        let mut buf = framed(rec::TEXT, &[0u8; 40]);
        buf.extend(9u32.to_le_bytes());
        buf.extend(b"REFDES 12");
        buf.extend(framed(
            rec::TRACK,
            &track_payload(1, 0, (1.0, 1.0), (2.0, 1.0), 0.2),
        ));
        let recs = decode_mixed_records(&buf, &validation()).unwrap();
        assert_eq!(recs.len(), 1, "the track after the text must still decode");
        assert_eq!(recs[0].kind(), "TRACK");
        // Altium's internal unit is 2.54 nm, so a round-trip through it lands
        // within ~1.3e-6 mm of the authored value.
        assert!((recs[0].len_mm("X2").unwrap() - 2.0).abs() < 1e-5);
    }

    /// `Board6` opens with a bare record (which is where the outline lives)
    /// and then appends a `|RECORD=Board|` sub-record. Keeping only the
    /// sub-record loses the board outline entirely.
    #[test]
    fn a_board_block_contributes_both_its_bare_lead_in_and_its_subrecord() {
        let text = "|SELECTION=FALSE|VX0=0mm|VY0=0mm|VX1=10mm|VY1=0mm|VX2=10mm|VY2=8mm                    |RECORD=Board|V9_MASTERSTACK_STYLE=0|";
        let mut buf = (text.len() as u32).to_le_bytes().to_vec();
        buf.extend(text.as_bytes());
        let recs = parse_param_stream(&buf, "Board");
        assert_eq!(recs.len(), 2);
        assert!(recs.iter().all(|r| r.kind() == "BOARD"));
        assert_eq!(recs[0].get("VX1"), Some("10mm"));
        assert_eq!(recs[1].get("V9_MASTERSTACK_STYLE"), Some("0"));
    }

    #[test]
    fn ascii_pcblib_takes_the_text_path() {
        let text = "\
|RECORD=Component|SOURCEDESIGNATOR=R_0805|PATTERN=R_0805|LAYER=TOPLAYER|X=0mm|Y=0mm|ROTATION=0|
|RECORD=Pad|NAME=1|COMPONENT=0|LAYER=TOPLAYER|X=-1mm|Y=0mm|XSIZE=1mm|YSIZE=1.2mm|SHAPE=RECTANGLE|HOLESIZE=0mm|
|RECORD=Pad|NAME=2|COMPONENT=0|LAYER=TOPLAYER|X=1mm|Y=0mm|XSIZE=1mm|YSIZE=1.2mm|SHAPE=RECTANGLE|HOLESIZE=0mm|
";
        let lib = parse_altium_pcblib(text.as_bytes()).unwrap();
        assert_eq!(lib.footprints.len(), 1);
        assert_eq!(lib.footprints[0].name, "R_0805");
        assert_eq!(lib.footprints[0].pads.len(), 2);
        assert!((lib.footprints[0].pads[0].position.0 + 1.0).abs() < 1e-9);
    }
}
