#!/usr/bin/env python3
"""Regenerate the Swift packages' CVcadFFI headers from the canonical one.

There are three copies of `vcad_ffi.h`: the canonical crate header, and one
inside each Swift package's `systemLibrary` target (SwiftPM requires the header
to live under the target's own path, so a symlink to the crate is not an
option).

Hand-syncing three copies of a C ABI is a miscompile waiting to happen: C does
no signature checking across the boundary, so a mirror that lags the Rust
declarations does not fail to link — it corrupts. Generating them makes the
drift impossible rather than merely discouraged.

Run from anywhere; each app's `build-ffi.sh` invokes it before every build.
"""

import pathlib
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent
CANON = REPO / "crates/vcad-ffi/include/vcad_ffi.h"
MIRRORS = [
    REPO / "apple/VcadApp/Sources/CVcadFFI/vcad_ffi.h",
    REPO / "apple/VcadVision/Sources/CVcadFFI/vcad_ffi.h",
]

# This banner must not contain a literal star-slash. An earlier version wrote
# the source path as a glob and the star-slash closed the comment block on line
# 2, so every mirror failed to parse with "unknown type name 'build'" — an
# error that points nowhere near its cause.
BANNER = """/* GENERATED FILE - DO NOT EDIT.
 *
 * Regenerated from crates/vcad-ffi/include/vcad_ffi.h by
 * scripts/sync-ffi-header.py, which each app's build-ffi.sh runs before
 * building. Edit the canonical header; this mirror is overwritten.
 */
"""

GUARD = "#ifndef VCAD_FFI_H"


def main() -> int:
    if not CANON.exists():
        print(f"canonical header missing: {CANON}", file=sys.stderr)
        return 1
    text = CANON.read_text()
    try:
        start = text.index(GUARD)
    except ValueError:
        print(f"{CANON} has no {GUARD} include guard", file=sys.stderr)
        return 1

    out = BANNER + text[start:]
    changed = []
    for path in MIRRORS:
        if not path.parent.exists():
            continue
        if not path.exists() or path.read_text() != out:
            path.write_text(out)
            changed.append(path)

    for path in changed:
        print(f"synced {path.relative_to(REPO)}")
    if not changed:
        print("C headers already in sync")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
