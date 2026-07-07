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
  // Export funnel — started fires at the moment the user triggers an export,
  // completed only on success. A started without a completed is a failed or
  // aborted export, which document_exported alone (success-only) can't show.
  exportStarted: (format: "stl" | "glb" | "step" | "dxf" | "gerber") =>
    ph?.capture("export_started", { format }),
  exportCompleted: (format: "stl" | "glb" | "step" | "dxf" | "gerber") =>
    ph?.capture("export_completed", { format }),

  // Activation funnel — which starting template (example part or molecule
  // demo) a user opened, from the first-run gallery, ⌘K palette, or menu.
  templateOpened: (id: string) => ph?.capture("template_opened", { template_id: id }),

  // Feature usage
  primitiveAdded: (kind: "cube" | "cylinder" | "sphere" | "cone") =>
    ph?.capture("primitive_added", { kind }),
  booleanApplied: (type: "union" | "difference" | "intersection") =>
    ph?.capture("boolean_applied", { type }),
  sketchStarted: () => ph?.capture("sketch_started"),
  sketchCompleted: (constraintCount: number) =>
    ph?.capture("sketch_completed", { constraint_count: constraintCount }),
  // Sketch mode exited without committing an operation. Reasons: "empty"
  // (nothing drawn), "discarded" (drawn segments thrown away), "no_operation"
  // (finished with segments but no extrude/revolve/… pending), or
  // "face_selection" (bailed at the pick-a-face step before sketching).
  sketchAbandoned: (
    reason: "empty" | "discarded" | "no_operation" | "face_selection",
  ) => ph?.capture("sketch_abandoned", { reason }),
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

  // "Continue in Claude" handoff — which host, and whether it was a signed-in
  // (token) or accountless (inline) handoff.
  continueHandoff: (host: string, mode: "token" | "inline") =>
    ph?.capture("continue_handoff", { host, mode }),

  // The very first command this browser profile ever executes — the key
  // activation step. localStorage-guarded so it fires once per user, not
  // once per session; the flag is only set when the event actually sends.
  firstCommand: (id: string) => {
    if (!ph) return;
    try {
      if (localStorage.getItem("vcad_first_command") != null) return;
      localStorage.setItem("vcad_first_command", "1");
    } catch {
      return; // storage unavailable — skip rather than fire every command
    }
    ph.capture("first_command", { command_id: id });
  },

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
