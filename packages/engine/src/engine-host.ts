/**
 * The document engine off the main thread — step 1 of "store as a view".
 *
 * `WasmDocumentEngine` today lives on the main thread inside the document
 * store and is called synchronously. This module is the seam that lets it
 * move into a worker:
 *
 * - {@link handleEngineRequest} runs one request against an engine. It is
 *   the whole worker body (see `engine-worker.ts`) and also runs in-process.
 * - {@link DocumentEngineClient} is the main-thread side: an async command
 *   API whose results come back as deltas (`applyDelta` / `applyToolDelta`)
 *   and are folded into a local {@link EngineState} with `applyEngineDelta`.
 *   The client holds the view; the worker holds the truth.
 * - A {@link EngineTransport} carries requests: {@link workerTransport} for a
 *   real `Worker`, {@link inProcessTransport} for tests, SSR and fallback.
 *
 * Nothing adopts it yet (the store keeps its synchronous engine); the next
 * steps move store mutations onto the client.
 */
import type { DocCommand } from "@vcad/ir";
import {
  applyEngineDelta,
  parseDeltaEnvelope,
  stateFromEnvelope,
  type DeltaChanges,
  type EngineState,
} from "./delta.js";

/** The slice of `WasmDocumentEngine` the host drives. */
export interface HostedEngine {
  applyDelta(commandJson: string): string;
  applyToolDelta(tool: string, argsJson: string): string;
  stateEnvelope(): string;
  stateHash(): string;
  save(): Uint8Array;
  can_undo(): boolean;
  can_redo(): boolean;
  free(): void;
}

/** Constructors the host can make engines with. */
export interface HostedEngineClass {
  new (): HostedEngine;
  withReplica?(replica: number): HostedEngine;
  load(bytes: Uint8Array): HostedEngine;
}

/** One request to the engine host. */
export type EngineRequest =
  | { op: "init"; replica?: number }
  | { op: "load"; bytes: Uint8Array }
  | { op: "apply"; command: DocCommand }
  | { op: "applyTool"; tool: string; args: unknown }
  | { op: "state" }
  | { op: "save" };

/** The host's answer: raw engine JSON, so the wire stays the engine's own. */
export type EngineResponse =
  | { ok: true; envelope?: string; bytes?: Uint8Array; hash: string; canUndo: boolean; canRedo: boolean }
  | { ok: false; message: string };

/** Host-side state: the engine class and the live engine. */
export interface EngineHostState {
  Engine: HostedEngineClass;
  engine: HostedEngine | null;
}

/** Run one request against the host's engine. Never throws. */
export function handleEngineRequest(host: EngineHostState, req: EngineRequest): EngineResponse {
  try {
    let envelope: string | undefined;
    let bytes: Uint8Array | undefined;
    switch (req.op) {
      case "init":
        host.engine?.free();
        host.engine =
          req.replica !== undefined && host.Engine.withReplica
            ? host.Engine.withReplica(req.replica)
            : new host.Engine();
        envelope = host.engine.stateEnvelope();
        break;
      case "load":
        host.engine?.free();
        host.engine = host.Engine.load(req.bytes);
        envelope = host.engine.stateEnvelope();
        break;
      default: {
        const engine = host.engine;
        if (!engine) return { ok: false, message: "engine not initialized" };
        if (req.op === "apply") envelope = engine.applyDelta(JSON.stringify(req.command));
        else if (req.op === "applyTool") envelope = engine.applyToolDelta(req.tool, JSON.stringify(req.args));
        else if (req.op === "state") envelope = engine.stateEnvelope();
        else bytes = engine.save();
      }
    }
    const engine = host.engine!;
    return {
      ok: true,
      envelope,
      bytes,
      hash: engine.stateHash(),
      canUndo: engine.can_undo(),
      canRedo: engine.can_redo(),
    };
  } catch (e) {
    return { ok: false, message: e instanceof Error ? e.message : String(e) };
  }
}

/** Carries requests to an engine host. */
export interface EngineTransport {
  request(req: EngineRequest): Promise<EngineResponse>;
  dispose(): void;
}

