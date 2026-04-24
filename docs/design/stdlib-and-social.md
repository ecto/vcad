# Parts library and social layer — design

Status: **Draft, pre-implementation.** Phase 0 deliverable. Awaiting sign-off
before any code.

## Goal

Close the gap where every complex shape (bolt, bearing, spoked wheel, saddle,
shelf bracket, parametric frame) has to be re-authored from primitives on
every use. Introduce parts as a first-class, parametric, editable-after-insertion
unit — built-in ones ship with the app, users can publish their own.

## Non-goals (this pass)

- Curator system, editorial tiers, legendary/featured grades — deferred.
  A simple popularity sort covers ranking needs on day one.
- Revenue / paid parts.
- Auto-thumbnail rendering. Hand-drawn SVG icons for the starter set.
- Bundling or redistributing third-party CAD geometry (McMaster, manufacturer
  libraries). We reference their part numbers; we never ship their files.
- "Import from URL" flows — deferred to a later phase.

## Core concept

Exactly one primitive: a **part**.

- parametric Loon function that returns geometry
- accompanied by a metadata file (name, category, params, icon, catalog xrefs)
- versioned and immutable once published
- inserted into a document as a `PartInstance` IR node, not baked

Two distribution channels share this one contract:

| Kind       | Implementation              | Lives in                            | Addressable as                  |
|------------|-----------------------------|-------------------------------------|---------------------------------|
| Built-in   | Rust fn returning `Document`| `crates/vcad-parts/` in the repo    | `std:<category>.<slug>@<ver>`   |
| User       | Loon source evaluated at use| Supabase (published by a user)      | `@<username>/<slug>@<ver>`      |

Both resolve to the same `PartInstance` IR node. The feature tree, chat tools,
and MCP server treat them identically. Only the resolver differs by path
prefix: `std:` dispatches to a compiled-in Rust function, `@user/` evaluates
Loon source fetched from Supabase.

### Why Rust for built-ins, Loon for user parts

Phase 1 built-ins ship as Rust fns because:

- no Loon extensions needed (full `std::f64` math, incl. trig, comes free)
- compile-time type safety on params
- faster eval, easier golden tests
- no user-code sandbox concerns for shipped parts

Phase 2 user-published parts run as Loon because:

- shippable as source without recompiling the kernel
- fork/remix flows stay simple (copy the source, edit, publish)
- sandboxable — Loon has no file or network access

The shared contract — `PartInstance` node, `part.toml` metadata, parameter
schema, `search_parts` / `place_part` tools — is identical across both
worlds. Users who want to contribute a part to the built-in stdlib submit a
PR with a Rust implementation; users who want to distribute a part without
a vcad release publish it as Loon through the social layer.

## Preflight verification

Before writing any part, confirm the following tools exist on `main`:

- `tube`, `polyline_tube`, `place`, `inspect_part` in
  `packages/core/src/commands/executors.ts`
- `rotate { pivot }` option in the same file
- `quad` option on `screenshot_viewport`
- All registered in `packages/core/src/commands/registry.ts` and documented in
  `packages/core/src/commands/prompt-appendix.ts`

The Phase-1 starter parts rely on them. If any are missing, stop and raise
before designing around them.

## Layer 1 — Parts as first-class IR

### File format

**Built-in parts** live under `crates/vcad-parts/src/<category>/<slug>.rs`.
Each file declares the part's metadata as a `const` and exports a `build`
function:

```rust
pub const METADATA: PartMetadata = PartMetadata {
    id:       "fastener.bolt.socket-head",
    name:     "Bolt (ISO 4762)",
    category: "Fasteners",
    params:   &[
        Param::enum_("size", &["M3","M4","M5","M6","M8","M10","M12"], "M6"),
        Param::length("length", 4.0, 200.0, 20.0, "mm"),
    ],
    xrefs:    &[
        Xref { params: &[("size","M6"),("length","20")],
               mcmaster: Some("91290A115"), iso: Some("ISO 4762"), din: Some("DIN 912") },
    ],
    thumb:    include_bytes!("bolt-socket-head.svg"),
    synonyms: &["SHCS","allen bolt","cap screw"],
    version:  "1.0",
};

pub fn build(p: &Params) -> Document { … }
```

