//! Tests for the material table and the recommender.
//!
//! House rule 1 applies: these assert numbers, not existence. Where a test is
//! the only thing standing between a derating and silence, it is called out in
//! its own doc comment as a mutation-check target.

use super::*;

/// The diameters every sweep runs over: the ends of the table, the two sizes
/// the machine actually owns, and 1/8".
const GRID: [f64; 6] = [0.5, 1.0, 2.0, 3.175, 6.0, 12.0];

const OPS: [OpKind; 5] = [
    OpKind::Slot,
    OpKind::Profile,
    OpKind::Pocket,
    OpKind::Drill,
    OpKind::Finish,
];

fn tool(d: f64) -> ToolSpec {
    ToolSpec::new(d, 2, ToolKind::FlatEndMill, 10.0)
}

fn rel_err(a: f64, b: f64) -> f64 {
    if b == 0.0 {
        a.abs()
    } else {
        (a - b).abs() / b.abs()
    }
}

// ---------------------------------------------------------------------------
// The arithmetic is the arithmetic
// ---------------------------------------------------------------------------

/// `feed = chipload × flutes × rpm` must hold on every branch, including the
/// ones that clamped the feed or dropped the spindle speed — otherwise the
/// arithmetic printed in the notes is a story about a different number.
#[test]
fn feed_is_exactly_chipload_times_flutes_times_rpm() {
    let machines = [
        Machine::anolex_ultra2(),
        Machine::benchtop_mill(),
        Machine::rigid_vmc(),
    ];
    let spindles = [Spindle::router_dial(), Spindle::vfd_spindle()];
    for m in materials() {
        for &d in &GRID {
            for &op in &OPS {
                for flutes in [1u8, 2, 3, 4] {
                    for machine in &machines {
                        for spindle in &spindles {
                            let t = ToolSpec::new(d, flutes, ToolKind::FlatEndMill, 10.0);
                            let r = recommend(m, &t, op, machine, spindle).unwrap();
                            let rebuilt = r.chipload_mm * f64::from(r.flutes) * r.rpm;
                            assert!(
                                rel_err(rebuilt, r.feed_mm_min) < 1e-9,
                                "{} Ø{d} {:?} {}fl: {} × {} × {} = {rebuilt}, but feed = {}",
                                m.id,
                                op,
                                flutes,
                                r.chipload_mm,
                                r.flutes,
                                r.rpm,
                                r.feed_mm_min
                            );
                        }
                    }
                }
            }
        }
    }
}

/// The machine class is the single biggest lever in this module, so it gets a
/// test that fails if either derating is silently dropped.
///
/// **Mutation-check target.** Break `MachineClass::chipload_factor` or
/// `depth_factor` and this is what catches it.
#[test]
fn machine_class_derates_chipload_and_depth_by_exactly_the_published_factors() {
    let al = material(ids::ALUMINIUM_6061_T6).unwrap();
    // Ø6 keeps every cap and floor out of the way, so what is left is the
    // derating and nothing else. Same motion limits on both machines so the
    // only difference is the class.
    let t = tool(6.0);
    let spindle = Spindle::Controlled {
        min_rpm: 12_000.0,
        max_rpm: 12_000.0,
    };
    let mk = |class| Machine {
        name: "fixture".into(),
        class,
        max_feed_mm_min: 4000.0,
        max_plunge_mm_min: 800.0,
    };

    let rigid = recommend(al, &t, OpKind::Slot, &mk(MachineClass::Rigid), &spindle).unwrap();
    let bench = recommend(al, &t, OpKind::Slot, &mk(MachineClass::Benchtop), &spindle).unwrap();
    let hobby = recommend(al, &t, OpKind::Slot, &mk(MachineClass::Hobby), &spindle).unwrap();

    // The table value at Ø6 is the rigid value.
    assert!(rel_err(rigid.chipload_mm, al.chipload_at(6.0)) < 1e-12);

    assert!(
        rel_err(hobby.chipload_mm / rigid.chipload_mm, 0.60) < 1e-12,
        "hobby chipload derating: {} / {}",
        hobby.chipload_mm,
        rigid.chipload_mm
    );
    assert!(rel_err(bench.chipload_mm / rigid.chipload_mm, 0.85) < 1e-12);

    assert!(
        rel_err(hobby.stepdown_mm / rigid.stepdown_mm, 0.50) < 1e-12,
        "hobby depth derating: {} / {}",
        hobby.stepdown_mm,
        rigid.stepdown_mm
    );
    assert!(rel_err(bench.stepdown_mm / rigid.stepdown_mm, 0.80) < 1e-12);

    // And the absolute numbers, so a change to the table shows up here too.
    assert!(rel_err(rigid.stepdown_mm, 0.25 * 6.0) < 1e-12);
    assert!(rel_err(hobby.stepdown_mm, 0.75) < 1e-12);
}

