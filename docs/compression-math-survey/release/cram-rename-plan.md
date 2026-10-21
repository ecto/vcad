# Rename plan: tang-compress → cram (2026-10-21)

Brand **cram**. Homepage https://cram.sh (bought 2026-10-21); API docs on docs.rs/cram-codec. Crate **`cram-codec`** (Rust import `cram_codec`). The plain `cram` crate is taken.

## Name checks
- **`cram` crate:** v0.1.2, last updated 2019-06-20, 4.8k downloads. Description: "Cram realy awesome middleware". Owner: Joaquim Oliveira (github.com/JoaquimOliveira34); repo gitlab.com/ram-project/ram.
  - It looks abandoned, so a transfer is plausible. Ask the owner by email or a GitHub issue. If they don't answer, crates.io's abandoned-crate policy is the fallback (crates.io staff decide, and it takes weeks).
  - Don't block the launch on it.
- **`cram-codec`** and **`cramcodec`**: free.
- **Domains** (no NS records, so most likely unregistered; confirm at checkout):
  - cram.so, cram.sh, cram.toys: unregistered.
  - cram.dev: taken (Porkbun).
- **Collision warning:** CRAM is also the long-established genomics format (samtools/htslib `.cram`, Rust `noodles-cram`). A search for "cram compression" will turn up genomics first. That's fine for a brand, but the docs should say "cram, the codec" and never use the `.cram` file extension.

## Domain recommendation
**cram.sh.** It reads like a shell/CLI tool, it's short, and it suits a dev-facing launch. cram.so is the backup, and the pair is cheap to hold.

## Rename scope (tang repo, origin/main: 152 references)
| Item | Change |
|---|---|
| `crates/tang-compress` → `crates/cram-codec` | package name `cram-codec`, docs.rs URL, README |
| `crates/tang-compress-wasm` → `crates/cram-codec-wasm` | package name; the npm/wasm package name follows (`cram-codec-wasm`) |
| Root `Cargo.toml` | members, the workspace dep `cram-codec = { version = "0.1", path = ... }`, publish-order comment |
| Downstream crates | tang-compute (Cargo.toml + 4 `tang_compress::` uses), tang-llm (Cargo.toml + kvcomp.rs). Use `cram-codec`, or alias `tang_compress = { package = "cram-codec" }` to keep the diff small (prefer the real rename) |
| tang-expr README | 2 prose mentions ("First user: cram") |
| `scripts/publish.sh`, `docs/publishing.md` | step 3 becomes `cram-codec` |
| `bench/*.py` (4) | labels/paths only |
| `docs/compression-math-survey` in tang (4 files) | prose |

**Keep the stream magics (`TCG1`, `TCG3`, `TCS1`, `TKV1`).** They are wire-format identifiers, and decoders already exist in the wild (WASM, the vcad viewer). Note in the docs that "TC" is historical. Renaming them would break every stream already written, for no gain.

The tag `tc-auto` in reports and figures becomes `cram`. Keep `tc-gpu` and the other variant names as internal mode names.

## vcad repo (this worktree)
- `docs/compression-math-survey/*`: 38 files mention tang-compress. Replace the name in prose; report filenames stay as they are (they're history).
- **Paper** (`paper-draft.{md,typ}`): title, abstract, every "tang-compress" → "cram". Add one footnote: "formerly tang-compress; built on the tang IR". Rebuild the PDF on mew.
- **Launch figures:** change the legend label `tc-auto` → `cram` in `make_figures.py`, then regenerate the PNG and SVG files on mew. `provenance.csv` gets a `label` column; the raw codec ids are left alone.
- **Release kit**, crates-io-readiness: rename.
- vcad viewer (#937): it imports the wasm package, so bump the dependency name once `cram-codec-wasm` exists.

## Order
1. One tang PR: the crate/dir rename, downstream deps, publish script (mechanical; CI must stay green, `cargo publish --dry-run -p cram-codec`).
2. One vcad docs PR: prose, paper, figures.
3. After Cam's go: publish tang-expr 0.2.0 → tang-compile 0.1.0 → cram-codec 0.1.0.
4. Demo URL on cram.sh once Cam buys it.

Effort: about half a day of mechanical work plus a figure/PDF rebuild on mew. No code-behavior change.