The `parts-manifest.json` consumed by the palette and Cmd+K is generated
from these `METADATA` declarations at build time — no TOML files for
built-ins.

**User-published parts** (Phase 2) use a directory layout:

```
~/.vcad/drafts/fancy-bracket/
├── part.loon     ; [defn fancy-bracket [height width] …]
├── part.toml     ; id, category, params, xrefs, thumb path
└── thumb.svg     ; 48×48 palette icon
```

The TOML form lets users author parts without recompiling the kernel.

Example `part.toml`:

```toml
[part]
id       = "fastener.bolt.socket-head"
name     = "Bolt (ISO 4762)"
category = "Fasteners"
entry    = "bolt"                    # loon fn to invoke
thumb    = "thumb.svg"
synonyms = ["SHCS", "allen bolt", "cap screw"]

[[param]]
name    = "size"
type    = "enum"
values  = ["M3","M4","M5","M6","M8","M10","M12","M16","M20"]
default = "M6"

[[param]]
name    = "length"
type    = "length"
min     = 4
max     = 200
default = 20
unit    = "mm"

[[xref]]
params   = { size = "M6", length = 20 }
mcmaster = "91290A115"
iso      = "ISO 4762"
din      = "DIN 912"
```

### `PartInstance` IR node

Add to `crates/vcad-ir/` and mirror in `packages/ir/`:

```rust
PartInstance {
    path:    String,          // "std:fastener.bolt.socket-head" | "@cam/widget"
    version: String,          // semver
    params:  Map<String, Value>,
    name:    Option<String>,  // user-supplied label
}
```

Evaluation in `packages/engine/src/evaluate.ts` resolves by path prefix:

1. `std:` → call WASM-exported `vcad_parts_build(path, params_json)` →
   returns a sub-`Document`
2. `@user/` → fetch Loon source for the pinned version → evaluate via
   `vcad_loon::eval_vcad` with params injected into the environment →
   returns a sub-`Document`
3. wrap the returned subtree so selection and the feature tree treat the
   part as one node, not its expansion

Editing a param re-evaluates. The subtree never bakes into the parent doc.
Save/load serializes only `path + version + params + name` — geometry is
regenerated on load.

Feature tree shows one line per instance with inline param scrubbers, the
same affordance as a sketched extrude today.

### Loon capability audit (for Phase 2 user parts only)

Phase 1 built-ins are Rust so this audit is deferred to Phase 2. Current
findings from a read of `loon_lang::interp::builtins`:

- **present**: `+ - * / %`, comparison ops, `sqrt pow abs min max`,
  `map filter fold range zip flatten reverse`, `if` / `cond` / `match`
  (language keywords), closures via `fn`, let bindings, ADTs via `type`
- **missing**: `sin`, `cos`, `tan`, `atan2`, `PI`, `TAU` — required for
  parts with circular geometry (spoked-wheel, helical patterns, hex heads
  built from flats)
- **missing**: keyword args with defaults — callable parts pass params as
  a map, but `[defn bolt {:keys [size length :or {length 20}]}]` isn't
  supported today. Workaround for Phase 2: pass params as a single map
  arg, part extracts keys with `get` + defaults.

Phase 2 blockers: add trig + `PI`/`TAU` to loon builtins before opening
user-published parts that model circular geometry.

### Versioning

- **Built-in parts**: versioned in `part.toml` (`version = "1.0"`). Bumping it
  is a deliberate breaking-change signal. Docs pin the exact version they were
  created against.
- **User parts**: semver, immutable once published, pin-by-default. A doc
  stores `@cam/widget@1.2.0` verbatim. When `1.3.0` publishes, an "Updates
  available" chip appears in the feature tree. Never auto-applied.
- **No separate lockfile** — the doc is the lockfile. Every `PartInstance`
  carries its pinned version.
- **Unpublishing**: allowed for 24h after publish, then permanently locked.
  Enough to yank a leaked secret, not enough to rug-pull downstream docs.

### Tool surfaces

All entry points funnel through one engine function:

```
  palette click ─┐
  Cmd+K entry  ──┼─► insertPart(path, version, params, transform)
  MCP tool     ──┤         │
  chat tool    ──┘         ▼
                    PartInstance node inserted at cursor / given transform
```

