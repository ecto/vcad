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
  documentExported: (format: "stl" | "glb" | "step" | "dxf") =>
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
};
