#!/usr/bin/env python3
"""V3 motor pipeline against the fixed toolchain (bridge on :8747).

Phase A: regression-verify the four merged fixes.
Phase B: full rebuild — stator board, clean assembly, clearances, irons, BOM.
Prints a compact ledger; exits nonzero on any hard failure.
"""
import json, math, sys, urllib.request

BASE = "http://127.0.0.1:8747"
FAIL = []

def call(name, args, expect_ok=True):
    req = urllib.request.Request(f"{BASE}/call", json.dumps({"name": name, "arguments": args}).encode())
    r = json.load(urllib.request.urlopen(req, timeout=120))
    if r.get("isError"):
        if expect_ok:
            FAIL.append(f"{name}: {r['text'][:150]}")
            print(f"  !! {name} ERROR: {r['text'][:150]}")
        return {"__error__": r["text"]}
    try:
        return json.loads(r["text"])
    except Exception:
        return {"__raw__": r["text"], "__images__": r.get("images", [])}

def check(label, cond, detail=""):
    print(f"  {'PASS' if cond else 'FAIL'}  {label}  {detail}")
    if not cond: FAIL.append(label)

def ngon(cx, cy, r, n, cw=False, aslist=True):
    pts = [[round(cx + r*math.cos(2*math.pi*i/n), 4), round(cy + r*math.sin(2*math.pi*i/n), 4)] for i in range(n)]
    if cw: pts = pts[::-1]
    return pts if aslist else [{"x": p[0], "y": p[1]} for p in pts]

P = lambda r, deg: (35 + r*math.cos(math.radians(deg)), 35 + r*math.sin(math.radians(deg)))

# ---------------------------------------------------------------- Phase A
print("== Phase A: fix regressions ==")

# A1 watertight flange-with-holes (#470) + verify_spec
rotor_iron = call("sheet_metal_create", {
    "width": 58, "depth": 58, "thickness": 2.7, "material": "steel-mild",
    "shop_profile": "sendcutsend", "outline": ngon(29, 29, 29, 64),
    "holes": [ngon(29, 29, 4.2, 16, cw=True)] + [
        ngon(29+11*math.cos(math.radians(a)), 29+11*math.sin(math.radians(a)), 1.65, 12, cw=True)
        for a in (0, 90, 180, 270)]})
RI = rotor_iron["document_id"]
vs = call("verify_spec", {"document_id": RI, "spec": {
    "volume": {"min": 6800, "max": 6950}, "watertight": True, "part_count": 1,
    "bbox_min": {"x": 0, "y": 0, "z": -1.36, "tol": 0.05},
    "bbox_max": {"x": 58, "y": 58, "z": 1.36, "tol": 0.05}}})
s = vs.get("summary", {})
check("A1 flange mesh spec (watertight+volume)", s.get("overall") == "pass", json.dumps(s))

# A2 apply_edits @N refs + consumption (#471)
probe = call("open_document", {})
pd = probe["document_id"]
b = call("apply_edits", {"document_id": pd, "ops": [
    {"op": "create", "type": "cylinder", "params": {"radius": 10, "height": 5}},
    {"op": "create", "type": "translate", "params": {"child": "@0", "offset": {"x": 35, "y": 35, "z": 0}}},
    {"op": "create", "type": "cylinder", "params": {"radius": 3, "height": 9}},
    {"op": "create", "type": "translate", "params": {"child": "@2", "offset": {"x": 35, "y": 35, "z": -2}}},
    {"op": "create", "type": "difference", "params": {"left": "@1", "right": "@3"}, "name": "probe"}]})
ins = call("inspect_cad", {"document_id": pd})
vol_ok = abs(ins.get("volume_mm3", 0) - math.pi*(100-9)*5) < 12
check("A2 apply_edits @refs + root consumption", ins.get("parts") == 1 and vol_ok,
      f"parts={ins.get('parts')} vol={round(ins.get('volume_mm3',0),1)}")
