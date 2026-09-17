# Native app friction log

Beta-user notes from one real job: open the rana-60-cnc stator in the native
Mac app and machine it on the Anolex 4030. Logged 2026-09-16/17. Items marked
**fixed** landed on `claude/vcad-ui-polish-brainstorm-5879d5`; everything else
is open.

## Opening files

1. The native app cannot open STEP, STL or DXF as geometry. `.loon` opened only
   after it was added (**fixed**: `.loon` opens as generated geometry). STEP/STL
   import exists only in the MCP server and CLI.
2. **Fixed.** Launching the app by opening a file produced two Dock tiles: the
   file arrived before the editor view registered its model, so the delegate
   spawned a second instance and left an empty "Untitled" behind. Launch-time
   files are now queued until the view attaches.
3. `open vcad.app file.dxf` hands the DXF to the system default handler, not
   vcad, because the bundle does not declare DXF as an importable type. Only
   `open -a` forces it. Declaring DXF (Viewer role) would make drag-to-Dock and
   Finder "Open With" work.
4. Opening a file into a running instance gives no feedback about which
   workspace it landed in or whether it replaced the scratch document.
5. **Fixed.** Geometry was evaluated on the main thread, so a slow document
   meant "Application Not Responding" with no spinner and no cancel. The kernel
   now runs on its own thread; the viewport shows "Solving <name>… n of N" and
   draws parts as they land.
6. **Fixed.** Every document was evaluated twice: the viewport rebuild key
   included the triangle count, an output of the build.
7. **Fixed.** The app never used the root-mesh cache (its three open entry
   points passed default options). It now consults `~/.cache/vcad`, so anything
   a vcad tool has already solved opens without a kernel walk (measured on a
   12-stage stator prefix: 3.39 s cold, 0.00 s warm).
8. **Fixed.** Save writes `<doc>.vcadmesh` beside the document and Open imports
   it first, so a file sent to another machine on the same kernel build opens
   instantly. Caveat: the bundle is written from the last *full* evaluation's
   keys; an unsaved scrub can leave a key stale until the next full solve.
9. A long solve has no ETA, no cancel, and no fallback such as showing the DXF
   outline or a coarser tessellation while the solid finishes.
10. The Dock tile and menu bar come up long before the window shows anything,
    so a slow launch reads as broken rather than busy.

## Kernel cost on the stator

11. **Fixed.** The stator is one root: a 50-stage `[pipe …]` of unions and
    differences on a 256-segment ring, every operand sharing the same top and
    bottom planes. On `main` it did not finish (40+ min, memory past 16 GB).
    Reference volume, grid-integrated straight from the CSG source:
    **7848 mm³**. It now solves to **7853 mm³ (+0.06%)**, fully `Analytic`,
    ~11.7k triangles, in about 24 s (debug build). An intermediate state got
    the volume right only as ~105k-triangle soup; the last two bullets are
    what kept it analytic.
    Eight separate causes, found with `VCAD_UNION_TRACE=1` (per-union fidelity,
    time and volumes), `sample`, and rasterising the result's caps against
    the reference:
    - *A split refused without a word — the root cause.*
      `find_line_polygon_crossings` read a face's plane off its first three
      vertices. Splitting a neighbouring cap imprints vertices on the shared
      edge, so the loop opened with three collinear points, the face read as
      degenerate, and the cut was dropped: `(post ∪ A) ∪ B` kept the post's
      un-notched side as an interior membrane (+6.75 mm³, open edges) while
      `post ∪ A` and `post ∪ (A ∪ B)` were exact. It looked like a tangency
      bug for a day; it was not. **Fixed** (Newell normal).
    - *One ray.* The mesh CSG classified every fragment with a single +X ray.
      Any ray threading that membrane read the wrong parity, so 35 mm² of the
      ring's top cap, nowhere near a post, was dropped: the fallback union
      came out 1600 mm³ short, inside its own volume bound. **Fixed**:
      membership is a vote of three perpendicular rays, and uses the ray index
      (the L5 union went from 6.5 s to under a second).
    - *A tangent arc mistaken for the circle's own.* A circle is not offered
      as a split to a planar face that already has it as a boundary arc (≥3
      vertices on it). A fillet tangent to the bore samples its rim densely
      enough that a run of ITS vertices sits inside that tolerance, so the
      bore never cut the fillet blocks' caps. **Fixed**: the on-circle
      vertices must also have the circle's radius as their circumradius.
    - *Silent wrong analytic unions.* **Fixed** for the gross case by the
      union volume bound `max(A, B) ≤ vol(A ∪ B) ≤ A + B`, and for the subtle
      one by a referee: a cracked union is compared with the mesh boolean of
      the same operands, and loses if that one is ≥20× cleaner and they
      disagree by more than tessellation slack.
    - *Naming was the 40 minutes.* ~100% of a chained soup boolean was
      `vcad_kernel_naming::propagate_boolean` sorting thousands of sibling
      names per coplanar cap triangle. **Fixed**.
    - *Coincidence judged one-sidedly.* A small cap patch lying inside a large
      coplanar cap read `OnSame`, but the large cap read `Outside` against the
      patch, so with the small solid as operand A both copies survived:
      `ring ∪ post` exact, `post ∪ ring` +5.44 mm³. **Fixed**: a one-sided
      match (`OnSameInner`) lets the larger face win.
    - *Phantom full-width chords.* Every post plane handed the ring's cap a
      chord across the whole face though the post reaches 0.5 mm into the
      wall; two posts' chords cross inside the cap and the pieces classify
      incoherently (12 plain posts lost 9.3%). **Fixed**: a line split of a
      planar face is skipped when the cutter face ends flush on that plane and
      the other solid's coplanar, same-facing face is contained in it. Also
      fixed on the way: a line crossing a notched face in several spans always
      cut the FIRST span (the sheet-metal bend-relief discrepancy).
    - **Open:** ~100 unpaired edges remain on the stator (tangent fillet
      rims) — analytic and inside tolerance, not watertight. Chained mesh
      booleans still re-split each other's coplanar caps without any re-merge,
      and the mesh fallback is not bit-reproducible across processes (neither
      matters to this part any more, both still matter).
