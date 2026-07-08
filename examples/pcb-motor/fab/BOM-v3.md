# Rare-earth-free PCB-stator axial-flux motor v3

vcad BOM `9642a606` · generated 2026-07-08 · document `doc_4_QWdkKNRqLIQm`

> All prices are ESTIMATES (Phase-0 quote estimates, catalog price bands, or caller-supplied numbers) — not binding quotes.

## Manufactured Parts

| # | Part | Process | Material | Vendor | Qty | Unit | Total | Source |
|---|------|---------|----------|--------|----:|-----:|------:|--------|
| 1 | Stator PCB v3 (9s/6p, 70mm, 2oz) | pcb | — | jlcpcb | 5 | $9.55 | $47.75 | quote 99c8e3ee · doc doc_3_J6MQfcFmcYnZ · fab/stator-v3-gerbers/ |
| 2 | Rotor drag-cup PCB (OPTIONAL demo) | pcb | — | jlcpcb | 5 | $8.40 | $42.00 | quote 536ee66f · doc doc_7_VNzkQU4r003i · fab/rotor-gerbers/ |
| 3 | Rotor back-iron D58x2.7, 4x M4 taps BCD22 | sheet_metal | mild steel 2.7mm | SendCutSend | 2 | $20.97 | $41.94 | quote 8958172e · doc doc_1_1X8fwdlC67xi · rotor-back-iron-v3.vcad |
| 4 | Stator back-iron D70x2.7 bore-mount | sheet_metal | mild steel 2.7mm | SendCutSend | 2 | $21.06 | $42.12 | quote fbe6a364 · doc doc_5_uxMhtVgPVbab · stator-back-iron-v3.vcad |
| 5 | Bearing-tower base (3D print) | 3dprint | abs | home FDM | 1 | $1.00 | $1.00 | quote 123e9644 · doc doc_6_X4i9Hxj9C4Br · fab/motor-base.stl |

## COTS Parts

| # | Item | Spec | Example PN | Vendor | Qty | Unit (est.) | Total (est.) |
|---|------|------|------------|--------|----:|------------:|-------------:|
| 1 | 608ZZ deep-groove ball bearing | bore 8 mm, od 22 mm, width 7 mm, ZZ (metal shields), low — shielded, light grease | 608ZZ | — | 2 | $0.75 | $1.50 |
| 2 | Ferrite disc magnet 15x3 mm, Y30 | disc, od 15 mm, thickness 3 mm, Y30, Br 370-400 mT, sintered ferrite (ceramic), max_temp_c 250 | Y30 D15x3 | — | 10 | $0.43 | $4.30 |
| 3 | Rigid flange coupling, 8 mm bore | bore 8 mm, flange_dia 30 mm, bcd 22 mm, bolt_holes 4, M4, hub_length 12 mm, set screw, steel or aluminum | 8mm flange coupling | — | 1 | $4.00 | $4.00 |
| 4 | Precision ground shaft, 8 mm | diameter 8 mm, h6, hardened bearing steel, chrome plated, lengths 100/150/200/300/500 mm | 8mm x 300mm linear shaft | — | 1 | $8.00 | $8.00 |
| 5 | M3 button head cap screw (ISO 7380) | M3, button, 2mm hex, 10.9 alloy steel, black oxide, lengths 4/5/6/8/10/12/16/20 mm | ISO 7380 M3x8 | — | 1 | $0.11 | $0.11 |
| 6 | M4x8 button head screws | ISO 7380, hub to rotor iron | ISO 7380 M4x8 | bolt depot | 4 | $0.15 | $0.60 |
| 7 | Polyimide film disc D70 | 0.13mm insulation | kapton 70mm | Amazon | 1 | $6.00 | $6.00 |
| 8 | Structural epoxy | 2-part | 3M DP420 | Amazon | 1 | $6.00 | $6.00 |
| 9 | JST-XH 4-pin harness | pre-crimped | XHP-4 | Amazon | 1 | $3.00 | $3.00 |
| 10 | BLDC driver | sensorless 10A+ 12V | ST B-G431B-ESC1 | DigiKey | 1 | $21.00 | $21.00 |

## Totals

| | |
|---|---:|
| Manufactured subtotal | $174.81 |
| COTS subtotal | $54.51 |
| Shipping estimate | $40.00 |
| **Estimated total** | **$269.32** |

Shipping basis: flat domestic estimate ($8.00) per distinct vendor (5); quote-linked lines excluded — their prices are already landed.

## Assembly Notes

- Air gap 1.0mm set by hub position (clearance-verified)
- Bore-mount: 3x M3x10 through PCB+iron into tower bosses; Kapton between iron and B.Cu
- Kt 3.7mNm/A derated; ~7mNm @ 1.5A; drive sensorless BLDC @12V
