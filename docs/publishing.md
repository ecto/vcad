# Publishing vcad

The crates.io idiom: **public crates depend on each other by version**, git
dependencies exist only for crates that are not published yet, and per-developer
overrides live in an untracked `.cargo/config.toml` — never in a committed
manifest.

## Version plan: 0.10.0

crates.io has `vcad` 0.1.0 and nothing else. The plan publishes the whole kernel
surface at **0.10.0**, in step with `[workspace.package] version` (0.9.4 today).

**The version is not bumped in this branch, on purpose.** vcad's release
convention is a release commit — `chore: release v0.9.4` — whose numbers come
from the newest entry under `changelog/entries/` and are pushed into every
manifest by `scripts/sync-version.mjs`. The manifests say "don't hand-edit
them". So the release runs:

```sh
# add changelog/entries/<date>-<slug>.json with "version": "0.10.0"
npm run version:sync
git commit -am 'chore: release v0.10.0'
git tag v0.10.0
```

Everything in the publish set then carries 0.10.0: `vcad`, `vcad-kernel`, every
`vcad-kernel-*` in the list below, `vcad-ir`, `vcad-eval`, `vcad-render`, and the
ECAD and support crates they pull in.

## What kosm needs, and what changed

kosm consumes `vcad`, `vcad-kernel`, `vcad-eval`, `vcad-ir`, `vcad-loon`,
`vcad-kernel-export`, `vcad-render` and `vcad-kernel-{acoustics,optics,raytrace,
math,primitives,gpu}`. Taking the dependency closure of that set gives 56 crates.
Ten of them were `publish = false`; nine are now publishable —
`vcad-eval`, `vcad-render`, `vcad-gdsii`, `vcad-kernel-cost`, `vcad-kernel-dfm`,
`vcad-ecad-{package,pcb,schematic,sim}`. Each closure crate gained `readme`,
`keywords`, `categories` and an `include` list; `vcad-render` was also pinned to
a stale `version = "0.9.3"` and now follows the workspace.

`vcad-loon` is the exception. It depends on `loon-lang`, which is a git rev in a
repo that has never published, so it **stays `publish = false`** and kosm keeps
pulling it by git until loon ships. `vcad-eval`'s dev-dependency on it was made
path-only so `cargo publish -p vcad-eval` does not go looking for it on the
index.

## Sibling dependencies

| dep | today | becomes |
| --- | --- | --- |
| `kosm-render` | git tag `kosm-render-v0.2.0` + `version = "0.2"` | `{ version = "0.2" }`, drop `git`/`tag` |
| `phyz*` | git rev + `version = "0.3"` | `{ version = "0.4" }`, drop `git`/`rev` |
| `tang`, `tang-la`, `tang-expr` | registry `0.2` / `0.1` | unchanged |
| `loon-lang` | git rev | unchanged until loon publishes |

Each is annotated in the root manifest with the `-> <crate> <version>` it turns
into.

### The committed `[patch]`s are gone

`[patch.crates-io]` pointed `tang`, `tang-la` and `tang-expr` at `../tang`, and
`[patch."https://github.com/ecto/loon"]` pointed `loon-lang` at `../loon`. A
clone without those sibling directories could not resolve. Both are removed;
`.cargo/config.toml.example` carries the same paths for whoever wants them, and
`.cargo/config.toml` is gitignored. Only `clipper-sys` stays patched — it points
inside `third_party/`, so it works on a bare clone.

One live consequence: the published `tang-la 0.1.0` and `tang-expr 0.1.0` depend
on `tang 0.1.0`, so a bare resolve now pulls two tangs and `ExprId: Scalar` stops
holding. Use the example config until tang publishes 0.2.1 with tang-la 0.1.1 and
tang-expr 0.1.1 (which depend on tang 0.2) — at which point vcad's existing
`"0.1"` requirements resolve to one tang with no manifest change.

## Publish order

Within vcad, dependency order — leaves first:

```
vcad-kernel-math -> vcad-kernel-topo -> vcad-kernel-geom
  -> vcad-kernel-{naming,primitives,nurbs,sketch,tessellate}
  -> vcad-kernel-{booleans,fillet,sweep,shell,sheet,text,step,export}
  -> vcad-kernel -> vcad-ir
  -> vcad-kernel-{diff,adjoint,constraints,drafting,calibration,tolerance}
  -> vcad-kernel-{em,optics,acoustics,antenna,neutronics,particle,qcd,
                  magnetostatic,photonics,orbit,thermal,flow,fea,atoms,
                  enclosure,cam,stocksim,topopt,assembly,urdf,cost,dfm}
  -> vcad-kernel-gpu -> vcad-kernel-raytrace
  -> vcad-parts, vcad-receipt, vcad-gdsii, stepperoni, vcad-tool-derive
  -> vcad-ecad-{package,schematic,pcb,sim}
  -> vcad-eval -> vcad-render -> vcad
```

`cargo publish` in that order; the exact list is the 56-crate closure minus
`vcad-loon`. Let the index settle between uploads.

## Position in the stack

tang → phyz → kosm-render → **vcad** → kosm. vcad is last of the three upstream
repos because `vcad-kernel-raytrace` and `vcad-kernel-gpu` consume kosm-render.