/** Run the host in this thread (tests, SSR, no-Worker fallback). */
export function inProcessTransport(Engine: HostedEngineClass): EngineTransport {
  const host: EngineHostState = { Engine, engine: null };
  return {
    request: async (req) => handleEngineRequest(host, req),
    dispose: () => {
      host.engine?.free();
      host.engine = null;
    },
  };
}

/** Talk to `engine-worker.ts` (or anything speaking the same protocol). */
export function workerTransport(worker: Worker): EngineTransport {
  let next = 0;
  const pending = new Map<number, (r: EngineResponse) => void>();
  worker.addEventListener("message", (e: MessageEvent) => {
    const { id, response } = e.data as { id: number; response: EngineResponse };
    pending.get(id)?.(response);
    pending.delete(id);
  });
  return {
    request: (req) =>
      new Promise((resolve) => {
        const id = next++;
        pending.set(id, resolve);
        worker.postMessage({ id, req });
      }),
    dispose: () => worker.terminate(),
  };
}

/** What one command did, from the client's point of view. */
export interface CommandOutcome {
  state: EngineState;
  changes: DeltaChanges;
  createdFeatureId?: string;
  warnings: string[];
}

/** A request the engine host refused or failed. */
export class EngineHostError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "EngineHostError";
  }
}

/**
 * Main-thread view of a hosted engine. Requests are serialized (each waits
 * for the previous), so the delta base is always the state the host had.
 */
export class DocumentEngineClient {
  private view: EngineState | null = null;
  private queue: Promise<unknown> = Promise.resolve();
  private flags = { hash: "", canUndo: false, canRedo: false };

  constructor(private readonly transport: EngineTransport) {}

  /** The current view (null before `init`/`load`). */
  get state(): EngineState | null {
    return this.view;
  }
  get stateHash(): string {
    return this.flags.hash;
  }
  get canUndo(): boolean {
    return this.flags.canUndo;
  }
  get canRedo(): boolean {
    return this.flags.canRedo;
  }

  /** Start an empty document. */
  init(replica?: number): Promise<EngineState> {
    return this.full({ op: "init", replica });
  }

  /** Restore a `save()` snapshot. */
  load(bytes: Uint8Array): Promise<EngineState> {
    return this.full({ op: "load", bytes });
  }

  /** Re-read the whole state (resynchronizes the view). */
  refresh(): Promise<EngineState> {
    return this.full({ op: "state" });
  }

  /** Apply a command; rejects with `EngineRefusal` when the engine refuses. */
  apply(command: DocCommand): Promise<CommandOutcome> {
    return this.delta({ op: "apply", command });
  }

  /** Lower and apply an AI tool call. */
  applyTool(tool: string, args: unknown): Promise<CommandOutcome> {
    return this.delta({ op: "applyTool", tool, args });
  }

  /** Snapshot the engine (document + history). */
  save(): Promise<Uint8Array> {
    return this.enqueue(async () => {
      const r = this.check(await this.transport.request({ op: "save" }));
      return r.bytes ?? new Uint8Array();
    });
  }

  dispose(): void {
    this.transport.dispose();
    this.view = null;
  }

  private full(req: EngineRequest): Promise<EngineState> {
    return this.enqueue(async () => {
      const r = this.check(await this.transport.request(req));
      this.view = stateFromEnvelope(r.envelope!);
      return this.view;
    });
  }

  private delta(req: EngineRequest): Promise<CommandOutcome> {
    return this.enqueue(async () => {
      const base = this.view;
      if (!base) throw new EngineHostError("engine not initialized");
      const r = this.check(await this.transport.request(req));
      const delta = parseDeltaEnvelope(r.envelope!); // throws EngineRefusal
      const { state, changes } = applyEngineDelta(base, delta);
      this.view = state;
      return { state, changes, createdFeatureId: delta.createdFeatureId, warnings: delta.warnings };
    });
  }

  private check(r: EngineResponse): Extract<EngineResponse, { ok: true }> {
    if (!r.ok) throw new EngineHostError(r.message);
    this.flags = { hash: r.hash, canUndo: r.canUndo, canRedo: r.canRedo };
    return r;
  }

  private enqueue<T>(f: () => Promise<T>): Promise<T> {
    const run = this.queue.then(f, f);
    this.queue = run.catch(() => undefined);
    return run;
  }
}
