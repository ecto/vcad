//! The material table.
//!
//! Every number here is a *conservative starting value for carbide tooling on
//! a light machine*, not an optimum. Where a figure comes from a published
//! chart it is cited next to the value; where it comes from the one cut this
//! project has actually made, it says so.
//!
//! Sources referred to by short name below:
//!
//! * **Onsrud** — Onsrud Cutter's "Feeds & speeds / chip load" charts for the
//!   52-/54-/63-/65-/190-series router bits (wood, MDF, plywood, plastics,
//!   aluminium). The origin of nearly all hobby-router chipload folklore.
//! * **Harvey** — Harvey Tool's speeds-and-feeds pages for miniature square
//!   end mills, which are the usual source for Ø < 2 mm values.
//! * **MH** — *Machinery's Handbook* cutting-speed tables for carbide
//!   (converted from sfm: 1 sfm = 0.3048 m/min).
//! * **Anchor** — the 2026-09-17 cut recorded in
//!   `docs/native-app-friction-log.md` item 54: 1 mm copper plate, Ø2 mm
//!   2-flute carbide, ≈13 500 rpm, 250 mm/min, 0.17 mm passes, 40 mm/min
//!   plunge. Clean profile, cutter survived. The copper row is calibrated to
//!   land on it.
//!
//! `difficulty_rank` is a coarse ordinal of *how hard this is to machine on a
//! light machine*, which is not the same as hardness — copper outranks 7075
//! because gumming, not strength, is what ends a copper cut on a router. It
//! orders the UI list and the monotonicity checks.

use super::{
    ChipWelding, CoolantNeed, Material, MaterialFamily, MaterialNote, NoteLevel, SpeedRange,
};

fn note(level: NoteLevel, text: &str) -> MaterialNote {
    MaterialNote {
        level,
        text: text.to_string(),
    }
}

fn speed(min: f64, target: f64, max: f64) -> SpeedRange {
    SpeedRange { min, target, max }
}