**Palette**: new "Parts" tab in `packages/app/src/components/ToolPalette.tsx`.
Category dropdown, grid of thumbs, click opens a param sheet (reuses the
`InlineProperties` pattern). Param sheet confirms → `insertPart`.

**Cmd+K**: one generic entry per part in `CommandPalette.tsx`. Tokens indexed:
name, category, synonyms, every xref number from `part.toml`. Typing
`91290A115` matches the socket-head bolt with default M6×20 params.

**Chat + MCP**: two new tools registered in both
`packages/core/src/commands/executors.ts` (chat) and
`packages/mcp/src/server.ts` (external agents):

```
search_parts(query, category?, limit?)
  → [{ id, name, category, params, xref, matchReason }]

place_part(path, params, transform?, name?)
  → { instanceId, summary }
```

Agents discover with `search_parts`, place with `place_part`. Both tools wrap
the same `insertPart` core.

### McMaster xref

Not a geometry source. A catalog alias table. Each `[[xref]]` row maps a
param combo to real-world part numbers. Powers:

- search matches on part numbers (typing `91290A115` finds our bolt)
- future BOM export ("shopping list" from doc → McMaster cart)
- user trust signal ("this is the thing on McMaster")

Zero legal exposure — referencing a part number is what every engineer's
drawing does.

### Build pipeline

A `build.rs` in `vcad-parts` walks each `src/<category>/<slug>.rs`,
invokes a `fn manifest_entry() -> ManifestEntry` from each module, and
emits `parts-manifest.json` at compile time. The JSON is bundled into
the WASM kernel and exported via `get_parts_manifest() -> String`. The
app reads it on boot.

```
{
  "parts": [ { id, name, category, entry, params, thumb, xrefs, … }, … ],
  "searchIndex": [
    { partId, tokens: ["bolt", "M6", "91290A115", "ISO 4762", …],
      defaultParams: { … } },
    …
  ]
}
```

Palette consumes `parts`. Cmd+K consumes `searchIndex`. Single source of
truth, generated from the TOMLs.

### Golden tests

Per part, a `tests/golden.rs` with param fixtures asserting mesh hash:

```rust
#[test]
fn bolt_m6x20_mesh_hash() {
    let mesh = eval_part(
        "std:fastener.bolt.socket-head",
        "1.0",
        params! { size: "M6", length: 20 },
    );
    assert_eq!(sha256(mesh), "a4f3…");
}
```

Kernel regressions or Loon extensions that silently break a part fail CI.

### Phase-1 starter set (8 parts)

1. `fastener.bolt.socket-head` — M3–M12, lengths 4–100
2. `fastener.nut.hex` — M3–M12
3. `fastener.washer.flat` — M3–M12
4. `bearing.608`
5. `bearing.6000-series` — parametric bore diameter
6. `bike.spoked-wheel` — exercises `polyline_tube` + circular pattern
7. `enclosure.vented-box` — exercises shell + linear pattern
8. `generic.shelf-bracket` — exercises sketch + extrude + fillet

Bike frame, crank set, saddle, drop-bar deferred to Phase 1.5 after Loon
extensions prove out on the easier eight.

## Layer 2 — User parts (social)

### Data model

New migrations under `supabase/migrations/`:

```
profiles         (user_id PK, username UNIQUE, bio, avatar_url,
                  created_at)

parts            (id PK, owner_id FK, slug, category, current_version,
                  favorites_count, insertions_count, created_at,
                  UNIQUE(owner_id, slug))

part_versions    (id PK, part_id FK, version, loon_src, metadata_toml,
                  thumb_url, published_at,
                  unpublished_at nullable,
                  forked_from_part_version_id nullable)

part_favorites   (user_id, part_id, PRIMARY KEY(user_id, part_id))

follows          (follower_id, followee_id, PRIMARY KEY(…))
```

RLS sketch:

- `profiles` — public-read on profiles with ≥1 published part; owner-write
- `parts` — public-read if any version published; owner-write
- `part_versions` — public-read if `unpublished_at IS NULL`; insert-only for
  owner; **never UPDATE** (immutable)
- `part_favorites`, `follows` — users own their own rows

### Publishing flow

```
User drafts locally:
  ~/.vcad/drafts/fancy-bracket/{part.loon, part.toml, thumb.svg}
                    │
        "Publish" ──┤
                    ▼
  validate: schema passes, loon compiles, golden render succeeds
                    ▼
  POST /parts/publish → insert part_version row (immutable)
                    ▼
  now addressable as @username/fancy-bracket@0.1.0
```

