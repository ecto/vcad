# vcad-diff — semantic diff & merge for `.vcad` documents

"Git for CAD." `.vcad` files are JSON documents holding a parametric feature
DAG; line-based `git diff` on them is noise (HashMap key order, one parameter
change rippling through formatting). This crate diffs and merges at the
**feature level**: entities are matched by stable id (node id, joint id,
material name, parameter name — never array position), and modifications are
reported as dotted field paths with old → new values.

## CLI

```bash
# Feature-level diff, human-readable
vcad diff a.vcad b.vcad

# Structured JSON diff (for tooling)
vcad diff a.vcad b.vcad --json

# Exit 1 when the documents differ (CI gates)
vcad diff a.vcad b.vcad --exit-code

# Three-way merge; writes merged doc, or prints conflicts and exits 1
vcad merge base.vcad ours.vcad theirs.vcad -o merged.vcad
```

Example output:

```
~ changed node 1 (plate)
    op.size.z: 8.0 → 12.0
+ added   node 2 (boss) [Cylinder]
+ added   root 2
```

## Merge semantics (fail-closed)

- Changes to **different entities** merge automatically.
- Changes to **different fields of the same entity** merge automatically
  (ours resized the cube, theirs renamed it → both survive).
- **Genuine conflicts** are reported explicitly and produce *no* output
  document — a side is never silently picked:
  - both sides set the same field to different values,
  - one side deleted an entity the other modified,
  - both sides added the same id with different content.

Identical edits on both sides are not conflicts.

## Library

```rust
use vcad_diff::{diff, merge, apply, MergeResult};

let d = diff(&old_doc, &new_doc)?;           // DocumentDiff, serializable
let patched = apply(&old_doc, &d)?;          // diff-then-apply round-trips
match merge(&base, &ours, &theirs)? {
    MergeResult::Merged(doc) => { /* clean */ }
    MergeResult::Conflicts(cs) => { /* fail-closed: resolve by hand */ }
}
```

## Git integration

Register `vcad` as the diff and merge driver for `*.vcad` files.

**1. `.gitattributes`** (commit this to the repo):

```gitattributes
*.vcad diff=vcad merge=vcad
```

**2. Diff driver** — `git diff`, `git log -p`, and `git show` render the
semantic diff instead of JSON text (config is per-clone; add to
`~/.gitconfig` with `--global` to enable everywhere):

```bash
git config diff.vcad.command 'vcad-git-diff'
```

where `vcad-git-diff` is a tiny wrapper on your `PATH` (git passes 7 args:
path, old-file, old-hex, old-mode, new-file, new-hex, new-mode):

```bash
#!/bin/sh
# vcad-git-diff — git external diff driver for .vcad
echo "vcad diff: $1"
vcad diff "$2" "$5"
```

**3. Merge driver** — `git merge` / `git rebase` auto-merge non-conflicting
CAD edits and stop (leaving the file conflicted) only on genuine feature
conflicts:

```bash
git config merge.vcad.name 'vcad semantic merge'
git config merge.vcad.driver 'vcad merge %O %A %B -o %A'
```

`%O`/`%A`/`%B` are git's base/ours/theirs temp files; writing the result to
`%A` and exiting 0 marks the file merged. On conflict `vcad merge` prints a
feature-level conflict report and exits 1, so git records the file as
conflicted — resolve by editing and `git add`, as usual.

**4. `git mergetool` (optional)** — to inspect conflicts interactively:

```bash
git config mergetool.vcad.cmd 'vcad merge "$BASE" "$LOCAL" "$REMOTE" -o "$MERGED"'
git config mergetool.vcad.trustExitCode true
```

## What is covered

Nodes (feature DAG), scene roots, materials, part-material assignments,
assembly part definitions / instances / joints, named parameters, expression
bindings, clearance specs, and document-level singletons (version, scene
settings, ground instance, schematic, PCB, molecule, timeline — each treated
as one entity; concurrent edits inside a singleton conflict at the field
level like any other entity).