call("close_document", {"document_id": pd})

# A3 sheet-metal cost model alignment (#472)
smc = call("sheet_metal_cost", {"document_id": RI, "quantity": 2})
laser_each = smc["breakdown"]["total_each"]
q = call("quote_manufacturing", {"document_id": RI, "process": "sheet_metal", "quantity": 2,
                                 "material": "mild steel 2.7mm"})
quote_each = q["total_amount_usd"] / 2
ratio = quote_each / laser_each
check("A3 cost models aligned (<=1.35x)", 0.7 <= ratio <= 1.35,
      f"laser ${laser_each:.2f} vs quote ${quote_each:.2f} (x{ratio:.2f})")
RI_QUOTE = q.get("quote_id", "")

# ---------------------------------------------------------------- Phase B
print("== Phase B: full v3 build ==")

# B1 schematic + board + rules
sch = call("create_schematic", {
    "title": "PCB Stator v3 — 9s/6p ferrite-PM axial",
    "components": [{"ref": "J1", "footprint": "JST_XH_4", "value": "B4B-XH-A", "x": 10, "y": 10,
        "pins": [{"number": "1", "name": "PHA", "type": "Passive"},
                 {"number": "2", "name": "PHB", "type": "Passive"},
                 {"number": "3", "name": "PHC", "type": "Passive"},
                 {"number": "4", "name": "WIND_N", "type": "Passive"}]}],
    "nets": {"PHA": ["J1.1"], "PHB": ["J1.2"], "PHC": ["J1.3"], "WIND_N": ["J1.4"]}})
DOC = sch["document_id"]
call("place_components", {"document_id": DOC, "board_shape": {"type": "circle", "outer_diameter": 70, "inner_diameter": 10},
                          "board_thickness": 1.6, "strategy": "radial", "radial_radius": 32, "radial_start_angle_deg": 100})
call("set_stackup", {"document_id": DOC, "copper_oz": 2})
call("set_design_rules", {"document_id": DOC, "clearance": 0.2, "track_width": 0.25, "via_drill": 0.3,
                          "via_diameter": 0.6, "edge_clearance": 0.3, "min_drill": 0.3, "min_annular_ring": 0.13})
call("set_placement", {"document_id": DOC, "placements": [{"ref": "J1", "x": 29.44, "y": 66.51, "rotation": 190}]})

# B2 winding + feed repair (expect same 2 NetIslands; stitch PHB/PHC)
w = call("add_motor_winding", {"document_id": DOC, "slots": 9, "poles": 6, "phases": 3,
    "center": {"x": 35, "y": 35}, "pitch_radius": 22.5, "inner_radius": 2.6, "outer_radius": 7.2,
    "trace_width": 0.25, "clearance": 0.2, "turns_per_coil": 10, "connection": "wye",
    "copper_layer": "FCu", "return_layer": "BCu", "neutral_net": "WIND_N",
    "phase_nets": ["PHA", "PHB", "PHC"]})
dd = w.get("drc_delta", {})
print(f"  winding drc_delta: clean={dd.get('clean')} introduced={dd.get('introduced')}")
if not dd.get("clean"):
    for net, pad, ring_r in (("PHB", (30.671, 66.727), 13.8), ("PHC", (28.209, 66.293), 12.9)):
        ang = 97.8 if net == "PHB" else 102.2
        hop, ring = P(16, ang), P(ring_r, ang)
        call("add_trace", {"document_id": DOC, "net": net, "layer": "BCu", "width": 0.5,
                           "points": [{"x": pad[0], "y": pad[1]}, {"x": round(hop[0],3), "y": round(hop[1],3)}]})
        call("add_via", {"document_id": DOC, "net": net, "position": {"x": round(hop[0],3), "y": round(hop[1],3)},
                         "diameter": 0.6, "drill": 0.3})
        call("add_trace", {"document_id": DOC, "net": net, "layer": "BCu", "width": 0.5,
                           "points": [{"x": round(hop[0],3), "y": round(hop[1],3)}, {"x": round(ring[0],3), "y": round(ring[1],3)}]})
        call("add_via", {"document_id": DOC, "net": net, "position": {"x": round(ring[0],3), "y": round(ring[1],3)},
                         "diameter": 0.6, "drill": 0.3})

