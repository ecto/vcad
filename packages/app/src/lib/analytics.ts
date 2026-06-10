let ph: typeof import("posthog-js").default | null = null;
import("posthog-js").then((m) => {
  ph = m.default;
});

export const analytics = {
  // Document lifecycle
  documentCreated: () => ph?.capture("document_created"),
  documentOpened: (source: "recent" | "file" | "new") =>
    ph?.capture("document_opened", { source }),
  documentSaved: (method: "manual" | "auto" | "cloud") =>
    ph?.capture("document_saved", { method }),
  documentExported: (format: "stl" | "glb" | "step" | "dxf" | "gerber") =>
    ph?.capture("document_exported", { format }),

  // Feature usage
  primitiveAdded: (kind: "cube" | "cylinder" | "sphere" | "cone") =>
    ph?.capture("primitive_added", { kind }),
  booleanApplied: (type: "union" | "difference" | "intersection") =>
    ph?.capture("boolean_applied", { type }),
  sketchStarted: () => ph?.capture("sketch_started"),
  sketchCompleted: (constraintCount: number) =>
    ph?.capture("sketch_completed", { constraint_count: constraintCount }),
  extrudeApplied: () => ph?.capture("extrude_applied"),

  // Auth events
  signupStarted: (provider: "google" | "github") =>
    ph?.capture("signup_started", { provider }),
  signupCompleted: (provider: "google" | "github") =>
    ph?.capture("signup_completed", { provider }),

  // Advanced features
  stepImported: () => ph?.capture("step_imported"),
  aiGenerationStarted: (prompt: string) =>
    ph?.capture("ai_generation_started", { prompt_length: prompt.length }),
  aiGenerationCompleted: (durationMs: number) =>
    ph?.capture("ai_generation_completed", { duration_ms: durationMs }),
  physicsSimulationRun: () => ph?.capture("physics_simulation_run"),
  printPanelOpened: () => ph?.capture("print_panel_opened"),
  quotePanelOpened: () => ph?.capture("quote_panel_opened"),

  // Command registry — fired for every action triggered through
  // useAppCommands, regardless of which surface invoked it. Lets us see
  // which commands are actually used and whether users prefer the mobile
  // hamburger, desktop menu bar, or ⌘K palette for each.
  commandExecuted: (params: {
    id: string;
    category?: string;
    surface: "palette" | "mobile-menu" | "desktop-menu";
  }) =>
    ph?.capture("command_executed", {
      command_id: params.id,
      command_category: params.category ?? "uncategorized",
      surface: params.surface,
    }),
  commandFailed: (params: {
    id: string;
    category?: string;
    surface: "palette" | "mobile-menu" | "desktop-menu";
    error: string;
  }) =>
    ph?.capture("command_failed", {
      command_id: params.id,
      command_category: params.category ?? "uncategorized",
      surface: params.surface,
      error: params.error.slice(0, 500),
    }),

  // Lazy chunk loading — transient network failures, stale-deploy
  // reloads, and per-region error-boundary recoveries. Lets us see how
  // often users hit these and which chunks are flakiest in production.
  chunkLoadRetry: (name: string, attempt: number, error: string) =>
    ph?.capture("chunk_load_retry", {
      chunk: name,
      attempt,
      error: error.slice(0, 500),
    }),
  chunkLoadFailed: (name: string, attempts: number, error: string) =>
    ph?.capture("chunk_load_failed", {
      chunk: name,
      attempts,
      error: error.slice(0, 500),
    }),
  chunkLoadStaleDeploy: () => ph?.capture("chunk_load_stale_deploy"),
  asyncBoundaryCaught: (region: string, error: string) =>
    ph?.capture("async_boundary_caught", {
      region,
      error: error.slice(0, 500),
    }),
  asyncBoundaryReset: (region: string) =>
    ph?.capture("async_boundary_reset", { region }),
};
