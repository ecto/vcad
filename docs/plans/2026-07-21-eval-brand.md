# vcad evals — brand family + subdomain plan

Status: design draft, 2026-07-21. Companion to
[2026-07-21-pcbeval.md](2026-07-21-pcbeval.md).

## Brand architecture

- **Family pattern**: `<domain>eval` — atomeval, mecheval, pcbeval, simeval.
  The suffix is the brand family (the `-bench` play). Each chapter is
  self-describing, tweetable, paper-citable.
- **Umbrella**: descriptive, not coined — **"vcad evals"** at `eval.vcad.io`.
  The umbrella's unique product is the **cross-domain index**: one aggregate
  number per model across chapters (only models with runs in ≥2 domains),
  plus cross-scale chained tasks (see below).
- **The scale ladder** (the umbrella narrative — "from atoms to machines"):

  | chapter | scale | oracles |
  |---|---|---|
  | atomeval | atoms → materials | md_run, minimize_energy, homogenize_material, design_material |
  | mecheval | parts → machines | kernel mass-props, physics gym, fit suite |
  | pcbeval  | boards → circuits | DRC/ERC, simulate_circuit, validate_for_fab |
  | simeval  | fields → systems | thermal, EM, photonics, antenna, acoustics, particle |

  Boundary rule: atomeval = the agent designs **matter** (molecules,
  lattices, materials); simeval = the agent designs **devices graded by
  field behavior**. Keeps each chapter's submission format coherent.
- **Cross-scale chained tasks** are the umbrella's signature content: e.g.
  design a lattice material atomically → homogenize → the bracket made from
  it must pass FEA (atomeval feeds mecheval via the homogenize bridge).
  Only expressible because all chapters share one document format and one
  receipt system.
- **Tagline direction**: "Evals for atoms, not tokens. Graded by kernels,
  not vibes."
- **Visual system**: locked vcad brand — ▽ mark, Inter + JetBrains Mono,
  green = proof (passes render proof-green), orange = action. One accent
  color per domain chapter + one hero artifact per chapter (OPERATOR mech /
  fabbed board photo / sim rollout); everything else identical across
  chapters. The sameness is the message: one kernel grades all of it.
- **Chapter page skeleton** (shared): hero artifact → leaderboard →
  "audit any number" forensic-blob links → villain-baselines footnote →
  "part of vcad evals" family footer.

## Domains

Buy now: pcbeval.com, simeval.com, atomeval.com.

| Domain | Target |
|---|---|
| eval.vcad.io | canonical site (one Vercel deploy) |
| mecheval.com | 301 → eval.vcad.io/mech |
| pcbeval.com | 301 → eval.vcad.io/pcb |
| simeval.com | 301 → eval.vcad.io/sim |
| atomeval.com | 301 → eval.vcad.io/atom |

Domains are handles for distribution, not homes. 301s consolidate SEO onto
vcad.io. Chapter pages render under their own name ("MechEval — part of
vcad evals") so the redirect target feels like the brand's home.

## Site structure (eval.vcad.io)

```
/            index: family overview, cross-domain model table, methodology
/atom        AtomEval chapter (later)
/mech        MechEval chapter (current mecheval.com content)
/pcb         PCBEval chapter
/sim         SimEval chapter (later)
/runs/<task>/<model>/<run>   permalinked forensic blobs (citability)
/methodology how grading works: fail-closed oracles, pass^k, splits, villains
/submit      submission spec (docker solve() pattern)
```

## Implementation

Repurpose `mecheval/leaderboard` into the multi-domain build:

1. `build.ts`: single-leaderboard build → loop over domain configs
   `{slug, name, accent, runsDir, tasksDir, heroRenderer}` emitting one
   `dist/` with `/`, `/mech/*`, `/pcb/*`.
2. `tokens.ts`: add per-domain accent tokens.
3. Index page: cross-domain aggregate table; a model appears once it has
   official runs in ≥2 domains (keeps the index honest while sim is empty).
4. Render cache pattern carries over: pcb chapters commit `render_pcb` PNGs
   to `leaderboard/cache/` the same way `vcad-render` SVGs are cached, so
   Vercel builds without Rust.
5. Vercel: existing project gains `eval.vcad.io` as canonical alias, the
   three .com domains, and a `redirects` block (301, path-preserving).

## Marketing cadence (one campaign, three chapters)

1. **mecheval v1** — full model matrix, audited blobs, writeup. Establishes
   the format's credibility.
2. **pcbeval launch** (~4–6 weeks later) — rides mecheval awareness; hero
   move: fab the winning board (cheap — one JLC order) and photograph it.
3. **suite X / simeval** — "and they work *together*": the cross-domain
   finale only vcad's unified kernel can grade.
4. **atomeval** — the scale-ladder capstone: matter design graded by MD +
   homogenization, with chained atom→material→part tasks landing back in
   mecheval. Sequencing flexible — it can also launch third if the MD
   oracles mature faster than suite X.

Each launch reuses the same kit: leaderboard drop + hero artifact + writeup
+ model-lab outreach with their scores. Quarterly official runs, versioned
harness, immutable results (SWE-bench release model, not a live board).
