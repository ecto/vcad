/**
 * Durable backing store for Tasks-extension records (protocol-2026.ts).
 *
 * WHY: task records live in a process-local Map with a 30-minute TTL, so a
 * restart (routine on serverless) makes every outstanding taskId report
 * "Unknown taskId" — including tasks that FINISHED before the restart, whose
 * results are perfectly serializable. A half-executed background tool cannot
 * resume after a restart (the bridge and its in-flight rpc die with the
 * process), so this store does not attempt resumable execution. It persists
 * TERMINAL records only (completed / failed / cancelled): `tasks/get` hydrates
 * from here on a cache miss before reporting unknown, so a client that polls
 * after a restart still gets its result.
 *
 * Mirrors session-store.ts exactly: file store under the local session dir,
 * Supabase (`mcp_tasks`, migration 037) when the service-role env is present,
 * in-memory no-op otherwise. Best-effort throughout — a store failure never
 * turns a finished task into an error.
 */
import fs from "node:fs/promises";
import nodePath from "node:path";
import { sessionDir, sessionFetch, useFileSessionStore } from "./session-store.js";

/** The serializable subset of a task record — terminal states only. */
export interface StoredTask {
  taskId: string;
  status: "completed" | "failed" | "cancelled";
  statusMessage?: string;
  createdAt: string;
  lastUpdatedAt: string;
  /** Epoch ms; a load past this is treated as a miss (TTL still prunes). */
  expiresAt: number;
  toolName: string;
  result?: Record<string, unknown>;
  error?: { code: number; message: string; data?: unknown };
}

export interface TaskStore {
  /** Fetch a terminal task record, or null on miss/expiry. Never throws. */
  load(taskId: string): Promise<StoredTask | null>;
  /** Persist a terminal record. Best-effort: errors are logged, never thrown. */
  save(task: StoredTask): Promise<void>;
}

/** No-op store: today's behavior — nothing survives the process. */
export class InMemoryTaskStore implements TaskStore {
  async load(): Promise<StoredTask | null> {
    return null;
  }
  async save(): Promise<void> {
    /* nothing durable */
  }
}

function isStoredTask(v: unknown): v is StoredTask {
  const t = v as StoredTask | null;
  return (
    !!t &&
    typeof t.taskId === "string" &&
    (t.status === "completed" || t.status === "failed" || t.status === "cancelled") &&
    typeof t.expiresAt === "number"
  );
}

/**
 * Disk-backed store for local runs. One JSON file per task under a `tasks/`
 * subdirectory of the session dir, so `VCAD_MCP_SESSION_DIR` relocates both.
 * Same write-then-rename and never-escape-the-dir discipline as
 * FileSessionStore.
 */
export class FileTaskStore implements TaskStore {
  constructor(private dir: string) {}

  private pathFor(taskId: string): string | null {
    if (!/^[A-Za-z0-9_-]{1,128}$/.test(taskId)) return null;
    return nodePath.join(this.dir, `${taskId}.json`);
  }

  async load(taskId: string): Promise<StoredTask | null> {
    const file = this.pathFor(taskId);
    if (!file) return null;
    try {
      const raw = await fs.readFile(file, "utf8");
      const task = JSON.parse(raw) as unknown;
      if (!isStoredTask(task)) return null;
      if (task.expiresAt <= Date.now()) {
        await fs.rm(file, { force: true }).catch(() => {});
        return null;
      }
      return task;
    } catch (err) {
      if ((err as NodeJS.ErrnoException)?.code !== "ENOENT") {
        console.error("[task-store] file load failed:", err);
      }
      return null;
    }
  }

  async save(task: StoredTask): Promise<void> {
    const file = this.pathFor(task.taskId);
    if (!file) return;
    try {
      await fs.mkdir(this.dir, { recursive: true });
      const tmp = `${file}.${process.pid}.tmp`;
      await fs.writeFile(tmp, JSON.stringify(task), "utf8");
      await fs.rename(tmp, file);
    } catch (err) {
      console.error("[task-store] file save failed:", err);
    }
  }
}

/** Where local task records persist: `<sessionDir>/tasks`. */
export function taskDir(): string {
  return nodePath.join(sessionDir(), "tasks");
}

/**
 * Cloud-backed store over the `mcp_tasks` table (service role only, migration
 * 037). Capability-keyed by the unguessable taskId alone — tasks have no user
 * scoping, matching the anonymous session store's model.
 */
export class SupabaseTaskStore implements TaskStore {
  constructor(private cfg: { supabaseUrl: string; serviceRoleKey: string }) {}

  private headers(extra: Record<string, string> = {}): Record<string, string> {
    return {
      apikey: this.cfg.serviceRoleKey,
      Authorization: `Bearer ${this.cfg.serviceRoleKey}`,
      "Content-Type": "application/json",
      ...extra,
    };
  }

  private url(query = ""): string {
    return `${this.cfg.supabaseUrl}/rest/v1/mcp_tasks${query}`;
  }

  async load(taskId: string): Promise<StoredTask | null> {
    try {
      const res = await sessionFetch(
        this.url(`?task_id=eq.${encodeURIComponent(taskId)}&select=record&limit=1`),
        {
          method: "GET",
          headers: this.headers({ Accept: "application/vnd.pgrst.object+json" }),
        },
      );
      if (!res.ok) return null; // 406 = zero rows → miss
      const row = (await res.json()) as { record?: unknown };
      const task = row?.record;
      if (!isStoredTask(task) || task.expiresAt <= Date.now()) return null;
      return task;
    } catch (err) {
      console.error("[task-store] load failed:", err);
      return null;
    }
  }

  async save(task: StoredTask): Promise<void> {
    try {
      const res = await sessionFetch(this.url("?on_conflict=task_id"), {
        method: "POST",
        headers: this.headers({
          Prefer: "resolution=merge-duplicates,return=minimal",
        }),
        body: JSON.stringify([
          {
            task_id: task.taskId,
            record: task,
            expires_at: new Date(task.expiresAt).toISOString(),
          },
        ]),
      });
      if (!res.ok) {
        console.error(
          "[task-store] save failed:",
          res.status,
          await res.text().catch(() => ""),
        );
      }
    } catch (err) {
      console.error("[task-store] save failed:", err);
    }
  }
}

/**
 * Choose the store impl from env, mirroring `createSessionStore`'s gating:
 * Supabase when the service-role env is present, disk for local runs, else the
 * in-memory no-op. Constructed per use — stores hold no connection state.
 */
export function createTaskStore(): TaskStore {
  const url = (process.env.SUPABASE_URL || "").replace(/\/+$/, "");
  const key = process.env.SUPABASE_SERVICE_ROLE_KEY || "";
  if (url && key) {
    return new SupabaseTaskStore({ supabaseUrl: url, serviceRoleKey: key });
  }
  if (useFileSessionStore()) return new FileTaskStore(taskDir());
  return new InMemoryTaskStore();
}
