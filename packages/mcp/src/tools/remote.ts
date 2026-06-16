/**
 * Remote-deployment mode for filesystem-touching tools.
 *
 * On a local stdio server, "write the export to ./part.stl" means the
 * user's own disk — useful. On a shared HTTP deployment (e.g. a
 * claude.ai connector), the process cwd is the *server's* disk: writes
 * are invisible to the caller and reads can't see the caller's files.
 *
 * The HTTP entry point sets VCAD_MCP_REMOTE=1; tools check this at call
 * time and switch to inline payloads (base64 out, base64 in) instead of
 * touching the filesystem.
 */

/** True when running as a shared/remote deployment (HTTP entry point). */
export function isRemoteDeployment(): boolean {
  return process.env.VCAD_MCP_REMOTE === "1";
}

/** Cap on raw bytes returned inline as base64 from export tools.
 *  Tool results land in the model's context — keep them bounded. */
export function maxInlineExportBytes(): number {
  const raw = process.env.MCP_MAX_INLINE_EXPORT_BYTES;
  const n = raw ? parseInt(raw, 10) : NaN;
  return Number.isFinite(n) && n > 0 ? n : 4 * 1024 * 1024;
}