/// Chip thinning is the one place the recommender feeds *faster* than the
/// table, so the factor is pinned to its closed form.
#[test]
fn chip_thinning_matches_its_closed_form_and_is_capped() {
    // Full slot: no thinning.
    assert!(rel_err(chip_thinning_factor(6.0, 6.0), 1.0) < 1e-12);
    // Half diameter: the transition, still 1.0.
    assert!(rel_err(chip_thinning_factor(3.0, 6.0), 1.0) < 1e-12);
    // Quarter diameter: 1 / sqrt(1 - (1 - 0.5)^2) = 1 / sqrt(0.75).
    assert!(rel_err(chip_thinning_factor(1.5, 6.0), 1.0 / 0.75_f64.sqrt()) < 1e-12);
    // Vanishing engagement is capped, not infinite.
    assert!(rel_err(chip_thinning_factor(1e-6, 6.0), MAX_CHIP_THINNING) < 1e-12);
    assert!(rel_err(chip_thinning_factor(0.0, 6.0), 1.0) < 1e-12);
}

// ---------------------------------------------------------------------------
// Monotonicity
// ---------------------------------------------------------------------------

/// A bigger cutter must not end up with a *lower* feed than a smaller one in
/// the same material.
///
/// The spindle is pinned to one speed on purpose. At a constant surface speed
/// the rpm falls as 1/D, so feed ≈ chipload/D × constant and is genuinely not
/// monotone in D — that is physics, not a bug (published charts show the same:
/// a 1/4" cutter at the same surface speed feeds a little slower than a 1/8"
/// one). What must be monotone is the quantity the machine class acts on,
/// which is the chipload, and the feed at a fixed rpm.
#[test]
fn bigger_tool_never_feeds_slower_at_a_fixed_spindle_speed() {
    let machine = Machine::anolex_ultra2();
    let spindle = Spindle::Controlled {
        min_rpm: 17_000.0,
        max_rpm: 17_000.0,
    };
    for m in materials() {
        let mut prev: Option<(f64, f64)> = None;
        for &d in &GRID {
            let r = recommend(m, &tool(d), OpKind::Slot, &machine, &spindle).unwrap();
            if let Some((pd, pf)) = prev {
                assert!(
                    r.feed_mm_min >= pf - 1e-9,
                    "{}: Ø{d} feeds {} mm/min, slower than Ø{pd} at {pf}",
                    m.id,
                    r.feed_mm_min
                );
            }
            prev = Some((d, r.feed_mm_min));
        }
    }
}

/// The table itself must be monotone in diameter.
#[test]
fn bigger_tool_never_takes_a_smaller_chip() {
    for m in materials() {
        let mut prev = 0.0;
        for &d in &GRID {
            let cl = m.chipload_at(d);
            assert!(
                cl > prev,
                "{}: chipload at Ø{d} is {cl}, not above {prev}",
                m.id
            );
            prev = cl;
        }
    }
}

/// Harder-to-machine materials must not be handed a bigger chip.
///
/// Asserted both on the raw table and on what [`recommend`] actually returns
/// at 1/8", where no cap or floor is binding.
#[test]
fn harder_material_never_takes_a_bigger_chip() {
    let sorted = materials();
    assert!(
        sorted
            .windows(2)
            .all(|w| w[0].difficulty_rank < w[1].difficulty_rank),
        "difficulty ranks must be unique and ascending"
    );

    for &d in &GRID {
        for w in sorted.windows(2) {
            assert!(
                w[0].chipload_at(d) >= w[1].chipload_at(d) - 1e-12,
                "at Ø{d}, {} (rank {}) takes {} but the harder {} (rank {}) takes {}",
                w[0].id,
                w[0].difficulty_rank,
                w[0].chipload_at(d),
                w[1].id,
                w[1].difficulty_rank,
                w[1].chipload_at(d)
            );
        }
    }

    let machine = Machine::anolex_ultra2();
    let spindle = Spindle::vfd_spindle();
    let t = tool(CHIPLOAD_REF_DIAMETER_MM);
    let mut prev = f64::INFINITY;
    for m in sorted {
        let r = recommend(m, &t, OpKind::Slot, &machine, &spindle).unwrap();
        assert!(
            r.chipload_mm <= prev + 1e-12,
            "{} (rank {}) is recommended {} mm/tooth, above the softer material's {prev}",
            m.id,
            m.difficulty_rank,
            r.chipload_mm
        );
        prev = r.chipload_mm;
    }
}

// ---------------------------------------------------------------------------
// The whole grid stays inside the machine
// ---------------------------------------------------------------------------

