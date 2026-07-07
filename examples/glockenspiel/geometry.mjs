/**
 * Glockenspiel geometry — the single source of truth for every dimension.
 *
 * Eight free-free aluminum bars tuned to C6–C7 major plus a folded
 * sheet-metal stand. Every number here is derived from physics or from a
 * published SendCutSend capability, so the receipt claims in the demo are
 * reproducible from this file alone.
 *
 * Physics: the fundamental of a free-free rectangular bar in transverse
 * vibration is closed-form (Euler–Bernoulli, first mode):
 *
 *   f₁ = (4.730)² / (2π L²) · sqrt(E I / ρ A)   with I/A = t²/12
 *      = 3.5608 · (t / L²) · sqrt(E / 12ρ)
 *
 * Nodal lines of that mode sit at 0.2242·L from each end — the bars hang
 * on cord through holes centered there, so the suspension doesn't damp
 * (or detune) the note.
 *
 * Material constants MUST match crates/vcad-kernel-sheet/src/materials.rs
 * ("al-hard" = 6061-T6). build.mjs asserts this against the live engine
 * registry at run time.
 */

/** First free-free mode coefficient: (4.730)² / 2π. */
export const FREE_FREE_COEFF = 3.5608;
/** Nodal line of the first free-free mode, as a fraction of L from each end. */
export const NODAL_FRAC = 0.2242;

/** 6061-T6 constants — must equal the engine registry's "al-hard" entry. */
export const BAR_MATERIAL = {
  key: "6061", // resolves to al-hard in the vcad materials registry
  registryName: "al-hard",
  displayName: "Aluminum 6061-T6",
  modulusGpa: 69.0,
  densityKgM3: 2700.0,
};

export const BAR = {
  thicknessMm: 3.175, // 0.125" — SCS cuts 6061 at this stock (cut only, no bends)
  widthMm: 25.0, // width cancels in I/A: doesn't move f₁ to first order
  holeDiaMm: 4.2, // cord hole; 132% of thickness (SCS aluminum min ≈ 50%)
};

/** C6–C7 major scale, equal temperament, A4 = 440 Hz. */
export const NOTES = [
  { name: "C6", semisFromA4: 15 },
  { name: "D6", semisFromA4: 17 },
  { name: "E6", semisFromA4: 19 },
  { name: "F6", semisFromA4: 20 },
  { name: "G6", semisFromA4: 22 },
  { name: "A6", semisFromA4: 24 },
  { name: "B6", semisFromA4: 26 },
  { name: "C7", semisFromA4: 27 },
];

export const targetHz = (semisFromA4) => 440 * 2 ** (semisFromA4 / 12);

/**
 * f₁ · L² for this stock (SI): 3.5608 · t · sqrt(E / 12ρ).
 * ≈ 16.50 Hz·m² for 6061 at 3.175 mm.
 */
export function barConstantSI(
  { thicknessMm } = BAR,
  { modulusGpa, densityKgM3 } = BAR_MATERIAL,
) {
  const t = thicknessMm / 1000;
  const E = modulusGpa * 1e9;
  return FREE_FREE_COEFF * t * Math.sqrt(E / (12 * densityKgM3));
}

/** Predicted fundamental (Hz) for a bar of length `lengthMm`. */
export function predictedHz(lengthMm) {
  const L = lengthMm / 1000;
  return barConstantSI() / (L * L);
}

/** Pitch error of `actualHz` vs `refHz`, in cents. */
export const cents = (actualHz, refHz) => 1200 * Math.log2(actualHz / refHz);

/**
 * Solve each note's bar from the CLOSED-FORM model: length from the target
 * pitch (rounded to 0.1 mm — the shop's cut tolerance dwarfs anything
 * finer), nodal hole positions, and the frequency the as-modeled length
 * predicts. This is the plan doc's published table.
 */
export function barSpecs() {
  const C = barConstantSI();
  return NOTES.map(({ name, semisFromA4 }) => {
    const fT = targetHz(semisFromA4);
    const exactMm = Math.sqrt(C / fT) * 1000;
    const lengthMm = Math.round(exactMm * 10) / 10;
    const holeFromEndMm = NODAL_FRAC * lengthMm;
    const predicted = predictedHz(lengthMm);
    return {
      note: name,
      targetHz: fT,
      lengthMm,
      holeFromEndMm,
      holeXsMm: [holeFromEndMm, lengthMm - holeFromEndMm],
      predictedHz: predicted,
      errorCents: cents(predicted, fT),
    };
  });
}

/** BarSpec (acoustics-module shape) for a bar of length L with nodal holes. */
export function acousticBar(lengthMm) {
  const h = NODAL_FRAC * lengthMm;
  return {
    length_mm: lengthMm,
    width_mm: BAR.widthMm,
    thickness_mm: BAR.thicknessMm,
    holes_mm: [h, lengthMm - h],
    hole_dia_mm: BAR.holeDiaMm,
    modulus_gpa: BAR_MATERIAL.modulusGpa,
    density_kg_m3: BAR_MATERIAL.densityKgM3,
  };
}

/**
 * Hole-compensated bar specs: the Ø4.2 mm nodal holes remove bending
 * stiffness where mode-1 curvature is nonzero, flattening every bar by
 * ~5 cents — the audio simulation caught this before the order. Shorten
 * each bar (f ∝ 1/L², fixed point in 3 iterations) so the hole-aware FEM
 * fundamental lands on the target, then round to the 0.1 mm cut grid.
 *
 * `femHz(bar, count)` is injected (from @vcad/mcp's acoustics module) so
 * this file stays dependency-free for closed-form use.
 */