# B3 bore-mount outline
call("set_board_outline", {"document_id": DOC, "thickness": 1.6, "outline": {
    "vertices": ngon(35, 35, 35, 64, aslist=False),
    "cutouts": [ngon(35, 35, 5.0, 32, aslist=False)] + [
        ngon(*P(8, a), 1.7, 16, aslist=False) for a in (60, 180, 300)]}})

# B4 gates
drc = call("run_drc", {"document_id": DOC})
check("B4 DRC clean", drc.get("violations") == 0, f"violations={drc.get('violations')} {drc.get('byRule')}")
dfm = call("dfm_check", {"document_id": DOC, "process": "pcb_jlcpcb"})
hard = [r for r in dfm.get("rules", []) if r.get("applicable") and not r.get("passed") and r.get("severity") == "error"]
check("B4 DFM jlcpcb no errors (#473 tie-aware)", not hard,
      "; ".join(f"{r['rule']}x{r.get('violations')}" for r in hard) or "warnings only")
call("export_gerber", {"document_id": DOC, "output_dir": "fab/stator-v3-gerbers"})
qpcb = call("quote_manufacturing", {"document_id": DOC, "process": "pcb", "quantity": 5, "layers": 2})
call("save_document", {"document_id": DOC, "name": "stator-v3"})
rcpt = call("build_receipt", {"document_id": DOC})
print(f"  stator v3: ${qpcb['total_amount_usd']}/5, receipt {rcpt.get('receipt',{}).get('board_hash')}")

# B5 clean assembly via solid_from_board + @N apply_edits
sb = call("solid_from_board", {"document_id": DOC, "include_components": True, "part_name": "stator-pcb"})
AD = sb["document_id"]
ops, marks = [], {}
def op(o, mark=None):
    ops.append(o)
    if mark: marks[mark] = f"@{len(ops)-1}"
    return f"@{len(ops)-1}"
def cyl(r, h): return op({"op": "create", "type": "cylinder", "params": {"radius": r, "height": h}})
def tr(c, x, y, z): return op({"op": "create", "type": "translate", "params": {"child": c, "offset": {"x": x, "y": y, "z": z}}})
def dif(l, r, name=None):
    o = {"op": "create", "type": "difference", "params": {"left": l, "right": r}}
    if name: o["name"] = name
    return op(o)
def uni(l, r, name=None):
    o = {"op": "create", "type": "union", "params": {"left": l, "right": r}}
    if name: o["name"] = name
    return op(o)

m = None
for k in range(6):
    c = tr(cyl(7.5, 3), *P(21.5, 60*k), 2.6)
    m = c if m is None else uni(m, c, "magnets" if k == 5 else None)
dif(tr(cyl(29, 2.7), 35, 35, 5.6), tr(cyl(4.2, 4.7), 35, 35, 4.6), "rotor-iron")
dif(uni(tr(cyl(15, 4), 35, 35, 8.3), tr(cyl(7.5, 8), 35, 35, 12.3)),
    tr(cyl(4.05, 14), 35, 35, 7.8), "hub")
op({"op": "create", "type": "translate", "params": {"child": cyl(4, 34), "offset": {"x": 35, "y": 35, "z": -15}}, "name": "shaft"})
h = None
for a in (60, 180, 300):
    c = tr(cyl(2.85, 1.65), *P(8, a), 1.6)
    h = c if h is None else uni(h, c, "screw-heads" if a == 300 else None)