/// Every combination must produce finite, positive numbers that the machine
/// can actually execute. This is the test that stops a table edit from
/// shipping a NaN feed or a stepdown past the end of the flutes.
#[test]
fn every_material_diameter_and_op_is_finite_positive_and_inside_the_machine() {
    for machine in [
        Machine::anolex_ultra2(),
        Machine::benchtop_mill(),
        Machine::rigid_vmc(),
    ] {
        for spindle in [Spindle::router_dial(), Spindle::vfd_spindle()] {
            for m in materials() {
                for &d in &GRID {
                    for &op in &OPS {
                        let t = tool(d);
                        let r = recommend(m, &t, op, &machine, &spindle).unwrap();
                        let ctx = format!("{} Ø{d} {op:?} on {}", m.id, machine.name);

                        for (label, v) in [
                            ("rpm", r.rpm),
                            ("chipload", r.chipload_mm),
                            ("feed", r.feed_mm_min),
                            ("plunge", r.plunge_mm_min),
                            ("stepdown", r.stepdown_mm),
                            ("ramp", r.ramp_angle_deg),
                            ("surface speed", r.surface_speed_m_min),
                        ] {
                            assert!(v.is_finite() && v > 0.0, "{ctx}: {label} = {v}");
                        }
                        assert!(r.stepover_mm.is_finite() && r.stepover_mm >= 0.0, "{ctx}");
                        assert!(
                            r.finish_allowance_mm.is_finite() && r.finish_allowance_mm >= 0.0,
                            "{ctx}"
                        );

                        let ceiling = if op == OpKind::Drill {
                            machine.max_plunge_mm_min
                        } else {
                            machine.max_feed_mm_min
                        };
                        assert!(
                            r.feed_mm_min <= ceiling + 1e-9,
                            "{ctx}: feed {} above the {ceiling} ceiling",
                            r.feed_mm_min
                        );
                        assert!(
                            r.plunge_mm_min <= machine.max_plunge_mm_min + 1e-9,
                            "{ctx}: plunge {} above the Z ceiling",
                            r.plunge_mm_min
                        );
                        assert!(
                            r.stepdown_mm <= t.flute_length_mm + 1e-9,
                            "{ctx}: stepdown {} past the flute length",
                            r.stepdown_mm
                        );
                        assert!(
                            r.stepover_mm <= d + 1e-9,
                            "{ctx}: stepover {} wider than the cutter",
                            r.stepover_mm
                        );
                        assert!(r.rpm >= spindle.min_rpm().unwrap() - 1e-9, "{ctx}");
                        assert!(r.rpm <= spindle.max_rpm().unwrap() + 1e-9, "{ctx}");
                        // A drill has no radial engagement and a finish pass
                        // has nothing left to leave.
                        if op == OpKind::Drill {
                            assert_eq!(r.stepover_mm, 0.0, "{ctx}");
                            assert_eq!(r.finish_allowance_mm, 0.0, "{ctx}");
                        }
                        if op == OpKind::Finish {
                            assert_eq!(r.finish_allowance_mm, 0.0, "{ctx}");
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The anchor: a cut that actually happened
// ---------------------------------------------------------------------------

/// 2026-09-17, friction-log item 54: 1 mm copper plate, Ø2 mm 2-flute carbide,
/// ≈13 500 rpm, 250 mm/min, 0.17 mm passes, 40 mm/min plunge. Clean profile,
/// cutter survived. Cutting a part out of 1 mm sheet buries the cutter, so the
/// operation is a slot, not a profile.
///
/// The recommender has to land in that neighbourhood. The band below is
/// deliberately one-sided-tight on the fast end: being 20 % conservative is
/// fine, being 3× hot is the failure this module exists to prevent.
#[test]
fn copper_anchor_case_reproduces_the_cut_that_worked() {
    let copper = material(ids::COPPER_C110).unwrap();
    let t = ToolSpec::new(2.0, 2, ToolKind::FlatEndMill, 6.0);
    let r = recommend(
        copper,
        &t,
        OpKind::Slot,
        &Machine::anolex_ultra2(),
        &Spindle::router_dial(),
    )
    .unwrap();

    // Dial 2 is what was actually set on the router that day.
    assert_eq!(r.dial.as_deref(), Some("2"), "expected dial 2");
    assert!(rel_err(r.rpm, 13_500.0) < 1e-12);

    // Stated band: 250 mm/min was run; 180–350 is "the same neighbourhood".
    assert!(
        (180.0..=350.0).contains(&r.feed_mm_min),
        "feed {} mm/min is outside the 180–350 band around the 250 mm/min that worked",
        r.feed_mm_min
    );
    // 0.17 mm passes were run; 0.10–0.30 is the band.
    assert!(
        (0.10..=0.30).contains(&r.stepdown_mm),
        "stepdown {} mm is outside the 0.10–0.30 band around the 0.17 mm that worked",
        r.stepdown_mm
    );
    // 40 mm/min plunge was run.
    assert!(
        (25.0..=75.0).contains(&r.plunge_mm_min),
        "plunge {} mm/min is outside the 25–75 band around the 40 mm/min that worked",
        r.plunge_mm_min
    );
    // Never more than 1.5× the feed that survived.
    assert!(
        r.feed_mm_min <= 1.5 * 250.0,
        "feed {} is more than 1.5x the anchor",
        r.feed_mm_min
    );

    // And the copper hazard/lubricant advice has to be on it.
    assert!(
        r.notes.iter().any(|n| n.text.contains("welds")),
        "copper must warn about welding to the cutter"
    );
    assert_eq!(r.coolant, CoolantNeed::Lubricant);
}

/// The MDF cut from the same machine, earlier: Ø3.175 at 10 000 rpm. The
/// recommender picks a faster dial (wood likes rpm), which is fine — what must
/// hold is that the surface speed stays inside the material's band and the
/// feed stays inside the machine.
#[test]
fn mdf_case_stays_inside_the_material_and_machine_bands() {
    let mdf = material(ids::MDF).unwrap();
    let machine = Machine::anolex_ultra2();
    let r = recommend(
        mdf,
        &tool(3.175),
        OpKind::Slot,
        &machine,
        &Spindle::router_dial(),
    )
    .unwrap();
    assert!(
        r.surface_speed_m_min >= mdf.surface_speed.min
            && r.surface_speed_m_min <= mdf.surface_speed.max,
        "surface speed {} outside {:?}",
        r.surface_speed_m_min,
        mdf.surface_speed
    );
    assert!(r.feed_mm_min <= machine.max_feed_mm_min);
    // D/2 is the standard stepdown guidance in wood on a hobby machine.
    assert!(
        rel_err(r.stepdown_mm, 3.175 / 2.0) < 1e-9,
        "{}",
        r.stepdown_mm
    );
}

// ---------------------------------------------------------------------------
// Spindle
// ---------------------------------------------------------------------------

/// The dial picks the nearest position, ties go to the slower one, and the rpm
/// the recommendation reports is the rpm its feed was built from — not the
/// ideal rpm it wanted.
///
/// **Mutation-check target.** Break the nearest-entry search (take the first
/// entry, or the largest) and this fails.
#[test]
fn dial_picks_the_nearest_position_and_the_feed_uses_it() {
    let dial = Spindle::router_dial();

    // Nearest, in both directions.
    assert_eq!(dial.resolve(13_000.0).rpm, 13_500.0);
    assert_eq!(dial.resolve(13_000.0).dial.as_deref(), Some("2"));
    assert_eq!(dial.resolve(18_000.0).rpm, 17_000.0);
    assert_eq!(dial.resolve(20_000.0).rpm, 21_000.0);
    // Exactly halfway between 10 000 and 13 500: the slower one wins.
    assert_eq!(dial.resolve(11_750.0).rpm, 10_000.0);
    // Outside the dial's range, clamped to its ends.
    let fast = dial.resolve(90_000.0);
    assert_eq!(fast.rpm, 30_000.0);
    assert!(fast.clamped_high && !fast.clamped_low);
    let slow = dial.resolve(500.0);
    assert_eq!(slow.rpm, 10_000.0);
    assert!(slow.clamped_low && !slow.clamped_high);

    // resolve_at_most never rounds up.
    assert_eq!(dial.resolve_at_most(20_999.0).rpm, 17_000.0);
    assert_eq!(dial.resolve_at_most(21_000.0).rpm, 21_000.0);
    assert_eq!(dial.resolve_at_most(9_000.0).rpm, 10_000.0);

    // The reported rpm is a real dial position and is the one in the feed.
    let dial_rpms: Vec<f64> = match &dial {
        Spindle::Dial { settings } => settings.iter().map(|s| s.rpm).collect(),
        _ => unreachable!(),
    };
    for m in materials() {
        for &d in &GRID {
            let r = recommend(m, &tool(d), OpKind::Slot, &Machine::anolex_ultra2(), &dial).unwrap();
            assert!(
                dial_rpms.contains(&r.rpm),
                "{} Ø{d}: {} rpm is not a dial position",
                m.id,
                r.rpm
            );
            assert!(rel_err(r.chipload_mm * 2.0 * r.rpm, r.feed_mm_min) < 1e-9);
            assert!(r.dial.is_some(), "a dial spindle must name the position");
            assert!(
                r.notes
                    .iter()
                    .any(|n| n.text.contains("S word") && n.level >= NoteLevel::Warning),
                "{} Ø{d}: must say the S word does nothing",
                m.id
            );
        }
    }

    // A controlled spindle names no dial and says nothing about the S word.
    let r = recommend(
        material(ids::ALUMINIUM_6061_T6).unwrap(),
        &tool(3.175),
        OpKind::Slot,
        &Machine::benchtop_mill(),
        &Spindle::vfd_spindle(),
    )
    .unwrap();
    assert!(r.dial.is_none());
    assert!(!r.notes.iter().any(|n| n.text.contains("S word")));
}

/// When the machine's feed ceiling pins the feed low enough that the chipload
/// falls under the rubbing floor, the recommender drops the *spindle speed*
/// rather than handing back a feed that burnishes.
#[test]
fn a_feed_ceiling_that_would_cause_rubbing_lowers_the_rpm_instead() {
    let mdf = material(ids::MDF).unwrap();
    let machine = Machine {
        name: "slow fixture".into(),
        class: MachineClass::Hobby,
        max_feed_mm_min: 1000.0,
        max_plunge_mm_min: 800.0,
    };
    let spindle = Spindle::Controlled {
        min_rpm: 6_000.0,
        max_rpm: 24_000.0,
    };
    let t = ToolSpec::new(3.175, 4, ToolKind::FlatEndMill, 10.0);
    let r = recommend(mdf, &t, OpKind::Slot, &machine, &spindle).unwrap();

    assert!(rel_err(r.feed_mm_min, 1000.0) < 1e-9);
    // 1000 / (4 × rpm) must land exactly on the floor.
    assert!(
        rel_err(r.chipload_mm, mdf.min_chipload_mm) < 1e-9,
        "chipload {} should have been pulled to the {} floor",
        r.chipload_mm,
        mdf.min_chipload_mm
    );
    assert!(
        rel_err(r.rpm, 1000.0 / (4.0 * mdf.min_chipload_mm)) < 1e-9,
        "{}",
        r.rpm
    );
    assert!(
        r.notes
            .iter()
            .any(|n| n.text.contains("rubbing floor") && n.level >= NoteLevel::Warning),
        "must say why the speed came down"
    );
    // Without the ceiling the spindle would have stayed near the ideal speed.
    let free = recommend(mdf, &t, OpKind::Slot, &Machine::anolex_ultra2(), &spindle).unwrap();
    assert!(free.rpm > r.rpm);
}

#[test]
fn an_unusable_spindle_or_tool_is_refused_not_guessed() {
    let al = material(ids::ALUMINIUM_6061_T6).unwrap();
    let machine = Machine::anolex_ultra2();
    let dial = Spindle::router_dial();

    assert_eq!(
        recommend(al, &tool(0.0), OpKind::Slot, &machine, &dial),
        Err(MaterialsError::InvalidDiameter(0.0))
    );
    assert_eq!(
        recommend(
            al,
            &ToolSpec::new(6.0, 0, ToolKind::FlatEndMill, 10.0),
            OpKind::Slot,
            &machine,
            &dial
        ),
        Err(MaterialsError::InvalidFlutes(0))
    );
    assert!(matches!(
        recommend(
            al,
            &tool(6.0),
            OpKind::Slot,
            &machine,
            &Spindle::Dial { settings: vec![] }
        ),
        Err(MaterialsError::UnusableSpindle(_))
    ));
    assert!(matches!(
        recommend(
            al,
            &tool(6.0),
            OpKind::Slot,
            &Machine {
                name: "broken".into(),
                class: MachineClass::Hobby,
                max_feed_mm_min: 0.0,
                max_plunge_mm_min: 800.0,
            },
            &dial
        ),
        Err(MaterialsError::InvalidMachineLimit(_))
    ));
}

// ---------------------------------------------------------------------------
// check()
// ---------------------------------------------------------------------------

fn settings(feed: f64, plunge: f64, rpm: f64, stepdown: f64, stepover: f64) -> CamSettings {
    CamSettings {
        stepover,
        stepdown,
        feed_rate: feed,
        plunge_rate: plunge,
        spindle_rpm: rpm,
        safe_z: 5.0,
        retract_z: 10.0,
    }
}

/// A feed far too slow for the rpm is the quiet way to ruin a cutter: it looks
/// cautious and it burnishes. `check` has to name it.
#[test]
fn check_flags_a_rubbing_chipload() {
    let al = material(ids::ALUMINIUM_6061_T6).unwrap();
    let t = tool(3.175);
    // 40 mm/min at 24 000 rpm on 2 flutes is 0.00083 mm/tooth: a fifth of the
    // 0.004 mm rubbing floor.
    let s = settings(40.0, 10.0, 24_000.0, 0.3, 1.0);
    let notes = check(
        &s,
        al,
        &t,
        OpKind::Slot,
        &Machine::anolex_ultra2(),
        &Spindle::vfd_spindle(),
    );
    assert!(
        notes
            .iter()
            .any(|n| n.text.contains("rubbing floor") && n.level >= NoteLevel::Warning),
        "no rubbing note in {notes:#?}"
    );
    // And it has to say by how much.
    assert!(
        notes.iter().any(|n| n.text.contains("0.0008")),
        "the note must quote the actual chipload"
    );

    // The same settings with a sane feed produce no rubbing note.
    let ok = settings(1000.0, 250.0, 17_000.0, 0.3, 1.0);
    let notes = check(
        &ok,
        al,
        &t,
        OpKind::Slot,
        &Machine::anolex_ultra2(),
        &Spindle::vfd_spindle(),
    );
    assert!(!notes.iter().any(|n| n.text.contains("rubbing floor")));
}

/// A chipload far above the recommendation is the loud way to ruin a cutter.
#[test]
fn check_flags_an_overloaded_chipload() {
    let al = material(ids::ALUMINIUM_6061_T6).unwrap();
    let s = settings(6000.0, 300.0, 12_000.0, 0.3, 1.0);
    let notes = check(
        &s,
        al,
        &tool(3.175),
        OpKind::Slot,
        &Machine::rigid_vmc(),
        &Spindle::vfd_spindle(),
    );
    assert!(
        notes
            .iter()
            .any(|n| n.text.contains("cutter-breaking") && n.level >= NoteLevel::Warning),
        "no overload note in {notes:#?}"
    );
}

/// A 5 mm pass with a Ø1 cutter is 5 × D in one bite. There is no machine on
/// which that is a slot depth.
#[test]
fn check_flags_a_five_diameter_slot_with_a_one_millimetre_cutter() {
    let al = material(ids::ALUMINIUM_6061_T6).unwrap();
    let t = ToolSpec::new(1.0, 2, ToolKind::FlatEndMill, 6.0);
    let s = settings(300.0, 60.0, 24_000.0, 5.0, 1.0);
    for machine in [Machine::anolex_ultra2(), Machine::rigid_vmc()] {
        let notes = check(&s, al, &t, OpKind::Slot, &machine, &Spindle::vfd_spindle());
        let depth = notes
            .iter()
            .find(|n| n.text.contains("stepdown 5.000"))
            .unwrap_or_else(|| panic!("no depth note for {} in {notes:#?}", machine.name));
        assert_eq!(depth.level, NoteLevel::Danger);
        // The rigid limit for a slot in 6061 is 0.25 x D = 0.25 mm, so the
        // note must report 20x.
        assert!(
            depth.text.contains("20.0×"),
            "must say by how much: {}",
            depth.text
        );
    }

    // 0.12 mm with the same cutter is the recommended depth and is silent.
    let ok = settings(300.0, 60.0, 24_000.0, 0.12, 0.3);
    let notes = check(
        &ok,
        al,
        &t,
        OpKind::Slot,
        &Machine::anolex_ultra2(),
        &Spindle::vfd_spindle(),
    );
    assert!(!notes.iter().any(|n| n.text.contains("stepdown 0.120")));
}

#[test]
fn check_flags_a_plunge_faster_than_the_cut_and_a_stepdown_past_the_flutes() {
    let al = material(ids::ALUMINIUM_6061_T6).unwrap();
    let t = ToolSpec::new(3.175, 2, ToolKind::FlatEndMill, 3.0);
    let s = settings(1000.0, 1500.0, 17_000.0, 4.0, 1.0);
    let notes = check(
        &s,
        al,
        &t,
        OpKind::Slot,
        &Machine::rigid_vmc(),
        &Spindle::vfd_spindle(),
    );
    assert!(notes
        .iter()
        .any(|n| n.text.contains("no edge at its centre") && n.level >= NoteLevel::Warning));
    assert!(notes
        .iter()
        .any(|n| n.text.contains("flute length") && n.level == NoteLevel::Danger));
}

/// The S word does nothing on a relay-switched spindle. Saying so is the whole
/// point of carrying the spindle model into `check`.
#[test]
fn check_says_the_s_word_is_irrelevant_on_a_dial_spindle() {
    let al = material(ids::ALUMINIUM_6061_T6).unwrap();
    let s = settings(1000.0, 250.0, 12_000.0, 0.3, 0.9);
    let notes = check(
        &s,
        al,
        &tool(3.175),
        OpKind::Slot,
        &Machine::anolex_ultra2(),
        &Spindle::router_dial(),
    );
    let n = notes
        .iter()
        .find(|n| n.text.contains("does nothing"))
        .expect("must flag the S word");
    // S12000 is 2000 rpm from the nearest dial position, so this is the loud
    // version: the typed speed is not the speed that will run.
    assert_eq!(n.level, NoteLevel::Warning);
    assert!(n.text.contains("13500") || n.text.contains("13 500"));

    // A controlled spindle gets no such note.
    let notes = check(
        &s,
        al,
        &tool(3.175),
        OpKind::Slot,
        &Machine::benchtop_mill(),
        &Spindle::vfd_spindle(),
    );
    assert!(!notes.iter().any(|n| n.text.contains("does nothing")));
}

#[test]
fn check_flags_a_router_class_machine_in_stainless() {
    let ss = material(ids::STAINLESS_304).unwrap();
    assert!(!ss.router_class_ok);
    let s = settings(200.0, 40.0, 10_000.0, 0.1, 0.4);
    let notes = check(
        &s,
        ss,
        &tool(3.175),
        OpKind::Slot,
        &Machine::anolex_ultra2(),
        &Spindle::router_dial(),
    );
    assert!(notes
        .iter()
        .any(|n| n.text.contains("not a combination") && n.level >= NoteLevel::Warning));
    // And the recommendation says so too.
    let r = recommend(
        ss,
        &tool(3.175),
        OpKind::Slot,
        &Machine::anolex_ultra2(),
        &Spindle::router_dial(),
    )
    .unwrap();
    assert!(r
        .notes
        .iter()
        .any(|n| n.text.contains("not a router-class material")));
}

// ---------------------------------------------------------------------------
// Hazards
// ---------------------------------------------------------------------------

/// The dust hazards are the part of this table a user cannot recover from by
/// taking a test cut, so they are asserted by content, not by count.
#[test]
fn dust_hazards_are_present_and_reach_both_entry_points() {
    let cases = [
        (ids::FR4_COPPER_CLAD, "glass fibre"),
        (ids::CARBON_FIBRE_SHEET, "respiratory hazard"),
        (ids::MDF, "formaldehyde"),
    ];
    let machine = Machine::anolex_ultra2();
    let spindle = Spindle::router_dial();
    let s = settings(1000.0, 250.0, 17_000.0, 0.5, 1.0);

    for (id, needle) in cases {
        let m = material(id).unwrap();
        assert!(!m.hazards.is_empty(), "{id} must carry hazards");
        assert!(
            m.hazards.iter().any(|h| h.text.contains(needle)),
            "{id} hazards must mention {needle}"
        );
        // "vacuum, don't blow" has to be said for the two abrasive dusts.
        assert!(
            m.hazards
                .iter()
                .any(|h| h.text.contains("blow") || h.text.contains("compressed air")),
            "{id} must say not to blow the dust clear"
        );

        let r = recommend(m, &tool(3.175), OpKind::Slot, &machine, &spindle).unwrap();
        assert!(
            r.notes.iter().any(|n| n.text.contains(needle)),
            "{id} recommend"
        );
        let notes = check(&s, m, &tool(3.175), OpKind::Slot, &machine, &spindle);
        assert!(notes.iter().any(|n| n.text.contains(needle)), "{id} check");
    }

    // Melting plastics and welding metals are called out too.
    for (id, needle) in [
        (ids::ACRYLIC_CAST, "melts"),
        (ids::POLYCARBONATE, "melts"),
        (ids::COPPER_C110, "welds"),
        (ids::ALUMINIUM_6061_T6, "welds"),
    ] {
        let m = material(id).unwrap();
        let r = recommend(m, &tool(3.175), OpKind::Slot, &machine, &spindle).unwrap();
        assert!(
            r.notes.iter().any(|n| n.text.contains(needle)),
            "{id} must mention {needle}"
        );
    }
}

// ---------------------------------------------------------------------------
// The table as data
// ---------------------------------------------------------------------------

#[test]
fn every_promised_material_is_present_with_a_stable_id() {
    let expected = [
        ids::ALUMINIUM_6061_T6,
        ids::ALUMINIUM_7075_T6,
        ids::BRASS_C360,
        ids::COPPER_C110,
        ids::MILD_STEEL,
        ids::STAINLESS_304,
        ids::MDF,
        ids::PLYWOOD,
        ids::HARDWOOD,
        ids::SOFTWOOD,
        ids::ACRYLIC_CAST,
        ids::POLYCARBONATE,
        ids::POM_ACETAL,
        ids::HDPE,
        ids::FR4_COPPER_CLAD,
        ids::CARBON_FIBRE_SHEET,
    ];
    assert_eq!(materials().len(), expected.len());
    for id in expected {
        let m = material(id).unwrap_or_else(|| panic!("{id} missing"));
        assert_eq!(m.id, id);
        assert!(!m.name.is_empty());
        assert!(m.chipload_ref_mm > 0.0 && m.min_chipload_mm > 0.0);
        assert!(m.surface_speed.min <= m.surface_speed.target);
        assert!(m.surface_speed.target <= m.surface_speed.max);
        assert!(m.slot_depth_frac > 0.0 && m.profile_depth_frac >= m.slot_depth_frac);
        assert!(m.radial_frac > 0.0 && m.radial_frac <= 1.0);
        assert!(m.plunge_frac > 0.0 && m.plunge_frac <= 1.0);
        assert!(m.ramp_angle_deg > 0.0 && m.ramp_angle_deg < 45.0);
    }
    assert!(material("no-such-material").is_none());
    // Stainless is the one marked as not for router-class machines.
    assert_eq!(
        materials()
            .iter()
            .filter(|m| !m.router_class_ok)
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>(),
        vec![ids::MILD_STEEL, ids::STAINLESS_304]
    );
}

#[test]
fn the_chipload_curve_is_anchored_at_one_eighth_inch_and_clamps_outside_the_table() {
    let al = material(ids::ALUMINIUM_6061_T6).unwrap();
    assert!(rel_err(al.chipload_at(CHIPLOAD_REF_DIAMETER_MM), al.chipload_ref_mm) < 1e-12);
    // Outside the table the endpoints are held.
    assert!(rel_err(al.chipload_at(0.1), al.chipload_at(MIN_TABLE_DIAMETER_MM)) < 1e-12);
    assert!(rel_err(al.chipload_at(40.0), al.chipload_at(MAX_TABLE_DIAMETER_MM)) < 1e-12);
    // And the recommender says the tool is off the table.
    let r = recommend(
        al,
        &tool(0.2),
        OpKind::Slot,
        &Machine::anolex_ultra2(),
        &Spindle::router_dial(),
    )
    .unwrap();
    assert!(r.notes.iter().any(|n| n.text.contains("outside the")));
}

/// The published hobby-router guidance for aluminium at 1/8" is
/// 0.001–0.002 inch per tooth. The table plus the hobby derating has to land
/// inside it, because that is the number everyone will check first.
#[test]
fn aluminium_at_one_eighth_inch_lands_in_the_published_hobby_band() {
    let al = material(ids::ALUMINIUM_6061_T6).unwrap();
    let r = recommend(
        al,
        &tool(CHIPLOAD_REF_DIAMETER_MM),
        OpKind::Slot,
        &Machine::anolex_ultra2(),
        &Spindle::router_dial(),
    )
    .unwrap();
    let inches = r.chipload_mm / 25.4;
    assert!(
        (0.001..=0.002).contains(&inches),
        "{:.5} in/tooth is outside the 0.001–0.002 hobby band",
        inches
    );
    // ~0.1 x D slotting depth in aluminium for small cutters on a hobby
    // machine.
    let ratio = r.stepdown_mm / CHIPLOAD_REF_DIAMETER_MM;
    assert!((0.08..=0.15).contains(&ratio), "stepdown is {ratio} x D");
}

// ---------------------------------------------------------------------------
// Serde
// ---------------------------------------------------------------------------

#[test]
fn everything_round_trips_through_serde() {
    let m = material(ids::COPPER_C110).unwrap();
    let json = serde_json::to_string(m).unwrap();
    assert!(json.contains("copper-c110"));
    let back: Material = serde_json::from_str(&json).unwrap();
    assert_eq!(&back, m);

    let machine = Machine::anolex_ultra2();
    let back: Machine = serde_json::from_str(&serde_json::to_string(&machine).unwrap()).unwrap();
    assert_eq!(back, machine);

    let spindle = Spindle::router_dial();
    let back: Spindle = serde_json::from_str(&serde_json::to_string(&spindle).unwrap()).unwrap();
    assert_eq!(back, spindle);

    let t = tool(2.0);
    let back: ToolSpec = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
    assert_eq!(back, t);

    let rec = recommend(m, &t, OpKind::Slot, &machine, &spindle).unwrap();
    let json = serde_json::to_string(&rec).unwrap();
    let back: Recommendation = serde_json::from_str(&json).unwrap();
    // Everything but the floats compares exactly; the floats come back within
    // a last-place rounding of the shortest decimal representation.
    assert_eq!(back.material_id, rec.material_id);
    assert_eq!(back.op, rec.op);
    assert_eq!(back.dial, rec.dial);
    assert_eq!(back.flutes, rec.flutes);
    assert_eq!(back.coolant, rec.coolant);
    assert_eq!(back.notes, rec.notes);
    for (a, b) in [
        (back.rpm, rec.rpm),
        (back.chipload_mm, rec.chipload_mm),
        (back.feed_mm_min, rec.feed_mm_min),
        (back.plunge_mm_min, rec.plunge_mm_min),
        (back.stepdown_mm, rec.stepdown_mm),
        (back.stepover_mm, rec.stepover_mm),
        (back.finish_allowance_mm, rec.finish_allowance_mm),
        (back.ramp_angle_deg, rec.ramp_angle_deg),
        (back.surface_speed_m_min, rec.surface_speed_m_min),
        (back.tool_diameter_mm, rec.tool_diameter_mm),
    ] {
        assert!(rel_err(a, b) < 1e-12, "{a} != {b}");
    }
    assert!(json.contains("\"dial\":\"2\""));

    let notes = check(
        &settings(250.0, 40.0, 13_500.0, 0.17, 2.0),
        m,
        &t,
        OpKind::Slot,
        &machine,
        &spindle,
    );
    let back: Vec<Note> = serde_json::from_str(&serde_json::to_string(&notes).unwrap()).unwrap();
    assert_eq!(back, notes);

    // The whole table serialises, which is what a UI listing needs.
    let json = serde_json::to_string(materials()).unwrap();
    let back: Vec<Material> = serde_json::from_str(&json).unwrap();
    assert_eq!(back.len(), materials().len());
}

// ---------------------------------------------------------------------------
// Integration with CamSettings
// ---------------------------------------------------------------------------

/// Friction-log item 52: five operations, no way to change material once.
/// `apply_to` is the one-call version.
#[test]
fn apply_to_writes_the_recommendation_into_cam_settings() {
    let m = material(ids::COPPER_C110).unwrap();
    let rec = recommend(
        m,
        &ToolSpec::new(2.0, 2, ToolKind::FlatEndMill, 6.0),
        OpKind::Slot,
        &Machine::anolex_ultra2(),
        &Spindle::router_dial(),
    )
    .unwrap();

    let mut s = CamSettings::default();
    let (safe_z, retract_z) = (s.safe_z, s.retract_z);
    rec.apply_to(&mut s);

    assert!(rel_err(s.feed_rate, rec.feed_mm_min) < 1e-12);
    assert!(rel_err(s.plunge_rate, rec.plunge_mm_min) < 1e-12);
    assert!(rel_err(s.spindle_rpm, rec.rpm) < 1e-12);
    assert!(rel_err(s.stepdown, rec.stepdown_mm) < 1e-12);
    assert!(rel_err(s.stepover, rec.stepover_mm) < 1e-12);
    // Clearance heights are the job's, not the material's.
    assert_eq!(s.safe_z, safe_z);
    assert_eq!(s.retract_z, retract_z);

    // And a round trip through check on the recommendation's own numbers
    // raises nothing above a caution.
    let notes = check(
        &s,
        m,
        &ToolSpec::new(2.0, 2, ToolKind::FlatEndMill, 6.0),
        OpKind::Slot,
        &Machine::anolex_ultra2(),
        &Spindle::router_dial(),
    );
    assert!(
        !notes.iter().any(|n| n.level == NoteLevel::Danger),
        "check must not condemn recommend's own output: {notes:#?}"
    );
}

#[test]
fn tool_spec_reads_a_tool_through_its_accessors_only() {
    let t = Tool::FlatEndMill {
        diameter: 3.175,
        flute_length: 12.0,
        flutes: 3,
    };
    let spec = ToolSpec::from_tool(&t, ToolKind::FlatEndMill);
    assert!(rel_err(spec.diameter_mm, 3.175) < 1e-12);
    assert_eq!(spec.flutes, 3);
    assert!(rel_err(spec.flute_length_mm, 12.0) < 1e-12);

    // A drill reports no flute length; three diameters is the fallback.
    let d = Tool::Drill {
        diameter: 2.5,
        point_angle: 118.0,
    };
    let spec = ToolSpec::from_tool(&d, ToolKind::Drill);
    assert!(rel_err(spec.flute_length_mm, 7.5) < 1e-12);
}

/// A profile that cuts a part free from sheet stock is buried on both sides
/// and must be run as a slot, not a profile. The slot recommendation says so.
#[test]
fn a_slot_warns_that_the_cutter_is_buried() {
    let r = recommend(
        material(ids::COPPER_C110).unwrap(),
        &tool(2.0),
        OpKind::Slot,
        &Machine::anolex_ultra2(),
        &Spindle::router_dial(),
    )
    .unwrap();
    assert!(r.notes.iter().any(|n| n.text.contains("buried cut")));
    assert!(rel_err(r.stepover_mm, 2.0) < 1e-12);

    // A profile takes a fraction of the diameter and feeds faster per tooth.
    let p = recommend(
        material(ids::COPPER_C110).unwrap(),
        &tool(2.0),
        OpKind::Profile,
        &Machine::anolex_ultra2(),
        &Spindle::router_dial(),
    )
    .unwrap();
    assert!(p.stepover_mm < 2.0);
    assert!(
        p.chipload_mm > r.chipload_mm,
        "chip thinning must raise the feed per tooth"
    );
    assert!(
        p.stepdown_mm > r.stepdown_mm,
        "a side cut goes deeper than a slot"
    );
}