export function compensateBarSpecs(femHz, specs = barSpecs()) {
  return specs.map((s) => {
    let L = s.lengthMm;
    for (let i = 0; i < 3; i++) {
      const f = femHz(acousticBar(L), 1)[0];
      L *= Math.sqrt(f / s.targetHz);
    }
    const lengthMm = Math.round(L * 10) / 10;
    const holeFromEndMm = NODAL_FRAC * lengthMm;
    const predicted = femHz(acousticBar(lengthMm), 1)[0];
    return {
      ...s,
      lengthMm,
      holeFromEndMm,
      holeXsMm: [holeFromEndMm, lengthMm - holeFromEndMm],
      closedFormLengthMm: s.lengthMm,
      predictedHz: predicted,
      errorCents: cents(predicted, s.targetHz),
    };
  });
}

// ── Stand — folded 5052 U-channel (SCS bends 5052; 6061 is cut-only) ──────

export const STAND_MATERIAL = {
  key: "al-soft", // vcad registry key; SCS catalog resolves it to al-5052
  scsKey: "al-5052",
  displayName: "Aluminum 5052-H32",
};

export const STAND = {
  lengthMm: 300, // channel axis (X); bars run across it (Y)
  widthMm: 100, // deck width; the longest bar overhangs by ~13 mm a side
  thicknessMm: 3.18, // 0.125" 5052 — SCS bends this at fixed R = 3.18 mm
  chamferMm: 12, // corner chamfers: no sharp corners, and they put material
  //                at the wall-bend ends — the honest bend-relief case
  wallMm: 30, // formed wall height (SCS min formed flange 12.09 mm)
  footMm: 15, // outward feet off each wall (same min-flange rule)
  holeDiaMm: 4.2, // cord anchor holes, matching the bar holes
  barPitchMm: 35, // 25 mm bar + 10 mm gap
};

/** Bar station centers along the channel axis. */
export function standStationsMm() {
  const n = NOTES.length;
  const span = (n - 1) * STAND.barPitchMm;
  const first = (STAND.lengthMm - span) / 2;
  return NOTES.map((_, i) => first + i * STAND.barPitchMm);
}

/** CCW deck outline: rectangle with four chamfered corners. */
export function standOutline() {
  const { lengthMm: L, widthMm: W, chamferMm: c } = STAND;
  return [
    { x: c, y: 0 },
    { x: L - c, y: 0 },
    { x: L, y: c },
    { x: L, y: W - c },
    { x: L - c, y: W },
    { x: c, y: W },
    { x: 0, y: W - c },
    { x: 0, y: c },
  ];
}

/** A CW circle (hole winding) as a polygon loop. */
export function circleCW(cx, cy, r, n = 64) {
  const pts = [];
  for (let i = 0; i < n; i++) {
    const a = -(2 * Math.PI * i) / n;
    pts.push({ x: cx + r * Math.cos(a), y: cy + r * Math.sin(a) });
  }
  return pts;
}

/**
 * Cord-anchor holes in the deck: two per bar, directly under that bar's
 * nodal holes. Two converging rows — the physics is visible in the frame.
 *
 * 24-gon circles: 0.018 mm from true at Ø4.2 (an order of magnitude inside
 * the shop's cut tolerance) and it keeps the folded STEP export ~12 MB —
 * face count in the B-rep writer scales with hole segments.
 */
export function standHoles(specs = barSpecs()) {
  const stations = standStationsMm();
  const midY = STAND.widthMm / 2;
  const holes = [];
  specs.forEach((bar, i) => {
    const off = (0.5 - NODAL_FRAC) * bar.lengthMm;
    for (const y of [midY - off, midY + off]) {
      holes.push(circleCW(stations[i], y, STAND.holeDiaMm / 2, 24));
    }
  });
  return holes;
}

/**
 * The fold chain: two walls down off the long deck edges, then an outward
 * foot off each wall. Edge indices follow the CCW outline above (edge 0 is
 * the y=0 run, edge 4 the y=W run); on a wall panel, edge 2 is its free edge.
 */
export function standFlanges() {
  return [
    { edge_index: 0, length: STAND.wallMm, direction: "Down" },
    { edge_index: 4, length: STAND.wallMm, direction: "Down", panel_id: 0 },
    { edge_index: 2, length: STAND.footMm, direction: "Down", panel_id: 1 },
    { edge_index: 2, length: STAND.footMm, direction: "Down", panel_id: 2 },
  ];
}

/** The sheet_metal_create argument for a single bar. */
export function barCreateArgs(spec) {
  const { widthMm, thicknessMm, holeDiaMm } = BAR;
  return {
    outline: [
      { x: 0, y: 0 },
      { x: spec.lengthMm, y: 0 },
      { x: spec.lengthMm, y: widthMm },
      { x: 0, y: widthMm },
    ],
    holes: spec.holeXsMm.map((x) => circleCW(x, widthMm / 2, holeDiaMm / 2)),
    thickness: thicknessMm,
    material: BAR_MATERIAL.key,
    shop_profile: "sendcutsend",
  };
}

/** The sheet_metal_create argument for the stand. `relief` toggles the fix. */
export function standCreateArgs(relief, specs = barSpecs()) {
  return {
    outline: standOutline(),
    holes: standHoles(specs),
    thickness: STAND.thicknessMm,
    material: STAND_MATERIAL.key,
    shop_profile: "sendcutsend",
    flanges: standFlanges(),
    ...(relief ? { bend_relief: true } : {}),
  };
}
