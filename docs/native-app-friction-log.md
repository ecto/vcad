# Native app friction log

Beta-user notes from one real job: open the rana-60-cnc stator in the native
Mac app and machine it on the Anolex 4030. Logged 2026-09-16/17. Items marked
**fixed** are done and say how; everything else is open. The first ones landed
on `claude/vcad-ui-polish-brainstorm-5879d5`, the rest through the CAM
roadmap's wave-3 app packages and the pass that joined them
(`cam/w3-app-integrate`, 2026-09-18) — which found items 55–62 of its own.
The follow-up pass (`cam/w3-app-followups`, 2026-09-18) closed 19, 51 and 55,
the open caveats on 16 and 45, and found 63–67.

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
16. **Fixed.** The outline was a separate file from the part and nothing
    checked the DXF against the loaded solid, so a stale outline machined
    silently. Every import is now compared against a section of the part on
    screen (`CNCWorkspace.compareWithModel`, via `vcad_cam_compare_outline`),
    and a disagreement is a warning that has to be acknowledged — not a
    refusal, because a fixture is legitimately not the part. Being unable to
    compare is itself said out loud: silence there would read as agreement.
    The caveat that stood here — *"the comparison runs at import only … a
    stale outline is caught; a freshly-staled one is not"* — is closed. The
    comparison runs again whenever the part on screen has changed: on a
    finished solve, on a change of selected part, and on every build, so a job
    is never verified against an outline the app has not just re-checked. The
    part's own bytes are the solve (`camDocument()` hands back the edited
    JSON), so a hash of them decides and the kernel section is only taken when
    they differ. The mismatch went into the job key as well — without it, a
    part edited under an unchanged outline produced an identical key and the
    warning kept the acknowledgement the *old* comparison had earned.
17. **Fixed.** "From model…" sets the blank's thickness from the part's own
    height (`CNCSection.suggestedStockThickness`) *and* work Z0 from the part's
    top (`zRange.last`), so the 6 mm stator plate modelled at z 11.1–17.1 comes
    in 6 mm thick with its top at Z0 rather than 10 mm thick at Z0 = 0.
    **Still open:** "Use model bounds" on the Stock panel is unchanged — it
    reads RealityKit `visualBounds`, not the section — and a DXF import still
    leaves the thickness alone, because a DXF has no height to read.
18. **Fixed.** The import sets the stock frame's origin from the outline in X
    and Y and from the section in Z, and `CNCPlacement` carries the shift and
    rotation into the request — moving the job *and* the part it is verified
    against together, so a placed job is still checked against the metal it
    really cuts.
19. **Fixed.** Holes between one and two cutter diameters are bored helically,
    one operation per diameter, so the pilots are machinable the moment a small
    enough cutter is fitted — and changing the cutter re-decides that without a
    re-import (item 49).

    The rest of it — one tool per job, no drill op, no tool change — is done
    too. `CNCWorkspace.tools` is a list of T1…Tn (diameter, flutes, flute
    length, stickout, centre-cutting), persisted per document, and it is the
    single source of truth: `toolDiameter` and its neighbours are now views
    onto the primary end mill, so the overlay, the named-field table and the
    envelope all read what the request is built from. Each operation names its
    tool; every tool reaches the request; the change is
    `tool_change: manual_pause_reprobe`, carrying the operator's own saved
    Z-probe macro when there is one and a touch-off instruction in words when
    there is not. Never `M6` (item 57).

    **A hole is drilled when a drill in the list is its size** — equality
    within 0.05 mm, not "small enough". A Ø2 drill leaves 0.25 mm a side of a
    Ø2.5 hole that nothing removes, so that hole stays unmachinable rather
    than being drilled undersize and called done. Derived, never stored, so
    fitting the drill re-plans on its own.

    `tool_sequence` and `verification_by_tool` are decoded. The readiness list,
    the Tool panel and the job outline say the sequence and "this job pauses N
    times for tool changes — re-zero Z after each", worded once in
    `CNCToolSequenceNote`; the G-code drawer marks the `M0` lines so the count
    is checkable against the program.

    The stator with T1 Ø3.175 + T2 Ø2.5 is three operations — the pilots
    drilled on T2 — one `M0`, and tool sequence **[2, 1]**. Why that order:
    the kernel sorts by phase (facing, inside features, the profile that frees
    the part), then keeps a tool's operations together *within* a phase, with
    a tool group running where its earliest operation asked to. The drill and
    the Ø27.6 opening are both inside features and the drill is first in the
    list (item 49), so T2's group leads; the profile is a phase later but
    still T1, and the assembler only writes a change when the tool number
    changes, so it joins T1's block without a second stop.
