#!/usr/bin/env python3
"""Print the workspace crates `cargo install vcad-cli` needs, one per line.

Reads `cargo metadata` on stdin and walks vcad-cli's normal + build
dependencies, keeping workspace members only. crates-publish.yml feeds the
result to `cargo publish -p ...`, which orders and uploads them; publishing
the whole workspace instead would also try crates whose deps are git-only
(vcad-sim, vcad-kernel-atoms -> phyz-gpu / phyz-md).

    cargo metadata --format-version 1 | python3 scripts/crates-closure.py
"""

import json
import sys

ROOT = "vcad-cli"

meta = json.load(sys.stdin)
packages = {p["id"]: p for p in meta["packages"]}
members = set(meta["workspace_members"])
nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}

root = next(i for i in members if packages[i]["name"] == ROOT)
seen, stack = set(), [root]
while stack:
    pkg = stack.pop()
    if pkg in seen:
        continue
    seen.add(pkg)
    for dep in nodes[pkg]["deps"]:
        if any(k["kind"] in (None, "build") for k in dep["dep_kinds"]):
            stack.append(dep["pkg"])

closure = sorted(packages[i]["name"] for i in seen if i in members)
unpublishable = [
    packages[i]["name"] for i in seen if i in members and packages[i]["publish"] == []
]
if unpublishable:
    sys.exit(f"publish = false in {ROOT}'s closure: {', '.join(sorted(unpublishable))}")
print("\n".join(closure))
