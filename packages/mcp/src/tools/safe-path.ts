/**
 * Safe path resolution for MCP tools that touch the filesystem.
 *
 * The MCP server runs with filesystem access, and in HTTP deployments it is
 * reachable from remote callers. Tool inputs must never produce paths outside
 * the server's working directory; otherwise a caller can read or overwrite
 * arbitrary files by passing `../../etc/passwd` or `/tmp/pwn`.
 */

import { isAbsolute, resolve, sep } from "node:path";

/**
 * Resolve `input` relative to `root` (defaults to process.cwd()) and reject
 * anything that escapes the root. Returns the absolute resolved path.
 *
 * Rejects:
 *  - absolute paths
 *  - paths containing `..` segments
 *  - paths that, after resolution, point outside `root`
 *  - NUL bytes
 */
export function resolveWithinRoot(input: string, root: string = process.cwd()): string {
  if (typeof input !== "string" || input.length === 0) {
    throw new Error("Invalid filename");
  }
  if (input.includes("\0")) {
    throw new Error("Invalid filename");
  }
  if (isAbsolute(input)) {
    throw new Error(
      "Invalid filename: absolute paths are not allowed — pass a filename relative to the " +
        "server working directory (set VCAD_MCP_EXPORT_DIR to choose the directory).",
    );
  }
  // Split on both separators so a Windows-style `..\\x` is caught on Linux too.
  const segments = input.split(/[\\/]/);
  if (segments.some((s) => s === "..")) {
    throw new Error(
      "Invalid filename: path traversal ('..') is not allowed — pass a filename relative to the " +
        "server working directory (set VCAD_MCP_EXPORT_DIR to choose the directory).",
    );
  }

  const resolved = resolve(root, input);
  const rootResolved = resolve(root);
  const rootWithSep = rootResolved.endsWith(sep) ? rootResolved : rootResolved + sep;
  if (resolved !== rootResolved && !resolved.startsWith(rootWithSep)) {
    throw new Error(
      "Invalid filename: path escapes the working directory — pass a filename relative to the " +
        "server working directory (set VCAD_MCP_EXPORT_DIR to choose the directory).",
    );
  }
  return resolved;
}