20. **Fixed.** Roughing/finishing split with stock-to-leave, a configurable
    number of finish stepdowns, an optional spring pass and separate finishing
    feed; ramp-along-contour or straight-down entry with a ramp angle, and
    tangential lead-in/out. The time estimate is the kernel's accel-aware one
    (`duration.accelAwareS`), not rapids at a flat 3000 mm/min.
21. **Fixed.** Tab handles are drawn on the contour (`cncTabHandle-…` in
    `syncCNCOverlay`) and can be dragged in the viewport; the same positions
    are typed as fractions in the inspector, and both go through
    `CNCWorkspace.moveTab`. Where a tab *landed* is read back from the job's
    own tab audit, because the kernel settles tabs onto straight stretches and
    the drawn contour and the cutter's offset loop are a rotation apart.
22. **Fixed (kernel).** A contour whose inward offset collapses because the
    cutter does not fit is an error, not a silently shorter path; the app
    offers "refuse" or "follow the centre line" with a stated wall tolerance,
    and the cutter-fit report gives the slot clearance per side. Caveat: the
    job's own `notes` come back decoded and are mined only for the spindle
    dial — the kernel's other warnings are on the wire and never shown.
23. **Fixed.** `CNCSetupSummary` shows operations, moves, time with
    acceleration and the deepest Z on every Setup panel, with the verification
    verdict under it.
24. **Fixed.** An import selects the first operation it created
    (`select(.operation(operations[0].id))`), which brings the inspector to it;
    a refusal and an outline mismatch are shown in the job outline rather than
    as a caption.
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
31. **Fixed.** Verifying the app blind was slow: the editor window is
    borderless, so it has no accessibility window, no Window-menu entry and no
    title to query. `kill -USR1 <pid>`, or Help ▸ Debug ▸ Dump Status, now
    writes one JSON document and prints its path to stderr: document and solve
    state, the workspace, the job (operations, blocked, blockers, warnings, the
    verification headline, whether there is any G-code, and why Run will not
    go), the machine (connection, the `$$` profile, homed, what changed since
    baseline, the alarm) and ncSender. `VCAD_STATUS` names the file; without it
    the name carries the pid, so two instances cannot overwrite each other's.
    It is a report, never a control surface — nothing in it moves a machine,
    changes a setting or starts a build — and the camera URL, which carries
    `user:pass@`, is never in it.

## Getting ready to cut (2026-09-17, second session)

Items marked **fixed** here are on `claude/unruffled-villani-c08436`. The job
was checked outside the app by a script that replays the G-code against the
outline (gouge, metal left, tabs, plunges); every defect below was found that
way, none by looking at the preview.

32. **Fixed. Inside contours cut on the wrong side of the line.**
    `Contour2D::inside()` was byte-identical to `outside()`: both offset the
    path by +tool radius. The stator's bore-and-slots pass ran at r 18.6–25.6
    about the part origin instead of r 15.4–22.4 — 3.2 mm into every post and
    the ring. It would have destroyed the part on the first pass. The tests
    that covered contours only asserted that moves came back. `inside` is now
    a field; the offset is signed; a test checks the side for both windings.
