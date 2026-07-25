//! `parse_kicad_pcb(write_kicad_pcb(board))` must preserve every pad's world
//! rectangle, at every rotation in the corpus.
//!
//! KiCad stores a pad's angle absolutely (it includes the footprint's
//! orientation); vcad's IR stores it relative, because every geometry consumer
//! composes `fp.rotation + pad.rotation`. Reader and writer must therefore be
//! exact inverses. They were not: the reader kept the absolute angle, so every
//! rotated footprint had its rotation counted twice.

use vcad_ecad_invariants::{corpus, pad_indices, PadRect, TOL_MM};

#[test]
fn kicad_round_trip_preserves_every_pad_rectangle() {
    for b in corpus() {
        let text = vcad_ecad_symbols::write_kicad_pcb(&b.pcb);
        let back = vcad_ecad_symbols::parse_kicad_pcb(&text)
            .unwrap_or_else(|e| panic!("{}: re-parse failed: {e:?}", b.name));

        assert_eq!(
            back.footprints.len(),
            b.pcb.footprints.len(),
            "{}: footprint count changed across the round trip",
            b.name
        );

        for (i, j) in pad_indices(&b.pcb) {
            let before_fp = &b.pcb.footprints[i];
            let before = &before_fp.pads[j];
            let after_fp = &back.footprints[i];
            let after = after_fp
                .pads
                .iter()
                .find(|p| p.number == before.number)
                .unwrap_or_else(|| {
                    panic!("{}: pad {} lost across the round trip", b.name, before.number)
                });

            let want = PadRect::of(before_fp, before);
            let got = PadRect::of(after_fp, after);
            let dev = want.max_deviation(&got);
            assert!(
                dev <= TOL_MM,
                "{}: pad {} moved {dev:.6}mm across the KiCad round trip\n  before: {want:?}\n  after:  {got:?}",
                b.name,
                before.number
            );
        }
    }
}

/// The fine-pitch case, stated as the consequence rather than the number: a
/// TQFN whose pads clear each other before the round trip must still clear
/// them after. Under the double-count bug they overlapped.
#[test]
fn kicad_round_trip_keeps_fine_pitch_pads_apart() {
    for &rot in &vcad_ecad_invariants::ROTATIONS {
        let pcb = vcad_ecad_invariants::board(vec![vcad_ecad_invariants::fine_pitch_tqfn(
            rot, true,
        )]);
        let back = vcad_ecad_symbols::parse_kicad_pcb(&vcad_ecad_symbols::write_kicad_pcb(&pcb))
            .expect("re-parse");
        let fp = &back.footprints[0];
        let rects: Vec<PadRect> = fp.pads.iter().map(|p| PadRect::of(fp, p)).collect();
        for a in 0..rects.len() {
            for b in (a + 1)..rects.len() {
                assert!(
                    !rects[a].overlaps(&rects[b]),
                    "TQFN at {rot}deg: pads {} and {} overlap after a KiCad round trip",
                    fp.pads[a].number,
                    fp.pads[b].number
                );
            }
        }
    }
}
