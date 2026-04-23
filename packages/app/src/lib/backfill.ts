import type { VcadFile, VcadFileCrdt, VcadFileLegacy } from "@vcad/core";
import { getAllDocuments, saveCompleteDocument } from "./storage";

type SyncTrigger = () => void | Promise<void>;

/**
 * One-shot migration that rewrites legacy `.vcad` rows in IndexedDB to the
 * canonical v0.4 CRDT format. Safe to run on every boot — already-migrated
 * rows are skipped by the tagged-union discriminator. Failures are logged
 * and the legacy blob is left in place so the user's data is never at risk.
 *
 * This backfill is the second half of the fix for the silent-data-loss
 * bug class: the live save/load path now writes CRDT directly, and this
 * job upgrades any docs authored before the refactor landed so their
 * next save is guaranteed-lossless too.
 */
export interface BackfillStats {
  scanned: number;
  migrated: number;
  skipped: number;
  failed: number;
}

/**
 * Minimal engine shape used by the backfill — we only need v1-load, save,
 * and free. Passed in so the backfill doesn't import `@vcad/kernel-wasm`
 * directly (that module is loaded on demand by the app boot).
 */
export interface BackfillEngineClass {
  from_v1_json(json: string): BackfillEngine;
  load(bytes: Uint8Array): BackfillEngine;
}

export interface BackfillEngine {
  save(): Uint8Array;
  free(): void;
}

export async function runIdbBackfill(
  EngineClass: BackfillEngineClass,
  /**
   * Optional sync trigger — called once after the backfill finishes with
   * at least one migration. The cloud backfill is deliberately piggy-backed
   * on the normal sync pipeline: migrated rows get `syncStatus: "pending"`
   * + a version bump, and the sync service uploads them using the same
   * CRDT-aware codec the live app uses. No separate cloud worker.
   */
  triggerSync?: SyncTrigger,
): Promise<BackfillStats> {
  const stats: BackfillStats = {
    scanned: 0,
    migrated: 0,
    skipped: 0,
    failed: 0,
  };

  let docs;
  try {
    docs = await getAllDocuments();
  } catch (e) {
    console.warn("[backfill] getAllDocuments failed, skipping IDB backfill:", e);
    return stats;
  }

  for (const stored of docs) {
    stats.scanned++;
    const migrated = await migrateOneIfNeeded(EngineClass, stored.document);
    if (!migrated) {
      stats.skipped++;
      continue;
    }
    if (migrated === "failed") {
      stats.failed++;
      continue;
    }
    try {
      // Version bump + syncStatus=pending so the existing sync worker picks
      // the migrated row up and uploads the CRDT payload to Supabase. That
      // path already discriminates via `vcadFileToCloudContent` so the new
      // format lands in `documents.content` cleanly.
      await saveCompleteDocument({
        ...stored,
        document: migrated,
        version: stored.version + 1,
        modifiedAt: Date.now(),
        syncStatus: stored.cloudId ? "pending" : "local",
      });
      stats.migrated++;
    } catch (e) {
      console.warn(`[backfill] failed to rewrite ${stored.id}:`, e);
      stats.failed++;
    }
  }

  if (stats.migrated > 0 || stats.failed > 0) {
    console.log(
      `[backfill] IDB: migrated=${stats.migrated} skipped=${stats.skipped} failed=${stats.failed} of ${stats.scanned}`,
    );
    if (triggerSync && stats.migrated > 0) {
      try {
        await triggerSync();
      } catch (e) {
        console.warn("[backfill] post-migration sync trigger failed:", e);
      }
    }
  }
  return stats;
}

/**
 * Attempt to convert a legacy VcadFile to CRDT. Returns:
 *  - `null` if already CRDT / loon / no work needed
 *  - `"failed"` if the migration or verification failed — caller should
 *    leave the legacy blob in place.
 *  - A new `VcadFileCrdt` variant on success.
 */
async function migrateOneIfNeeded(
  EngineClass: BackfillEngineClass,
  file: VcadFile,
): Promise<VcadFileCrdt | null | "failed"> {
  if (file.kind !== "legacy") return null;
  return migrateLegacyToCrdt(EngineClass, file);
}

export function migrateLegacyToCrdt(
  EngineClass: BackfillEngineClass,
  file: VcadFileLegacy,
): VcadFileCrdt | "failed" {
  let engine: BackfillEngine | null = null;
  try {
    const irJson = JSON.stringify({
      document: file.document,
      parts: file.parts,
      consumedParts: file.consumedParts ?? {},
      nextNodeId: file.nextNodeId,
      nextPartNum: file.nextPartNum,
    });
    engine = EngineClass.from_v1_json(irJson);
    const bytes = engine.save();
    // Verify before committing: load the bytes we just saved and confirm
    // the CRDT round-trip works. If verification fails, we won't overwrite
    // the legacy row — user's data stays reachable via the old path.
    const verify = EngineClass.load(bytes);
    try {
      verify.free();
    } catch {
      /* best effort */
    }
    return {
      kind: "crdt",
      version: "0.4",
      crdtBytes: bytes,
    };
  } catch (e) {
    console.warn("[backfill] legacy→CRDT migration failed:", e);
    return "failed";
  } finally {
    if (engine) {
      try {
        engine.free();
      } catch {
        /* best effort — wrapper is dead either way */
      }
    }
  }
}