dif(tr(cyl(35, 2.7), 35, 35, -2.7), tr(cyl(8, 4.7), 35, 35, -3.7), "stator-iron")
dif(dif(uni(tr(cyl(40, 6), 35, 35, -27.7), tr(cyl(12, 19), 35, 35, -21.7)),
        tr(cyl(11, 14.2), 35, 35, -17.1)),
    tr(cyl(5.5, 40), 35, 35, -30), "base")
for nm, z in (("bearing-lo", -16.9), ("bearing-hi", -9.9)):
    dif(tr(cyl(11, 7), 35, 35, z), tr(cyl(4.05, 7.4), 35, 35, z-0.2), nm)

r1 = call("apply_edits", {"document_id": AD, "ops": ops[:40]})
r2 = call("apply_edits", {"document_id": AD, "ops": ops[40:]})
# NOTE: batch 2 @N refs are batch-local; all cross-references stay within their batch
ai = call("inspect_cad", {"document_id": AD})
vol = ai.get("volume_mm3", 0)
check("B5 clean assembly (no ghost roots)", 60000 < vol < 80000 and ai.get("parts", 99) <= 12,
      f"parts={ai.get('parts')} vol={round(vol)}")

# B6 clearance assertions
for label, ga, gb, mn in (
    ("air-gap", ["magnets"], ["stator-pcb"], 0.9),
    ("magnet-vs-heads", ["magnets"], ["screw-heads"], 0.5),
    ("shaft-vs-stator", ["shaft"], ["stator-pcb"], 0.8),
    ("hub-vs-heads", ["hub"], ["screw-heads"], 2.0),
    ("rotor-vs-board", ["rotor-iron"], ["stator-pcb"], 2.0),
    ("shaft-vs-bearing", ["shaft"], ["bearing-lo", "bearing-hi"], 0.02)):
    c = call("check_clearance", {"document_id": AD, "label": label, "group_a": ga, "group_b": gb, "min_mm": mn})
    check(f"B6 {label}", bool(c.get("holds", c.get("pass"))), f"measured={c.get('measured_mm')}mm")
call("save_document", {"document_id": AD, "name": "motor-assembly-v3"})

# B7 stator iron + base + BOM
st_iron = call("sheet_metal_create", {
    "width": 70, "depth": 70, "thickness": 2.7, "material": "steel-mild", "shop_profile": "sendcutsend",
    "outline": ngon(35, 35, 35, 64),
    "holes": [ngon(35, 35, 8.0, 24, cw=True)] + [ngon(*P(8, a), 1.7, 12, cw=True) for a in (60, 180, 300)]})
SI = st_iron["document_id"]
q_si = call("quote_manufacturing", {"document_id": SI, "process": "sheet_metal", "quantity": 2, "material": "mild steel 2.7mm"})
base = call("load_document", {"name": "motor-base"})
q_b = call("quote_manufacturing", {"document_id": base["document_id"], "process": "3dprint", "quantity": 1, "material": "abs"})
rot_pcb = call("load_document", {"name": "rotor-dragcup"})
q_rp = call("quote_manufacturing", {"document_id": rot_pcb["document_id"], "process": "pcb", "quantity": 5, "layers": 2})
call("save_document", {"document_id": RI, "name": "rotor-back-iron-v3"})
call("save_document", {"document_id": SI, "name": "stator-back-iron-v3"})

