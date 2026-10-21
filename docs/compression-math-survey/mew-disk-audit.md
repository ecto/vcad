# mew disk audit (2026-10-20, audit only, nothing deleted)

mew `/` is 3.6 TB, 97% full, 110 GB free. Sizes are from `du`. "Last write" is the directory's mtime.

## Compression family (mine): safe-to-delete list

| Path | Size | What it is | Status | Safe to delete? |
|---|---|---|---|---|
| `~/hutter/m1` | 35 GB | Hutter M1 proxy training data plus runs (PPMD outputs, proxy streams) | Hutter is paused | **Yes.** It can be regenerated from enwik9 with the M1 pipeline if Hutter resumes |
| `~/hutter/decomp` | 8.9 GB | Working dir of the running **archive9** decompression (record-reproduction verify) | Running, ~14 h left | **After the run finishes**, or now if we stop the run |
| `~/hutter/work` | 5.7 GB | Decoded weights plus symmetry analysis scratch | Done | Yes (the analysis is written up) |
| `~/hutter/p2` | 1.3 GB | Kernel-speedup builds and harness | Done (patch saved in the repo notes) | Yes. Keep `p2-kernels.patch` (tiny) |
| `~/hutter/enwik9`, `enwik9.zip` | 1.3 GB | Test data | Needed by the running decompression | Keep until the run finishes; then the zip at least |
| `~/hutter/src`, `src-sym`, `p1`, `m2`, `perf`, `logs` | ~1 GB | Record source, timing, logs | Small | Keep logs and the result. The rest is optional |
| docker images `hutter-fx2:20.04`, `hutter-perf` | 2.0 GB | Build images | Hutter paused | Yes |
| `~/tc-int` | 6.4 GB | My integration test clone (PR #73, merged) | Done | **Yes** |
| `~/tc3-cleanup` | 3.1 GB | Cleanup track clone (PR #94, merged) | Done | **Yes** |
| `~/tc3-fix` | 2.1 GB | KV-phase fix clone (merged in #73) | Done | **Yes** |
| `~/tc3-kv-phase` | 0.6 GB | KV phase 3 clone (merged in #73) | Done | **Yes** |
| `~/crates-prep` | 5.8 GB | crates.io dry-run clone (PR #99, open) | Until #99 merges | After #99 merges |
| `~/tc4-gate` | 2.0 GB | Portfolio third-set gate (running) | Active | No, not yet |
| `~/tc2-images` | 1.0 GB | Kodak / Cool-chic campaign plus evaluation | Active | No, not yet |
| `~/tc4` | 21 MB | Learned-speed track (PR #94, merged) | Done | Yes |

**Reclaimable now from my family: ~60 GB** (m1 35 + work 5.7 + tc-int 6.4 + tc3-cleanup 3.1 + tc3-fix 2.1 + tc3-kv-phase 0.6 + p2 1.3 + docker images 2.0 + tc4). **Later:** +8.9 GB when archive9 finishes or is stopped, +5.8 GB after #99 merges, and ~3 GB when the images and portfolio gates finish.

## Other owners (flag only; I didn't assess these)

Large Rust build dirs (regenerable with a rebuild). Last write in parentheses:
- `code/vcad/target` 49 GB (2026-04-15)
- `code/gaia/target` 31 GB (2026-03-13)
- `code/muni/bvr/firmware/target` 9 GB (2026-04-01)
- `code/lore/target` 6 GB (2026-03-16)
- `Developer/ipse/target` 5 GB (2026-09-10)
- `code/pond/target` 5 GB (2026-08-22)

The four from March–April alone are ~95 GB of stale build output. Owners to confirm.

For the chief of staff's list (owner per CoS): `.cache/tang/flash` 117 GB and `.cache/huggingface` 50 GB (zest: flash model files, HF caches), `models` 141 GB, `runs` 131 GB, `checkpoints` 43 GB, `lanes` 75 GB, `flash-codex` 73 GB, `data` 270 GB, docker 150 GB. These need their owners (zest, ipse) to mark superseded runs and checkpoints and duplicate model caches. HF cache duplicates of files also in `models/` are the usual easy win.

## zest's list (tang caches and models; from zest's read-only survey)

zest runs nothing on mew any more (its units are disabled, and the bench loop is on RunPod). Live: Cam's `~/Developer/tang-serve` serving `mlx-community/Qwen3-8B-4bit`. **Keep** that model and its KV.

| Path | Size | Why it's superseded |
|---|---|---|
| `~/.cache/tang/flash/39ddbfdb2a1d622f` | 82 GB | bf16 dense pack from Oct 4, superseded by the q4x pack |
| `~/models/qwen3.8-flash-next/UD-Q2_K_XL` (keep its `MTP/` 3.9 GB and `mmproj-F16.gguf` 0.9 GB) | ~74 GB | unsloth GGUF used only for the llama.cpp baseline |
| `~/flash-codex` | 73 GB | MTP-training experiments; results are recorded in tang's flash-mtp-results.md |
| `~/.cache/tang/flash-serve` | 5.1 GB | Oct 5 serve system-prompt snapshot |
| `~/.cache/tang/kv`: the two old-name Qwen3-8B duplicates (keep `mlx-community_Qwen3-8B-4bit-Bf16`) | ~1 GB | duplicates under the old naming |
| `~/code/zest` 0.3 GB, `~/code/tang-ir/target` 3.9 GB | ~4 GB | disabled node; old build |

**Strong candidates: ~239 GB.**

**Cam's call, not superseded:** `~/.cache/tang/flash/b091fed7bd1bb60c` (36 GB, q4x pack) and `~/models/qwen3.8-flash-next-ista/Q2_0` (62 GB). These are only needed if Flash-Next should still run on mew.

**Low-risk but check other users first:** HF cache `mlx Qwen3-32B-4bit` (18 GB) and `Qwen2.5-7B` (15 GB), used only for benches.

**Not zest's (still unassessed):** `~/lanes` 75 GB, `~/runs` 131 GB, `~/checkpoints` 43 GB (March). Waiting on mind/ipse.

## mind/ipse list (from the mind manager; full table in its side-session notes, mew-mind-superseded.md)

**Keep:** `~/broca/voice` (live voice-serve), `~/Developer/broca-build`, tang-serve, the Qwen3-8B HF cache, `data/` and `runs/hear-heads`, `runs/releases`.

**Superseded, high confidence: ~86 GB.** `~/checkpoints/stage1_gpu` 43 GB (Mar), `~/ipse-room` 33 GB (Aug), `runs/ribt-hid` 9.4 GB (its gate failed), and the `runs/voice-v1`, `v1.1*`, `v1.2-base/cold/warm` variants (~3.2 GB).

**Waiting on the voice owner: ~350 GB.**
- Re-downloadable datasets: `data/libritts_r` 106 GB, hifitts 42 GB, librivox, fineweb.
- Voice data and runs: `runs/` and `data/voices-deep` 91 GB, `data/voices` + `voices_raw` 60 GB, `runs/ribt-*` ~32 GB.
- Other: `~/wakeword-train` 25 GB, venvs 37 GB (rebuildable).

## Voice owner's list (voice/broca session)

**Keep:**
- `data/libritts_r` (base corpus)
- `data/voices` (has `afheart/`, the 135 h RIBT-2 corpus)
- `runs/ribt-utmos/ext` (judge models)
- `runs/ribt-afheart/eval/sets` (frozen eval text sets)
- `runs/ribt-nv` (unreviewed)
- `~/runs/releases/`
- venvs `voice-eval`, `kokoro` (6.1 GB, low-priority keep)
- whatever `~/broca/voice-serve.sh` and the 8091/8092 servers read

**Superseded: ~141 GB.** Results are in docs/voice-model.md §21–§29.6, and final checkpoints in `~/runs/releases/`.

| Path | Size |
|---|---|
| `runs/voices-deep` | 73 GB |
| `data/voices_raw` | 19 GB |
| `data/voices-deep` | 18 GB |
| `data/librivox` | 8.2 GB |
| `runs/ribt-bon` | 7.3 GB |
| venv `hear-nemo` | 6.6 GB |
| `runs/ribt-gap` | 5.0 GB |
| `runs/ribt-diffro` | 2.1 GB |
| `runs/ribt-phase` | 1.8 GB |

The voice owner didn't size the superseded parts of `runs/ribt-utmos` and `runs/ribt-afheart`, which are everything except the kept subpaths. Those need a careful subpath delete, so they're left out here.

**Owner unknown:** `data/hifitts` 42 GB, `data/fineweb-edu`, `~/wakeword-train` 25 GB, venv `voice-enh`.

## Combined total

| Tier | GB |
|---|---|
| Compression family (mine) | ~60 (+15 later) |
| Stale Mar–Apr build targets (other repos) | ~105 |
| zest, strong candidates | ~239 |
| zest, Flash-Next packs (Cam said "free the 98 GB" via CoS) | 98 |
| mind/ipse, high confidence | ~86 |
| voice, superseded, minus voices-deep (Cam: keep voices-deep): voices_raw, librivox, ribt-bon/gap/diffro/phase, venv hear-nemo | ~50 |
| Orphans (Cam): data/fineweb-edu 9.5 + venv voice-enh 8.8 | ~18 |
| **Total proposed** | **~656** |
| Kept by Cam's call: data/voices-deep + runs/voices-deep (91), wakeword-train (25), hifitts (42) | — |
| Still open: HF bench models (33), ribt-utmos/afheart non-kept parts | — |

## archive9 (pid 835322)

- **What it is:** the decompress-and-verify half of my Hutter record reproduction (fx2-cmix-transformer), started automatically by `~/hutter/after_compress.sh` when the 37.4 h compress finished at 2026-10-20 03:22Z. The compress produced 93,768,588 B for the payload. Its job is to confirm the round trip decodes to exactly enwik9.
- **Progress:** 59.9% after 21.0 h, **ETA ~14 h** (cmix's estimate).
- **Still needed?** Only for completeness. Hutter is paused, so it isn't on any critical path.
- **Effect on gates:** it's a single-threaded CPU job on core 6. The running gates stay off core 6 and are CPU-sparse or GPU-bound, so the effect is small. Still, it shares memory bandwidth and L3, and that can inflate CPU timing comparisons. The portfolio gate measures encode time, so it carries that caveat.
- **Options:** let it finish (~14 h; frees 8.9 GB after), or stop it now and delete `~/hutter/decomp`. That loses only the final byte-compare.