33. **Fixed. Holding tabs held nothing.** Three defects in one function:
    tabs were honoured on the final pass only, so with 0.5 mm stepdown the
    pass before it cut a 1 mm tab down to 0.5 mm; the width was measured along
    the tool-centre path, so a "4 mm" tab left 4 − 3.175 = 0.8 mm of metal;
    and a tab at position 0 was clipped at the loop seam (1.7 mm of lift, no
    metal at all). Net: two slivers of 0.6 × 0.5 mm. Tabs were also entered
    along a ramp from the previous vertex, which on a sparse polyline eats the
    tab. Now every pass below the tab top steps over it, vertically at both
    ends; width means metal left; the seam is handled; even spacing starts
    half a pitch in.
34. **Fixed.** Evenly spaced tabs land wherever the arithmetic puts them: one
    of the stator's three sat inside the 4 mm lead notch. A tab now settles on
    the nearest stretch that runs straight (chord/path ≥ 0.98, within half a
    tab pitch). Still open from item 21: the user cannot see or move them.
35. **Fixed.** Cut depth was copied from the stock thickness at import, so the
    natural order — import the outline, then enter 6 mm — left both contours
    at 10 mm and the job blocked with no hint why. A through cut now follows
    the thickness until its depth is edited by hand.
36. **Fixed (kernel).** An inward offset that falls into several pieces (the
    cutter does not fit through a neck) used to follow the first piece and say
    nothing; it is now an error. The stator's slot mouths are 3.87 mm: a
    Ø3.175 cutter passes with 0.35 mm a side, anything from Ø3.9 up does not.
    (Item 22's "5 mm slots" is the post width; the gap is what matters.)
37. **Fixed.** "From model…" sections the part on screen and machines *that*
    outline, so the outline no longer has to be regenerated outside vcad. It
    is offered first in the Import menu, and first for a reason: the outline
    that machines the part should come from the part, not from a file that may
    be a different revision. A solid torn at the section plane is refused with
    the gaps that caused it rather than healed into a guess (item 30's signal).
38. **Fixed.** The cutter-fit report names the corners the cutter cannot
    reach, their count, the total metal left there and how far it stands
    proud, per operation, in the inspector.
39. **Fixed.** Tabs are offered on inside cuts too, and pocketing with islands
    clears the waste while keeping declared material. The verification's
    "pieces that come free" check counts what comes loose and whether a skin
    holds it.
40. **Fixed.** `bottom_allowance` is signed: positive leaves a skin, negative
    breaks through — and a break-through is only offered when a spoilboard is
    declared, because on a bare bed the cut would be into the machine.
41. **Fixed.** The blank has an explicit margin round the part (following the
    cutter by default), work zero is a choice of part corner / stock corner /
    stock centre, and the panel states the blank's corner and the part's corner
    as distances from zero. The sweep rectangle — the outline plus a cutter
    radius, turned and placed with the job — is drawn on the stock.
42. **Fixed.** `spin_up_seconds` puts a `G4 P3000` dwell after `M3`, and one
    job is one program with one spindle start whatever the operation list holds
    (asserted in `CNCStudioTests`).
43. **Fixed.** Climb or conventional is a per-operation choice
    (`CNCCutDirection`), sent as the request's `direction`.
44. **Fixed. Run Job was the window's default button.** Its shortcut was
    ⌥⌘Return, and AppKit advertises any button whose key is Return as the
    window's default button, modifiers or not. A physical Return did not
    trigger it (checked: the modifiers are honoured, and committing a setup
    field un-confirms the setup), but an accessibility client asked to press
    "return" pressed Run Job — and the confirmation that follows had "Start
    machining" as its Return-default, as did Home, Move to, Set zero and Run
    macro. The shortcut is now ⌥⌘J, and every dialog that starts motion has
    Cancel as the default. Stop-and-reset keeps Return = stop.
45. **Fixed.** Import and Export job existed only inside a pull-down and a
    popover. Neither was in the menu bar (File ▸ Export offered STL/USDZ only),
    and the readiness popover's contents are not in the window's accessibility
    tree, so neither a keyboard user nor an assistive tool could reach "Export
    job…". There is now a **Manufacture** menu carrying every one of them, and
    the popovers and sheets it opens moved from the machine bar's `@State` onto
    the workspace so the menu and the button open the same one.

    The rule that matters more than the menu: **a menu item must never do what
    the button beside it refuses.** Every action is named once, in
    `CNCCommand`, with one `isEnabled` and one `run`; the buttons in the job
    outline, the inspector, the readiness list, the ncSender panel and the
    machine bar all ask it, so there is no second copy of the rule to fall out
    of step. `CNCIntegrationTests` pins the enabled state of all ten under an
    unbuilt, a passing and a refused job.

    The narrow remainder — *"the readiness checklist itself is only rendered
    in a popover"* — is closed. The Machine stage leads its inspector with
    `CNCReadinessList`: the same view the popover shows, reading the same
    `runBlocker` and the same `CNCCommand` predicates, so there is one gate and
    not a second copy of it. It is mounted outside the group the inspector
    disables while the machine streams, because that is precisely when the
    list has to stay readable.

    Shortcuts are ⌘ plus a second modifier, never a bare letter (which would
    fire while a number field has focus) and never Return (item 44). The table,
    and what each was checked against:

    | Manufacture | | Already taken |
    |---|---|---|
    | ⌥⌘O | Outline From Model | ⌘O Open… |
    | ⌥⌘I | Import Outline (DXF)… | ⇧⌘I Isolate |
    | ⌥⌘B | Generate / Rebuild Job | — |
    | ⌥⌘Y | Verify | — |
    | ⌃⌘E | Export Job… | ⌘E Export STL…, ⇧⌘E Export USDZ… |
    | ⌥⌘N | Send to ncSender | ⌘N New Document |
    | ⌥⌘G | Trace Bounds… | — |
    | ⌥⌘P | Probe… | — |
    | ⌥⌘K | Connect… / Disconnect | ⌘K Describe a Part… |
    | ⌥⌘J | Run Job… | (already the machine bar's, item 44) |

    Avoided as already used in-app: ⌃⌘1–3 (workspaces), ⌥⌘1 / ⌥⌘2 / ⌥⌘0
    (panels), ⌘0–⌘4 (camera), ⇧⌘0 (Frame Selection, moved there by item 55),
    ⌥⌘Z (zebra), ⌥⌘R (ray tracing), ⌥⌘↑ / ⌥⌘↓ (reorder operations), ⇧⌘D,
    ⇧⌘H, ⌥⇧⌘H, ⇧⌘A, ⇧⌘S, ⇧⌘Z. Avoided as system-reserved: ⌥⌘D (Dock), ⌥⌘H
    (Hide Others), ⌥⌘M (Minimize All), ⌥⌘T (Show Toolbar), ⌥⌘esc. A test
    asserts every command has a ⌘+modifier shortcut, that none is Return, and
    that no two collide — and `MenuShortcutTests` now does the same for the
    whole menu bar, which is where the one real collision was.
46. **Fixed.** `HostWindowHider.HiderView.viewDidMoveToWindow` sets the host
    window's `alphaValue` to zero, clears `isRestorable` and orders it out the
    instant the `WindowGroup` window attaches, and every further document opens
    its own *process* rather than another window in this one — so there is no
    blank window to see and no tab to accrue.
47. **Fixed.** Everything the CNC overlay draws hangs off a `cncRoot` placed at
    `cnc.origin` — where G54 sits in the model. X and Y come from the outline;
    Z comes from the section's own top, which is the line that was missing:
    without it the blank was the right thickness in the wrong place and the
    path was drawn 17 mm below the stator until "Place at model top" was
    pressed by hand. Items 17, 18 and 47 were all that one line.
48. **Fixed.** The readiness list says "Simulator connected · no machine" when
    it is the Simulator, and the machine bar and header say so too. The
    distinction now reaches the run gate as well: see item 56.
49. **Fixed.** Which holes the installed cutter can machine is derived, never
    stored, so changing the tool diameter re-decides it without a re-import and
    without losing the settings on the operations that survive. Operations are
    named from the geometry they came from ("Pilot Ø2.5 × 3", "Bore Ø27.6",
    "Opening 47.4 × 47.4", "Outside profile") and ordered so the small holes
    run before the profile that frees the part.
50. **Fixed.** "Leave a skin" / "break through" is a per-operation choice with
    a stated allowance, and the blank declares what is under it — machine bed
    or a spoilboard of a given thickness. On a bare bed "break through" is not
    offered at all, and an operation set to it is refused with the reason
    naming what is down there.
51. **Fixed.** Every number is now named in one table
    (`CNCWorkspace.fields`) with `fieldValue(_:)` and `setFieldValue(_:to:)`
    going through the same model the bindings do — including the invalidation
    that follows, because a path that changed a number without staling the job
    would be worse than no path. Four fields in the operation inspector had no
    identifier at all and fell back to their labels; they are named now, and
    `CNCFieldTests` parses the panels' own sources so a field on screen with no
    path through the table is a test failure.

    **The half the item actually asked for is now built.** Two routes, one
    model underneath — both through `fieldValue(_:)` / `setFieldValue(_:to:)`,
    so a value set from outside stales the job exactly as typing it does:

    - `CNCAccessibilityHost` is a real `NSView` in the Manufacture workspace
      whose accessibility children are one `CNCFieldElement` per named number,
      each with a role, label, identifier, value and a **setter**. SwiftUI's
      `.accessibilityValue` is read-only, which is why the fields on screen
      announced themselves and could not be written to.
      `isAccessibilitySelectorAllowed` reports read-only while the machine is
      streaming, so a client is *told* rather than ignored, and a field whose
      panel is not showing has no value rather than a plausible zero. Per-tool
      numbers (`cnc.tool.2.diameter`) are built from the list that is loaded,
      so a second cutter is reachable too; the plain `cnc.tool.*` names stay
      as aliases onto the primary end mill.
    - `VCAD_SET` names a file of `field=value` lines that `SIGUSR2` applies —
      the counterpart of item 31's `VCAD_STATUS`. Item 31 was careful that the
      dump is *a report, never a control surface*; this **is** one, so it is
      fenced: it exists only when the env var names a file, it writes only
      numbers already in the field table, and it refuses every one while the
      machine is streaming. It cannot build, run, connect, open or save.
      Everything it did and everything it refused is written back beside the
      request, because a writer that swallowed a refusal would turn it into a
      silent no-op — the exact failure the named-field work was for.

    A word handed to either route is refused rather than coerced:
    `NSString.doubleValue` reads "six" as 0, and a 0 mm stepdown arriving from
    a misheard dictation is what this app exists to refuse.

    What the tests drive, and what they cannot: `CNCAccessibilityTests` calls
    the `NSAccessibility` protocol methods on the mounted elements, which is
    the surface AppKit's accessibility server calls. The *client* side
    (`AXUIElementSetAttributeValue`) is deliberately not driven — it needs a
    running, front-most, accessibility-trusted app, so offscreen it would test
    the grant rather than the bridge. `VCAD_SET` is covered end to end, and is
    the route a script on this machine can use without a grant.
52. **Fixed.** "Apply these feeds to all operations" copies the cutting values
    — and only those; geometry stays put. "Recommend feeds" applies to every
    operation at once.
53. **Fixed.** "Trace bounds…" walks the job's bounding rectangle at a safe
    height with an optional dip at the corners, from the machine bar or
    Manufacture ▸ Trace Bounds (⌥⌘G). The sweep square, cutter radius
    included, is drawn on the stock, turned and shifted with the job; clamps
    are drawn with it and go red when the cutter would sweep through them.
    The two-point edge probe measures how far the blank is off the axes and
    one click turns the job to match (see the machine side of item 41).
54. First cut (2026-09-17): a 1 mm copper plate (teal-coated; taken for
    aluminium from the camera until the owner said otherwise) on a
    doubled-MDF riser, Ø2 2-flute, F250 / plunge F40 / 0.17 mm passes,
    0.15 mm onion skin plus three 4 × 0.42 mm tabs, sent from ncSender.
    14 min, cutter survived, profile clean. The first start cut air: the
    paper touch-off was 0.81 mm high. Copper-coloured slots and dust were
    first read as the skin breaking through to the MDF; with a copper plate
    that is just the cut metal, so whether the skin held is unverified.
    Nothing came loose. The app has no material setting at all — feeds were
    typed by hand for a material that turned out to be a different one.

    **Fixed (the material part).** The blank names what it is made of, from the
    kernel's table, and nothing is assumed: with no material chosen the app
    offers no feeds at all, because numbers for the wrong material are worse
    than none. "Recommend feeds" fills feed, plunge, stepdown, stepover and
    spindle from material × cutter × machine class, says which **dial** to set
    (the S word does nothing on this router), and has a "first cut on this
    machine: use 60 %" derate. Hand-typed numbers get a second opinion from the
    same table. Whether the skin held on that first cut is still unverified.

## Joining the four packages (2026-09-18, `cam/w3-app-integrate`)

Found by wiring the machine, sender, setup and job packages into one workspace
and then looking at it.

55. **Fixed. ⌥⌘0 was two different menu items.** View ▸ Show/Hide All Panels
    and Camera ▸ Frame Selection both claimed it (`Shell.swift`, the `.sidebar`
    and `Camera` command groups). AppKit shows both and only one fires.

    **Frame Selection moved to ⇧⌘0**, on two grounds: it is the less-used of
    the two — it needs a selection at all, and is disabled without one — and
    it has a near neighbour in ⌘0 Frame All, so ⇧⌘0 reads as "frame, but
    narrower" beside it. Show/Hide All Panels keeps ⌥⌘0, where it sits with
    ⌥⌘1 and ⌥⌘2, the other two panel keys. The table in item 45 records ⇧⌘0 as
    taken.

    `MenuShortcutTests` now scans every source file for literal
    `keyboardShortcut` calls and fails on any two that share a key and
    modifiers — the whole menu bar, not just `CNCCommand`, which is what let
    this one through. Mutation-checked: putting the collision back fails it.
    ⌘K is still registered twice — the menu item and the command bar's
    invisible accelerator — but both focus the same field, so it is
    duplication rather than a conflict; it is listed as a named exception in
    the test rather than left as a hole in it.
56. **Fixed. The Simulator refused every job.** Folding the machine's half of
    the gate into `runBlocker` made this visible: the envelope pre-check placed
    the job with the Simulator's work offset, which is whatever nobody set —
    zero — and Grbl's travel runs `-$13x…0`, so work zero sits at the *far
    corner* and any +X move is "outside travel". Every simulated job was
    refused, by a check about a table that does not exist. The finding is still
    reported in full, with its numbers; it is the *refusal* the Simulator does
    not earn (item 48's distinction, applied to the gate).

    It had been true before the merge too — the machine bar combined the two
    halves itself — but only the Run button could see it, so nothing said why.
57. **Fixed. The app could not import its own exported job.** Two reasons, both
    in vcad's own output: `M0` (the operator stop the job assembler puts
    between tools, because this machine has no changer) was not on the accepted
    M list, and the comment stripper `\([^)]*\)` stopped at the first `)` —
    so `(T1 Ø3.175 flat end mill (Ø3.17) at 10000 rpm)` left ` at 10000 rpm)`
    on the line and the parser called it unrecognised G-code. `G55`–`G59`,
    `G61`/`G64` are accepted now; `G53` *with motion* is still refused and says
    why. `M6` stays refused on purpose: a changer-less controller ignores it
    silently and carries on cutting with whatever is in the collet.
58. **Fixed. The camera tile's empty frame was a white rectangle.**
    `.fill(.quaternary)` resolved against nothing in particular — invisible
    over a light panel, solid white over a dark one, which reads as an
    overexposed live frame exactly where the picture of the cut goes. The
    status line under it also said "no frame" while the frame said "no frame";
    it now says what the tile is *doing* (not watching / waiting for the first
    frame / the frame's age).
59. **Fixed. The G-code drawer said "Generate the job or import a G-code
    file." for a refused job** — the one case where the absence of G-code is
    the whole point of the pipeline, read as "you have not pressed Generate
    yet". It now says the job was refused, lists the reasons, and says there is
    nothing to export or send.
60. **An alarm reported the wrong sentence.** With the controller in ALARM:1
    the gate said "Waiting for Idle and a known work position", which is true
    and useless beside "a limit switch was hit during motion … re-home before
    running". **Fixed:** the alarm's own text wins. Open: the ordering is still
    hand-written, and a new machine-side blocker could land behind a generic
    controller-state message the same way.
61. **The visionOS spike cannot build.** `VcadVision` symlinks `Editor.swift`
    into its target, and `EditorModel` holds a `CNCWorkspace` — but no `CNC*`
    file is symlinked in, and several now import AppKit. Nothing in this pass
    made it worse, and nothing here fixed it; it is worth knowing before
    anything is added to a shared file on the assumption that the Vision target
    still compiles.

    **Still open, and one symbol deeper.** `Shell.swift` is symlinked in too,
    and already called `VcadStatus` (in the unsymlinked `CNCStatusDump.swift`);
    the accessibility work added a call to `VcadFieldWriter` (in the
    unsymlinked `CNCAccessibility.swift`) beside it. Same class of breakage, one
    more instance of it — the target is already far from compiling, so this
    changes nothing in practice, but it is one more symbol to resolve whenever
    someone takes the spike seriously. The fix is a symlink set or a
    `#if os(macOS)` fence, not a rearrangement of `Shell.swift`.
62. **`swift test` runs the whole Manufacture suite against the real kernel.**
    Each built job is a second or two of FFI, so the app's test suite is ~55 s
    and a single job test is not cheap to iterate on. Worth a fixture cache if
    it grows much further. **Still open, and moving the wrong way:** the tool
    and drilling work took it to 183 tests and ~75 s.

## Multi-tool, drilling and the accessibility bridge (2026-09-18, `cam/w3-app-followups`)

63. **The full-retract peck is refused by our own verifier, and the refusal is
    right about the rule and wrong about the job.** `DrillCycle::Peck` rapids
    back *down* into the hole between pecks (`drill.rs`: `rapid(x, y,
    -previous + peck_clearance)`) — which is ordinary G83, and what every
    controller does. `verify2d::check_rapids` refuses any rapid that descends
    below the stock top over the stock, because it replays a *prismatic* job
    and has no way to know that the hole under the tool is already open. So a
    peck job is refused: `Rapid descends to Z-5.000, below the stock top`.

    Reproduced across all four cycles on the stator (Ø2.5 × 6 mm deep):
    `straight` and `chip_break` pass, `peck` is refused, `spot` correctly
    refuses a 6 mm hole as not a spot. The app therefore defaults to chip
    break — which is also the better cycle for that hole on a router, short
    retract, flutes stay engaged — still offers peck, and says in the drill
    panel what will happen *before* the build rather than after it. Pinned by
    `CNCToolTests.testTheFullRetractPeckIsRefusedByTheOracle`, so a kernel
    that learns to allow it, or an app that quietly stops offering it, shows
    up as a changed test.

    The fix belongs in the oracle, not here: it needs to know which material
    an earlier pass of the same program already removed. That is the same
    knowledge `vcad-kernel-stocksim` has, and is out of scope for an app
    package.

64. **The offscreen snapshot harness draws AppKit control indicators without
    their state — this is what "all three Work Zero radios looked selected"
    was.** `cacheDisplay(in:to:)` renders a `Picker(.radioGroup)`'s radios and
    a `Toggle`'s checkbox as a solid filled shape in dark mode and as nothing
    at all in light mode, whatever the binding says.

    The falsifier is in the same picture and is asserted rather than described
    (`CNCJobSnapshotTests.testReadinessSectionSnapshots`): "Cuts on its centre"
    is **on** and "I checked the tool, workholding…" is **off**, and the render
    draws them identically. No selection bug can do that. The model-side proof
    is `CNCFollowupTests.testWorkZeroIsExactlyOneChoice` — `zeroLocation` is a
    single enum, so exactly one of the three is ever the selection, and each
    of the three puts the blank's corner somewhere different.

    Worth knowing before reading any of these PNGs: **a control's *state* is
    not visible in them, only its layout and its text.** Custom rows are fine
    — the operation list's own selection dot renders correctly — it is
    specifically AppKit's own indicators that do not. Assert the state in the
    model; use the PNG for layout, wrapping and wording.

65. **A tool list makes three panels want to say the same sentence.** With two
    tools, "T2 Ø 2.5 → T1 Ø 3.175" and "this job pauses 1 time for tool
    changes — re-zero Z after each" appeared in the job outline's footer, the
    readiness list and the Tool panel at once — three copies on one screen,
    found by looking at the render rather than by a test. The footer keeps the
    count only now; the sentence stays where it is acted on. Worth watching as
    more per-job facts arrive: every panel has a good reason to want them.

66. **The per-tool field family weakened the registry's drift guard, slightly.**
    `CNCFieldTests` fails when a panel has a number the registry cannot reach.
    The Tool panel now addresses tools by number (`cnc.tool.2.diameter`), which
    is an interpolated identifier and so deliberately unmatched by the source
    scan, and the plain `cnc.tool.*` names became aliases onto the primary end
    mill rather than any panel's own. They are listed as aliases in the test,
    with `testTheToolFamilyAddressesEveryToolInTheList` guarding the family
    instead. That is a decision on the record, not a hole — but the guard is
    now two tests rather than one, and a third dynamic family would be worth
    handling properly rather than listing.

67. **The tool list is persisted in `UserDefaults`, keyed by the document's
    path.** It follows the macros precedent, and it is honest about what it
    is, but it means a tool list does not travel with a `.vcad` file, does not
    survive a move or a rename, and is not in the document a colleague opens.
    The job's own tools are arguably part of the job. Putting them in the
    document needs a schema decision nobody has made yet.

## Integrated app on the stator (2026-09-18, integrator's live look)

68. **A job the tool gate refused said "import an outline" with the outline on
    screen.** The verification list is driven by `policy.verified`, and a job
    turned back by the flute-length check never reaches the replay, so
    `verified` is false and `unverifiedReason` explained it as "there is no
    part outline to replay this job against" — with `stator-outline.dxf · 4
    holes` two panels to the left. Fixed: with an outline present the reason
    says the job was refused before it could be replayed. The rule underneath
    stands: the reason for an unverified job has to come from why it was not
    verified, not from a boolean.

69. **Stock thickness defaults to 10 mm on a DXF import, and the first build
    is refused for it.** A DXF carries no thickness; the stator is 6 mm and
    its solid was open in the same window. With 10 mm the Ø3.175 tool's 9.52
    mm flutes refuse both contours (correctly — that gate is doing its job),
    and the operator's first sight of the verify gate is a refusal about a
    number they never entered. The solid's Z extent is on hand when a part is
    open; the thickness should come from it, with the DXF override kept.

70. **Background accessibility cannot type into a SwiftUI number field by
    position.** Setting the thickness from outside took: find the field by
    role (`AXTextField`; every title is withheld to a background caller, so
    the `cnc.*` names are not searchable that way), click it as a raw event,
    then write to the focused element. Straight `app_type` at the field's
    coordinate is refused. The `VCAD_SET` path (item 51) is the honest
    answer for automation; the AX bridge is for assistive clients in front.
    Same live look, for the record: after the thickness, the stator built to
    5530 moves / 18:13 / Z −6.000 — identical to the headless dump — and was
    replayed clean with three warnings (bore slug free, stickout undeclared
    ×2) holding Run Job until acknowledged.