### Discovery

`vcad.io/parts` with tabs: **Trending** · **Recently published** ·
**By category** · **By tag**. Server-side ranking function combines
unique-user favorites, unique-doc insertions, and recency.

Insertion counting: `place_part` increments `parts.insertions_count`
server-side when the resolved `path` begins with `@`. Dedupe by
`(part_id, doc_id)` so re-opening a doc doesn't re-count.

### Remix / fork

One click copies source and metadata into the viewer's namespace, adding
`forked-from = "@ecto/spoked-wheel@1.2"` to the new `part.toml`. Viewer
edits, publishes as `@viewer/spoked-wheel@0.1.0`. Attribution persists in
`part_versions.forked_from_part_version_id`.

### Favorites and follows

Star a part → appears in user's sidebar. Follow a user → their publishes
show up in the viewer's feed.

### Shared docs that reference user parts

The existing share-link flow lives in
`packages/app/src/lib/url-document.ts` and the `readOnlyShare` slice of
the UI store. When a viewer opens a shared doc containing
`@cam/widget@1.2.0` they've never seen:

1. Resolver fetches the pinned version from Supabase (public-read RLS)
2. Caches locally so offline reload works
3. Feature tree shows the part with `via @cam` attribution

Attribution is load-bearing — it's how the social layer closes the loop
from consumption back to discovery.

### Profile page

`vcad.io/@username`:

```
  cam pedersen                          [ Follow ]
  38 parts · 412 favorites

  ▸ Parts      Recent · Popular
  ▸ Docs       publicly shared
  ▸ Activity   feed of publishes and remixes
```

### Chat and MCP awareness

`search_parts` includes user parts in results, ranked by the same
popularity function used on `vcad.io/parts`. Agents can discover and
place user-published parts the same way they do built-in parts, subject
to RLS (drafts invisible, published public).

### Abuse mitigations

- **Unique-user / unique-doc counts everywhere.** Blocks sockpuppet
  favorite farms and script-inflated insertions.
- **Self-remix exclusion.** Forks owned by the same account as the parent
  don't count toward either's popularity.
- **Young-account damping.** Favorites from accounts less than 7 days old,
  or with fewer than 3 insertions themselves, soft-weighted to ~0.1 in
  ranking.
- **Rate limits on publish.** Per-user cap on publishes per hour to
  discourage spam.

## Phased implementation

**Phase 0 — Design doc.** This document. Lands at
`docs/design/stdlib-and-social.md`. **Stop for sign-off.**

**Phase 1 — Built-in parts + IR + tools.** New branch off `main`.

- Loon capability audit, extensions as needed
- `PartInstance` IR variant (Rust + TS mirror)
- Evaluator support in `packages/engine/src/evaluate.ts`
- `cad-lib/parts/` directory with 8 starter parts, each with golden test
- `parts-manifest.json` build step
- Palette "Parts" tab
- Cmd+K integration
- `search_parts` and `place_part` in chat (`executors.ts`) and MCP
  (`server.ts`)
- WASM binding updates in `packages/kernel-wasm/`
- Changelog entry under `feat`

No Supabase changes in Phase 1.

**Phase 2 — User parts.** New branch off `main`.

- Migrations under `supabase/migrations/` (include
  `supabase db push --dry-run` output in the PR description)
- Profiles, publishing, discovery, remix, favorites, follows
- Shared-doc auto-pin flow
- `search_parts` extended to return user parts with popularity ranking

**Phase 3 — Polish.** Auto-thumbnail rendering, McMaster BOM export,
import-from-URL, anything else that came up during Phases 1–2.

Each phase waits for sign-off before the next begins. No folding
phase-2 work into phase-1 branches.

## Checkpoints for sign-off

1. **`PartInstance` as IR node, not a Loon macro expansion.** Non-negotiable
   in this design — alternative breaks edit-after-insertion and shared-doc
   attribution.
2. **`part.toml` alongside `part.loon`.** TOML metadata stays readable by
   tools that don't run Loon. Keeps the palette simple.
3. **Phase-1 starter set (the 8 above).**
4. **Popularity ranking stays a single sort, no tiers.**