/// Build the table. Called once, behind a `OnceLock`.
pub(super) fn build() -> Vec<Material> {
    vec![
        // -------------------------------------------------------------- wood
        Material {
            id: super::ids::SOFTWOOD.into(),
            name: "Softwood (pine, fir, cedar)".into(),
            family: MaterialFamily::Wood,
            difficulty_rank: 5,
            // Onsrud 52-series softwood, 1/8": 0.004–0.007"/tooth
            // (0.10–0.18 mm). 0.130 sits mid-band.
            chipload_ref_mm: 0.130,
            // Fibrous material needs a real bite or it burnishes and burns;
            // burning, not wear, is the failure mode in wood.
            min_chipload_mm: 0.020,
            // Onsrud quotes 18 000 rpm for 1/8" in softwood ≈ 180 m/min; the
            // ceiling is where resin starts to burn rather than where the
            // tool wears.
            surface_speed: speed(120.0, 200.0, 450.0),
            slot_depth_frac: 1.2,
            profile_depth_frac: 2.4,
            radial_frac: 0.45,
            plunge_frac: 0.45,
            ramp_angle_deg: 8.0,
            coolant: CoolantNeed::Dry,
            chip_welding: ChipWelding::Negligible,
            finish_allowance_frac: 0.08,
            router_class_ok: true,
            hazards: vec![note(
                NoteLevel::Caution,
                "Wood dust is a fire and inhalation risk: extract at the cutter, and empty the vacuum — fine dust in a bag is a fire load.",
            )],
            guidance: vec![note(
                NoteLevel::Info,
                "Resin builds on the flutes in pine and fir; clean the cutter between jobs or it will start burning.",
            )],
        },
        Material {
            id: super::ids::MDF.into(),
            name: "MDF".into(),
            family: MaterialFamily::Wood,
            difficulty_rank: 8,
            // Onsrud 52-/63-series MDF, 1/8": 0.004–0.006"/tooth
            // (0.10–0.15 mm). 0.110 is the low end of the band: MDF is
            // abrasive and a hobby router is not the machine to push it on.
            chipload_ref_mm: 0.110,
            min_chipload_mm: 0.020,
            // 17 000–18 000 rpm at 1/8" is the usual router figure.
            surface_speed: speed(120.0, 180.0, 400.0),
            slot_depth_frac: 1.0,
            profile_depth_frac: 2.0,
            radial_frac: 0.45,
            plunge_frac: 0.40,
            ramp_angle_deg: 6.0,
            coolant: CoolantNeed::Dry,
            chip_welding: ChipWelding::Negligible,
            finish_allowance_frac: 0.08,
            router_class_ok: true,
            hazards: vec![note(
                NoteLevel::Warning,
                "MDF dust is very fine and carries formaldehyde from the binder: extract at the cutter and wear a fitted P2/N95 mask. Do not blow it off with compressed air — that puts the whole cut into the air you are standing in.",
            )],
            guidance: vec![note(
                NoteLevel::Info,
                "MDF is abrasive: carbide dulls noticeably within a sheet or two, and a dull cutter in MDF burns before it breaks.",
            )],
        },
        Material {
            id: super::ids::PLYWOOD.into(),
            name: "Plywood".into(),
            family: MaterialFamily::Wood,
            difficulty_rank: 10,
            // Onsrud 52-series plywood, 1/8": 0.003–0.005" (0.076–0.127 mm).
            chipload_ref_mm: 0.100,
            min_chipload_mm: 0.020,
            surface_speed: speed(120.0, 180.0, 400.0),
            slot_depth_frac: 0.9,
            profile_depth_frac: 1.8,
            radial_frac: 0.45,
            plunge_frac: 0.40,
            ramp_angle_deg: 5.0,
            coolant: CoolantNeed::Dry,
            chip_welding: ChipWelding::Negligible,
            finish_allowance_frac: 0.08,
            router_class_ok: true,
            hazards: vec![note(
                NoteLevel::Caution,
                "Plywood dust carries adhesive: extract at the cutter and wear a mask. Void-prone cores also throw chips unpredictably.",
            )],
            guidance: vec![note(
                NoteLevel::Info,
                "A compression (up-down) cutter is what stops the top and bottom veneers tearing out; a straight flute will fray one face.",
            )],
        },
        Material {
            id: super::ids::ACRYLIC_CAST.into(),
            name: "Acrylic, cast (PMMA)".into(),
            family: MaterialFamily::Plastic,
            difficulty_rank: 12,
            // Onsrud 63-series (O-flute, acrylic), 1/8": 0.003–0.005"
            // (0.076–0.127 mm). Acrylic wants a *thick* chip to carry heat
            // away — a thin chip melts and re-welds.
            chipload_ref_mm: 0.095,
            min_chipload_mm: 0.015,
            // Deliberately low: acrylic is limited by melting, not by wear,
            // so the fix for a gummy cut is fewer rpm, not more.
            surface_speed: speed(90.0, 130.0, 250.0),
            slot_depth_frac: 0.6,
            profile_depth_frac: 1.2,
            radial_frac: 0.40,
            plunge_frac: 0.25,
            ramp_angle_deg: 3.0,
            coolant: CoolantNeed::AirBlast,
            chip_welding: ChipWelding::High,
            finish_allowance_frac: 0.06,
            router_class_ok: true,
            hazards: vec![note(
                NoteLevel::Caution,
                "Acrylic melts: a stalled or rubbing cutter welds the swarf back into the slot and can grab the sheet. Keep the chip thick, keep the cutter moving, and blow the slot clear.",
            )],
            guidance: vec![note(
                NoteLevel::Info,
                "Single-flute or O-flute, and slow the spindle down before speeding the feed up. Cast acrylic machines far better than extruded.",
            )],
        },
        Material {
            id: super::ids::HDPE.into(),
            name: "HDPE".into(),
            family: MaterialFamily::Plastic,
            difficulty_rank: 14,
            // Onsrud 65-series soft plastics, 1/8": 0.003–0.005".
            chipload_ref_mm: 0.090,
            min_chipload_mm: 0.015,
            surface_speed: speed(100.0, 180.0, 350.0),
            slot_depth_frac: 1.0,
            profile_depth_frac: 2.0,
            radial_frac: 0.40,
            plunge_frac: 0.40,
            ramp_angle_deg: 5.0,
            coolant: CoolantNeed::Dry,
            chip_welding: ChipWelding::Moderate,
            finish_allowance_frac: 0.06,
            router_class_ok: true,
            hazards: vec![],
            guidance: vec![note(
                NoteLevel::Info,
                "HDPE springs back: cut it oversize and it still closes up. Stringy swarf wraps the cutter — clear it rather than letting it build.",
            )],
        },
        Material {
            id: super::ids::HARDWOOD.into(),
            name: "Hardwood (oak, maple, ash)".into(),
            family: MaterialFamily::Wood,
            difficulty_rank: 16,
            // Onsrud 52-series hardwood, 1/8": 0.003–0.005".
            chipload_ref_mm: 0.085,
            min_chipload_mm: 0.020,
            surface_speed: speed(100.0, 170.0, 350.0),
            slot_depth_frac: 0.8,
            profile_depth_frac: 1.6,
            radial_frac: 0.45,
            plunge_frac: 0.35,
            ramp_angle_deg: 5.0,
            coolant: CoolantNeed::Dry,
            chip_welding: ChipWelding::Negligible,
            finish_allowance_frac: 0.08,
            router_class_ok: true,
            hazards: vec![note(
                NoteLevel::Caution,
                "Some hardwood dusts (oak, beech, exotic species) are sensitisers or carcinogens: extract at the cutter and wear a mask.",
            )],
            guidance: vec![note(
                NoteLevel::Info,
                "Climb-mill the face you care about; conventional milling lifts the grain on figured stock.",
            )],
        },
        Material {
            id: super::ids::POM_ACETAL.into(),
            name: "POM / acetal (Delrin)".into(),
            family: MaterialFamily::Plastic,
            difficulty_rank: 18,
            // Onsrud 65-series, 1/8": 0.003–0.004". Acetal is the best
            // behaved plastic on this list — clean chips, no melting.
            chipload_ref_mm: 0.080,
            min_chipload_mm: 0.015,
            surface_speed: speed(100.0, 160.0, 300.0),
            slot_depth_frac: 0.8,
            profile_depth_frac: 1.6,
            radial_frac: 0.40,
            plunge_frac: 0.35,
            ramp_angle_deg: 4.0,
            coolant: CoolantNeed::Dry,
            chip_welding: ChipWelding::Low,
            finish_allowance_frac: 0.06,
            router_class_ok: true,
            hazards: vec![],
            guidance: vec![note(
                NoteLevel::Info,
                "Acetal cuts cleanly dry and holds size well. It is the plastic to prototype in.",
            )],
        },
        Material {
            id: super::ids::POLYCARBONATE.into(),
            name: "Polycarbonate".into(),
            family: MaterialFamily::Plastic,
            difficulty_rank: 20,
            // Onsrud 65-series, 1/8": 0.002–0.004". Lower than acrylic:
            // polycarbonate is tough rather than brittle, so it smears.
            chipload_ref_mm: 0.075,
            min_chipload_mm: 0.015,
            surface_speed: speed(90.0, 120.0, 250.0),
            slot_depth_frac: 0.6,
            profile_depth_frac: 1.2,
            radial_frac: 0.40,
            plunge_frac: 0.25,
            ramp_angle_deg: 3.0,
            coolant: CoolantNeed::AirBlast,
            chip_welding: ChipWelding::High,
            finish_allowance_frac: 0.06,
            router_class_ok: true,
            hazards: vec![note(
                NoteLevel::Caution,
                "Polycarbonate melts and smears rather than chipping: a rubbing cutter welds a bead into the slot that then grabs. Keep the spindle slow and the feed up, and blow the slot clear.",
            )],
            guidance: vec![note(
                NoteLevel::Info,
                "Single-flute, sharp, uncoated. Leave the protective film on — it keeps the swarf off the surface.",
            )],
        },
        // ------------------------------------------------------- composites
        Material {
            id: super::ids::CARBON_FIBRE_SHEET.into(),
            name: "Carbon-fibre sheet (cured)".into(),
            family: MaterialFamily::Composite,
            difficulty_rank: 26,
            // No consensus chart; diamond-coat and burr-style cutter vendors
            // quote roughly half the plastics figure for straight-flute
            // carbide at 1/8". Abrasion, not force, sets the limit.
            chipload_ref_mm: 0.060,
            min_chipload_mm: 0.010,
            surface_speed: speed(80.0, 120.0, 200.0),
            slot_depth_frac: 0.4,
            profile_depth_frac: 0.8,
            radial_frac: 0.25,
            plunge_frac: 0.20,
            ramp_angle_deg: 2.0,
            coolant: CoolantNeed::Dry,
            chip_welding: ChipWelding::Negligible,
            finish_allowance_frac: 0.05,
            router_class_ok: true,
            hazards: vec![
                note(
                    NoteLevel::Danger,
                    "Carbon-fibre dust is a respiratory hazard and is electrically conductive: extract at the cutter, wear a fitted P3/N95 mask and eye protection, and never blow the dust off — it gets into motors, bearings and electronics and shorts them.",
                ),
                note(
                    NoteLevel::Warning,
                    "Wet-vacuum or damp-wipe the machine afterwards. A shop vacuum without a HEPA filter puts the finest fraction straight back into the room.",
                ),
            ],
            guidance: vec![note(
                NoteLevel::Warning,
                "Carbon fibre destroys plain carbide in minutes. Use a diamond-coated or burr-style cutter and treat any straight-flute carbide as a one-job consumable.",
            )],
        },
        Material {
            id: super::ids::FR4_COPPER_CLAD.into(),
            name: "FR4 / copper-clad laminate".into(),
            family: MaterialFamily::Composite,
            difficulty_rank: 28,
            // PCB routing practice: 1/8" fishtail/diamond-cut router in FR4
            // at 0.002"/tooth (0.05 mm) is a common published figure; 0.055
            // is that with the glass abrasion in mind.
            chipload_ref_mm: 0.055,
            min_chipload_mm: 0.010,
            surface_speed: speed(100.0, 150.0, 250.0),
            slot_depth_frac: 0.5,
            profile_depth_frac: 1.0,
            radial_frac: 0.30,
            plunge_frac: 0.25,
            ramp_angle_deg: 2.0,
            coolant: CoolantNeed::Dry,
            chip_welding: ChipWelding::Negligible,
            finish_allowance_frac: 0.05,
            router_class_ok: true,
            hazards: vec![
                note(
                    NoteLevel::Danger,
                    "FR4 dust is glass fibre and epoxy: a respiratory and eye hazard. Extract at the cutter, wear a fitted P3/N95 mask and eye protection, and never blow it clear with compressed air.",
                ),
                note(
                    NoteLevel::Warning,
                    "Cut FR4 dry. Water turns the dust into a slurry that gets everywhere and does the laminate no good.",
                ),
            ],
            guidance: vec![note(
                NoteLevel::Warning,
                "Glass fibre is abrasive: a plain carbide end mill is dull after a board or two. Fishtail or diamond-cut router bits last far longer.",
            )],
        },
        // ------------------------------------------------------------ metals
        Material {
            id: super::ids::BRASS_C360.into(),
            name: "Brass C360 (free-machining)".into(),
            family: MaterialFamily::CopperAlloy,
            difficulty_rank: 32,
            // The easiest metal on this list: C360 is the 100 % machinability
            // reference. Published 1/8" carbide chiploads are 0.002–0.003"
            // (0.05–0.076 mm); 0.052 is the low end.
            chipload_ref_mm: 0.052,
            min_chipload_mm: 0.004,
            // MH: carbide in free-machining brass, 300–800 sfm
            // (91–244 m/min). Target 150 keeps a router in its usable band.
            surface_speed: speed(90.0, 150.0, 250.0),
            slot_depth_frac: 0.35,
            profile_depth_frac: 1.0,
            radial_frac: 0.35,
            plunge_frac: 0.30,
            ramp_angle_deg: 3.0,
            coolant: CoolantNeed::Dry,
            chip_welding: ChipWelding::Negligible,
            finish_allowance_frac: 0.05,
            router_class_ok: true,
            hazards: vec![note(
                NoteLevel::Caution,
                "C360 chips come off short, sharp and fast. Eye protection, and do not sweep them up by hand.",
            )],
            guidance: vec![note(
                NoteLevel::Info,
                "Brass cuts dry and cleanly. It grabs a positive-rake cutter, so a sharp tool with a light chipload can self-feed — take it steady on a light machine.",
            )],
        },
        Material {
            id: super::ids::ALUMINIUM_6061_T6.into(),
            name: "Aluminium 6061-T6".into(),
            family: MaterialFamily::Aluminium,
            difficulty_rank: 36,
            // Published carbide chipload at 1/8" in 6061 is 0.0015–0.003"
            // (0.038–0.076 mm). 0.050 is mid-band; the hobby derating (×0.60)
            // brings it to 0.030 mm/tooth ≈ 0.0012", which is exactly the
            // 0.001–0.002"-at-1/8" band that hobby-router guidance quotes.
            chipload_ref_mm: 0.050,
            // Sharp carbide has an edge hone of a few microns; below about
            // 4 µm per tooth the edge ploughs rather than shears.
            min_chipload_mm: 0.004,
            // MH: carbide in 6061, 300–1000 sfm (91–305 m/min). Target 180
            // lands 1/8" at ≈18 000 rpm, which is where routers live.
            surface_speed: speed(120.0, 180.0, 350.0),
            // ~0.1 × D slotting depth for small cutters on a hobby router is
            // the standard guidance; 0.25 × the hobby depth derating (0.50)
            // reproduces 0.125 × D.
            slot_depth_frac: 0.25,
            profile_depth_frac: 0.75,
            radial_frac: 0.30,
            plunge_frac: 0.25,
            ramp_angle_deg: 2.0,
            coolant: CoolantNeed::Lubricant,
            chip_welding: ChipWelding::High,
            finish_allowance_frac: 0.05,
            router_class_ok: true,
            hazards: vec![note(
                NoteLevel::Caution,
                "Aluminium swarf is sharp and gets everywhere; fine aluminium dust is flammable. Eye protection, and vacuum rather than blow.",
            )],
            guidance: vec![note(
                NoteLevel::Warning,
                "Aluminium welds itself to a dry cutter and then snaps it. Use a lubricant (paste, WD-40, kerosene) and clear the chips — re-cutting chips is what actually breaks the tool.",
            )],
        },
        Material {
            id: super::ids::ALUMINIUM_7075_T6.into(),
            name: "Aluminium 7075-T6".into(),
            family: MaterialFamily::Aluminium,
            difficulty_rank: 40,
            // Harder and stronger than 6061; published chiploads run ~15 %
            // below it, and it is less prone to gumming.
            chipload_ref_mm: 0.043,
            min_chipload_mm: 0.004,
            surface_speed: speed(120.0, 165.0, 300.0),
            slot_depth_frac: 0.22,
            profile_depth_frac: 0.66,
            radial_frac: 0.28,
            plunge_frac: 0.22,
            ramp_angle_deg: 2.0,
            coolant: CoolantNeed::Lubricant,
            chip_welding: ChipWelding::Moderate,
            finish_allowance_frac: 0.05,
            router_class_ok: true,
            hazards: vec![note(
                NoteLevel::Caution,
                "Aluminium swarf is sharp and gets everywhere; fine aluminium dust is flammable. Eye protection, and vacuum rather than blow.",
            )],
            guidance: vec![note(
                NoteLevel::Info,
                "7075 chips better than 6061 and gums less, but it is stiffer: the cut is louder and the machine will tell you sooner when the depth is too much.",
            )],
        },
        Material {
            id: super::ids::COPPER_C110.into(),
            name: "Copper C110 (ETP)".into(),
            family: MaterialFamily::CopperAlloy,
            difficulty_rank: 46,
            // Calibrated on the Anchor cut: Ø2 mm 2-flute at 13 500 rpm and
            // 250 mm/min is 0.0093 mm/tooth. This table value (0.028 at
            // 1/8") scales to 0.0185 at Ø2, and the hobby derating (×0.60)
            // gives 0.0111 mm/tooth → 300 mm/min, i.e. 1.2× the cut that
            // worked. Roughly half the aluminium figure, which matches the
            // reduced chiploads that copper entries in small-carbide charts
            // (Datron, PreciseBits) carry: pure copper smears rather than
            // shearing, and on a light machine that is what ends the cut.
            chipload_ref_mm: 0.028,
            min_chipload_mm: 0.005,
            // MH: carbide in copper, 200–400 sfm (61–122 m/min). Target 85
            // puts Ø2 mm at 13 528 rpm — dial 2 on the router, which is what
            // the Anchor cut actually ran.
            surface_speed: speed(60.0, 85.0, 120.0),
            // 0.18 × the hobby depth derating (0.50) = 0.09 × D, i.e.
            // 0.18 mm at Ø2 against the Anchor's 0.17 mm passes.
            slot_depth_frac: 0.18,
            profile_depth_frac: 0.50,
            radial_frac: 0.25,
            // The Anchor ran 40 mm/min plunge against a 250 mm/min feed.
            plunge_frac: 0.16,
            ramp_angle_deg: 1.5,
            coolant: CoolantNeed::Lubricant,
            chip_welding: ChipWelding::High,
            finish_allowance_frac: 0.05,
            router_class_ok: true,
            hazards: vec![note(
                NoteLevel::Caution,
                "Copper swarf is stringy and sharp, and copper dust discolours everything it lands on. Eye protection, and vacuum rather than blow.",
            )],
            guidance: vec![note(
                NoteLevel::Warning,
                "Copper is the worst material on this list for welding to a dry cutter: it smears onto the flutes, the smear then cuts nothing, and the tool snaps. Lubricant is not optional, and a sharp uncoated cutter beats a coated one.",
            )],
        },
        Material {
            id: super::ids::MILD_STEEL.into(),
            name: "Mild steel (low carbon)".into(),
            family: MaterialFamily::Steel,
            difficulty_rank: 60,
            // Published 1/8" carbide chipload in mild steel is
            // 0.0008–0.0015" (0.020–0.038 mm).
            chipload_ref_mm: 0.025,
            min_chipload_mm: 0.005,
            // MH: carbide in low-carbon steel, 200–400 sfm (61–122 m/min).
            surface_speed: speed(60.0, 80.0, 120.0),
            slot_depth_frac: 0.15,
            profile_depth_frac: 0.45,
            radial_frac: 0.20,
            plunge_frac: 0.20,
            ramp_angle_deg: 1.5,
            coolant: CoolantNeed::Mist,
            chip_welding: ChipWelding::Low,
            finish_allowance_frac: 0.05,
            router_class_ok: false,
            hazards: vec![note(
                NoteLevel::Warning,
                "Steel chips come off hot and sharp, and a dry cut can throw sparks. Eye protection, and keep the swarf away from anything flammable.",
            )],
            guidance: vec![note(
                NoteLevel::Warning,
                "A router-class machine has neither the rigidity nor the low-speed torque for steel. These numbers are for a benchtop mill; on a router treat any steel as an emergency, take a fraction of the depth, and expect chatter.",
            )],
        },
        Material {
            id: super::ids::STAINLESS_304.into(),
            name: "Stainless 304".into(),
            family: MaterialFamily::Stainless,
            difficulty_rank: 80,
            // Published 1/8" carbide chipload in 304 is 0.0005–0.001"
            // (0.013–0.025 mm); 304 work-hardens, so the *minimum* matters
            // more than the maximum — a light cut is what ruins the part.
            chipload_ref_mm: 0.018,
            min_chipload_mm: 0.006,
            // MH: carbide in austenitic stainless, 100–250 sfm
            // (30–76 m/min).
            surface_speed: speed(35.0, 50.0, 80.0),
            slot_depth_frac: 0.10,
            profile_depth_frac: 0.30,
            radial_frac: 0.15,
            plunge_frac: 0.15,
            ramp_angle_deg: 1.0,
            coolant: CoolantNeed::Flood,
            chip_welding: ChipWelding::Moderate,
            finish_allowance_frac: 0.05,
            router_class_ok: false,
            hazards: vec![note(
                NoteLevel::Warning,
                "Stainless chips are hot, sharp and spring-loaded. Eye protection, and flood coolant if the machine has it.",
            )],
            guidance: vec![note(
                NoteLevel::Danger,
                "304 work-hardens the moment the cutter stops cutting: one rubbing pass leaves a glazed skin the next pass cannot get under, and the tool dies in that skin. It is not a router material — do not attempt it on a gantry machine with a trim router.",
            )],
        },
    ]
}