bom = call("bom_create", {"title": "Rare-earth-free PCB-stator axial-flux motor v3", "document_id": AD, "lines": [
    {"kind": "manufactured", "name": "Stator PCB v3 (9s/6p, 70mm, 2oz)", "qty": 5, "vendor": "jlcpcb",
     "process": "pcb", "quote_id": qpcb["quote_id"], "artifact": "fab/stator-v3-gerbers/"},
    {"kind": "manufactured", "name": "Rotor drag-cup PCB (OPTIONAL demo)", "qty": 5, "vendor": "jlcpcb",
     "process": "pcb", "quote_id": q_rp["quote_id"], "artifact": "fab/rotor-gerbers/",
     "notes": "not needed for PM motor; verified will not self-start"},
    {"kind": "manufactured", "name": "Rotor back-iron D58x2.7, 4x M4 taps BCD22", "qty": 2, "vendor": "SendCutSend",
     "process": "sheet_metal", "quote_id": RI_QUOTE, "artifact": "rotor-back-iron-v3.vcad"},
    {"kind": "manufactured", "name": "Stator back-iron D70x2.7 bore-mount", "qty": 2, "vendor": "SendCutSend",
     "process": "sheet_metal", "quote_id": q_si["quote_id"], "artifact": "stator-back-iron-v3.vcad"},
    {"kind": "manufactured", "name": "Bearing-tower base (3D print)", "qty": 1, "unit_price_usd": 1.0,
     "vendor": "home FDM", "process": "3dprint", "quote_id": q_b["quote_id"], "artifact": "fab/motor-base.stl"},
    {"kind": "cots", "catalog_id": "bearing.608zz", "qty": 2, "notes": "ZZ mandatory: self-start margin 9.25 vs 1.39 (2RS)"},
    {"kind": "cots", "catalog_id": "magnet.ferrite-disc-15x3-y30", "qty": 10, "notes": "6 + 4 spare; alternating N/S; zero rare-earth"},
    {"kind": "cots", "catalog_id": "coupling.flange-8mm", "qty": 1},
    {"kind": "cots", "catalog_id": "shaft.ground-8mm", "qty": 1, "notes": "cut to 40mm"},
    {"kind": "cots", "catalog_id": "screw.m3-bhcs", "qty": 1, "notes": "3x M3x10 bore-mount"},
    {"kind": "cots", "name": "M4x8 button head screws", "spec": "ISO 7380, hub to rotor iron", "example_pn": "ISO 7380 M4x8",
     "qty": 4, "unit_price_usd": 0.15, "vendor": "bolt depot"},
    {"kind": "cots", "name": "Polyimide film disc D70", "spec": "0.13mm insulation", "example_pn": "kapton 70mm",
     "qty": 1, "unit_price_usd": 6.0, "vendor": "Amazon"},
    {"kind": "cots", "name": "Structural epoxy", "spec": "2-part", "example_pn": "3M DP420", "qty": 1,
     "unit_price_usd": 6.0, "vendor": "Amazon"},
    {"kind": "cots", "name": "JST-XH 4-pin harness", "spec": "pre-crimped", "example_pn": "XHP-4", "qty": 1,
     "unit_price_usd": 3.0, "vendor": "Amazon"},
    {"kind": "cots", "name": "BLDC driver", "spec": "sensorless 10A+ 12V", "example_pn": "ST B-G431B-ESC1", "qty": 1,
     "unit_price_usd": 21.0, "vendor": "DigiKey"}],
    "assembly_notes": ["Air gap 1.0mm set by hub position (clearance-verified)",
                       "Bore-mount: 3x M3x10 through PCB+iron into tower bosses; Kapton between iron and B.Cu",
                       "Kt 3.7mNm/A derated; ~7mNm @ 1.5A; drive sensorless BLDC @12V"]})
tot = bom.get("totals", {})
print(f"  BOM {bom.get('bom_id','')[:8]}: manufactured ${tot.get('manufactured_subtotal_usd')} + cots ${tot.get('cots_subtotal_usd')} + ship ${tot.get('shipping_estimate_usd')} = ${tot.get('grand_total_usd')}")
for fmt, ext in (("markdown", "md"), ("csv", "csv")):
    e = call("bom_export", {"bom_id": bom["bom_id"], "format": fmt})
    open(f"fab/BOM-v3.{ext}", "w").write(e["rendered"])
check("B7 BOM within budget", (tot.get("grand_total_usd") or 999) < 300, f"${tot.get('grand_total_usd')}")

print()
print("== LEDGER ==")
print("hard failures:", FAIL if FAIL else "none — ALL GREEN")
sys.exit(1 if FAIL else 0)