12. **Retracted.** An earlier revision of this log claimed the stator was
    fixed: reassociating the union chain (smallest operands first, re-pairing
    around a non-analytic pair) took it to 52 s, all `Analytic`. That result
    measured 2554 mm³ — a third of the part. `Analytic` fidelity is not a
    validity signal, and nobody, including me, checked a volume until a
    regression test asserted one. What survives of that work: the authored
    order is kept whenever it is clean; a search runs only when the authored
    fold fails with a long tail behind it; and a stalled search mesh-unions
    its last few groups instead of restarting the 50-step fold.
13. A loon author gets no hint that facet count and tangent placement multiply
    boolean cost. The evaluator already records `eval_ms` per node; surfacing
    it in the app (or `vcad info --timings`) would point straight at the two
    operations that cost 200 s.
14. No sub-root caching: a root is solved whole or not at all. Caching
    intermediate solids needs a serialized BRep (`Solid` has none today; STEP
    round-trip is the only existing path and is too lossy/slow for a cache).

## Manufacture workspace

15. **Fixed.** Native CAM was rectangular only (face / pocket / profile on
    width × height); the part's outline was never used. Contour operations
    (outside/inside along a closed polyline, with holding tabs) now exist, fed
    by a DXF outline import.
16. The outline is a separate file from the part. Nothing checks the DXF
    against the loaded solid (same bounds, same holes), so a stale outline
    machines silently.
17. Stock and origin are manual. Importing an outline sets stock X/Y from its
    extents but leaves thickness at the 10 mm default (the stator plate is
    6 mm); "Use model bounds" reads RealityKit bounds, and the stator is
    modelled at z 11.1–17.1 (its place in the can), so stock top ≠ part top
    until "Place at model top".
18. Part coordinates are centred on the origin while CAM assumes XY lower-left
    at 0 with stock top Z0. Nothing translates the part into the stock frame;
    the outline import does it for the DXF only.
19. Single tool only: the three M3 pilots (Ø2.5) are smaller than the Ø3.175
    end mill and there is no drill op, so they cannot be machined in the app.
    The import now refuses them with a message rather than dropping them.
20. Contour CAM cuts the full stock thickness in 0.5 mm stepdowns (12 passes,
    about 17.5 min for the stator). No roughing/finishing split, no
    stock-to-leave, no ramp or helix entry, no lead-in; the estimate treats
    rapids at a fixed 3000 mm/min.
21. Tabs are a count (3 × 4 mm × 1 mm by default) placed by the kernel; the
    user cannot see or move them in the viewport before cutting.
22. Bore-and-slots is one inside contour. The 5 mm slots between posts are only
    1.8 mm wider than the cutter, and nothing warns when a slot is narrower
    than the tool — it would simply not be cut.
23. The toolpath preview is reachable only after Generate, in the Toolpaths
    stage; Setup gives no summary of what was generated (moves, time, depth).
24. Nothing confirms an import succeeded except a small caption in the
    outline; the inspector should jump to the new operation.
25. **Fixed.** The bottom panel said "Disconnected" three times and carried the
    connection form, overrides and setup checklist twice each. It is now one
    machine bar (state, position, WCS, jog/zero/probe/overrides, readiness,
    Run/Stop) with an optional Terminal/G-code/Macros drawer; the item
    inspector lives in the right rail and follows the outline selection.
26. **Fixed.** Switching workspaces shrank the window to 346 pt: the hosting
    view let SwiftUI's ideal height drive the window's max size.

## Tooling

27. The CLI binary and the app binary are both named `vcad`. Killing one by
    name kills the other; every stator timing taken while restarting the app
    was cut short without saying so. A distinct process name for one of them
    would prevent it.
28. A git worktree builds with a different kernel id than the main checkout
    (content-hashed sources), so its cache directory starts cold even for
    documents the main checkout has solved.
29. **Fixed.** After any crash or force-quit, the next launch sat behind
    AppKit's modal "quit unexpectedly while reopening windows" prompt with no
    editor window (files passed at launch were never opened), and choosing to
    reopen crashed inside `NSPersistentUIRestorer` restoring the hidden SwiftUI
    host window. The app owns its windows and layout, so it now opts out of
    AppKit state restoration (`ApplePersistenceIgnoreState`).
30. A degraded solid looks exactly like a good one: a part that falls back to
    triangle soup (faces no longer selectable) opens and the viewport says nothing;
    the kernel already records the loss (`Solid::provenance`,
    `BooleanReport`), the app just never surfaces it.
31. Verifying the app blind is slow: the editor window is borderless, so it
    has no accessibility window, no Window-menu entry and no title to query.
    A `VCAD_STATUS` dump (document, workspace, solve state) on a signal or a
    debug menu item would have saved an hour here.
