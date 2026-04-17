import { lazy, type ComponentType, type LazyExoticComponent } from "react";
import { analytics } from "@/lib/analytics";

type ModuleWithDefault<T> = { default: ComponentType<T> };
type Importer<T> = () => Promise<ModuleWithDefault<T>>;

const RETRY_DELAYS_MS = [0, 250, 750, 1750];
const JITTER_MS = 250;

function isTransientImportError(err: unknown): boolean {
  const msg = String((err as { message?: unknown } | null)?.message ?? err ?? "");
  return /Failed to fetch|NetworkError|network connection|Importing a module script failed|Load failed|error loading dynamically imported module/i.test(
    msg,
  );
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * `React.lazy` + bounded retry on transient dynamic-import failures.
 *
 * Retries only on errors that look like network/preload failures. Permanent
 * errors (syntax errors in the chunk, missing exports) fail fast on the first
 * attempt so we don't mask real bugs with silent retry loops.
 *
 * The stale-deploy case — where the browser references a chunk hash that no
 * longer exists on the CDN — is handled separately in `bootstrap.ts` via
 * Vite's `vite:preloadError` event (one-shot reload, guarded against loops).
 * This helper covers the remaining case: live deploy + flaky network.
 */
export function lazyWithRetry<T>(
  importer: Importer<T>,
  name: string,
): LazyExoticComponent<ComponentType<T>> {
  return lazy(async () => {
    let lastErr: unknown;
    for (let attempt = 0; attempt < RETRY_DELAYS_MS.length; attempt++) {
      const baseDelay = RETRY_DELAYS_MS[attempt] ?? 0;
      if (baseDelay > 0) {
        await sleep(baseDelay + Math.random() * JITTER_MS);
      }
      try {
        return await importer();
      } catch (err) {
        lastErr = err;
        const transient = isTransientImportError(err);
        if (attempt > 0 || !transient) {
          analytics.chunkLoadRetry(
            name,
            attempt,
            err instanceof Error ? err.message : String(err),
          );
        }
        if (!transient) break;
      }
    }
    analytics.chunkLoadFailed(
      name,
      RETRY_DELAYS_MS.length,
      lastErr instanceof Error ? lastErr.message : String(lastErr),
    );
    throw lastErr;
  });
}
