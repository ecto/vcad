/**
 * MacroStore — durable per-user backing for the loon macro library.
 *
 * Mirrors the SessionStore seam (session-store.ts): a Supabase-backed
 * implementation over PostgREST with service-role auth and hard
 * `user_id=eq.<caller>` scoping, selected when the env + a signed-in user
 * are present; null otherwise (the warm registry + local files in
 * loon-macros.ts remain the fallback). All operations are best-effort and
 * fail soft — a Supabase hiccup never breaks define/call, it only reduces
 * durability, and the warm registry stays the source of truth for the
 * instance's lifetime.
 */

import type { AuthUser } from "./oauth.js";
import type { LoonMacro } from "./tools/loon-macros.js";

/** Durable macro storage, scoped to one verified user. */
export interface MacroStore {
  /** Load one macro by name; null on miss. */
  load(name: string): Promise<LoonMacro | null>;
  /** List the user's whole library. */
  list(): Promise<LoonMacro[]>;
  /** Upsert a macro (keyed user_id+name server-side). */
  save(macro: LoonMacro): Promise<void>;
}

interface MacroRow {
  name: string;
  version: number;
  description: string;
  params: LoonMacro["params"];
  source: string;
}

const rowToMacro = (r: MacroRow): LoonMacro => ({
  name: r.name,
  version: r.version ?? 1,
  description: r.description ?? "",
  params: Array.isArray(r.params) ? r.params : [],
  source: r.source,
});

/** Test seam: swappable fetch, mirroring session-store's sessionFetch. */
export let macroFetch: typeof fetch = (...args) => fetch(...args);
export function setMacroFetchForTest(f: typeof fetch): void {
  macroFetch = f;
}

class SupabaseMacroStore implements MacroStore {
  constructor(
    private supabaseUrl: string,
    private serviceRoleKey: string,
    private userId: string,
  ) {}

  private url(query: string): string {
    const uid = encodeURIComponent(this.userId);
    return `${this.supabaseUrl}/rest/v1/mcp_macros?user_id=eq.${uid}${query}`;
  }

  private headers(extra: Record<string, string> = {}): Record<string, string> {
    return {
      apikey: this.serviceRoleKey,
      Authorization: `Bearer ${this.serviceRoleKey}`,
      "Content-Type": "application/json",
      ...extra,
    };
  }

  async load(name: string): Promise<LoonMacro | null> {
    try {
      const res = await macroFetch(
        this.url(
          `&name=eq.${encodeURIComponent(name)}&select=name,version,description,params,source&limit=1`,
        ),
        {
          method: "GET",
          headers: this.headers({ Accept: "application/vnd.pgrst.object+json" }),
        },
      );
      if (!res.ok) return null; // 406 = zero rows → miss
      return rowToMacro((await res.json()) as MacroRow);
    } catch (err) {
      console.error("[macro-store] load failed:", err);
      return null;
    }
  }

  async list(): Promise<LoonMacro[]> {
    try {
      const res = await macroFetch(
        this.url("&select=name,version,description,params,source&order=name.asc"),
        { method: "GET", headers: this.headers() },
      );
      if (!res.ok) return [];
      return ((await res.json()) as MacroRow[]).map(rowToMacro);
    } catch (err) {
      console.error("[macro-store] list failed:", err);
      return [];
    }
  }

  async save(m: LoonMacro): Promise<void> {
    try {
      const body = [
        {
          user_id: this.userId, // always the verified caller — never tool input
          name: m.name,
          version: m.version,
          description: m.description,
          params: m.params,
          source: m.source,
          updated_at: new Date().toISOString(),
        },
      ];
      const res = await macroFetch(
        `${this.supabaseUrl}/rest/v1/mcp_macros?on_conflict=user_id,name`,
        {
          method: "POST",
          headers: this.headers({
            Prefer: "resolution=merge-duplicates,return=minimal",
          }),
          body: JSON.stringify(body),
        },
      );
      if (!res.ok) {
        console.error(
          "[macro-store] save failed:",
          res.status,
          await res.text().catch(() => ""),
        );
      }
    } catch (err) {
      console.error("[macro-store] save failed:", err);
    }
  }
}

/**
 * Store factory, mirroring createSessionStore: Supabase-backed when the env
 * and a signed-in user are present, else null (warm registry + local files
 * only). Anonymous callers get no cloud library — a macro library is an
 * identity-scoped asset, unlike capability-keyed sessions.
 */
export function createMacroStore(user: AuthUser | null): MacroStore | null {
  const url = process.env.SUPABASE_URL;
  const key = process.env.SUPABASE_SERVICE_ROLE_KEY;
  if (!url || !key || !user) return null;
  return new SupabaseMacroStore(url, key, user.sub);
}
