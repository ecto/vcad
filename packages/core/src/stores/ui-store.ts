import { create } from "zustand";
import type { Theme, ToolMode, TransformMode } from "../types.js";

export interface MaterialPreview {
  partId: string;
  materialKey: string;
}

export type RenderMode = "standard" | "raytrace";
export type RaytraceQuality = "draft" | "standard" | "high";
export type RaytraceDebugMode = "off" | "normals" | "face-id" | "lighting" | "orientation";

export type ToolbarTab = "create" | "transform" | "combine" | "modify" | "assembly" | "simulate" | "build";

export type SidebarPane = "tree" | "inspector";

/**
 * How the local user's camera relates to another participant's camera.
 * - "free": independent. The local camera is only moved by user input.
 * - "follow": user camera stays put, but the other participant's camera
 *             is rendered in-scene (frustum + attention highlight).
 * - "lock":  user camera lerps to match the followed participant's camera
 *             every frame — "see through their eyes".
 */
export type FollowMode = "free" | "follow" | "lock";

/**
 * What the inspector pane is focused on. `null` means it follows the viewport
 * selection (the common case). Non-null targets are things you can inspect
 * that are NOT 3D-selectable parts — currently just the scene itself.
 */
export type InspectorTarget = { kind: "scene" } | null;

export interface UiState {
  selectedPartIds: Set<string>;
  hoveredPartId: string | null;
  commandPaletteOpen: boolean;
  toolMode: ToolMode;
  transformMode: TransformMode;
  featureTreeOpen: boolean;
  theme: Theme;
  isDraggingGizmo: boolean;
  isOrbiting: boolean;
  /** True while ai-screenshot.ts is capturing an off-screen frame from the
   *  AI's camera. Overlays (selection highlights, participant attention
   *  rings) should suppress themselves when this is set so the screenshot
   *  reflects material colors rather than UI chrome. An axes gnomon is
   *  rendered in this mode so the AI can orient itself without guessing
   *  which direction is "right" or "front". Always false in normal use. */
  captureMode: boolean;
  showWireframe: boolean;
  gridSnap: boolean;
  pointSnap: boolean;
  snapIncrement: number;
  clipboard: string[];
  deleteConfirmParts: string[] | null;
  // Material selector state
  previewMaterial: MaterialPreview | null;
  recentMaterials: string[]; // Last 6 used material keys
  favoriteMaterials: string[]; // User-pinned material keys
  // Ray tracing state
  renderMode: RenderMode;
  raytraceQuality: RaytraceQuality;
  raytraceDebugMode: RaytraceDebugMode;
  raytraceAvailable: boolean;
  raytraceEdgesEnabled: boolean;
  raytraceEdgeDepthThreshold: number;
  raytraceEdgeNormalThreshold: number;
  // Toolbar state
  toolbarExpanded: boolean;
  toolbarTab: ToolbarTab;
  // Left sidebar pane (tree vs inspector / drill-down)
  sidebarPane: SidebarPane;
  inspectorTarget: InspectorTarget;
  // Bottom status bar visibility
  statusBarVisible: boolean;
  // Follow/Lock mode for watching another participant's camera (AI for now,
  // peers in the future). When `followingParticipantId` is null, the mode
  // is effectively "free" regardless of value.
  followMode: FollowMode;
  followingParticipantId: string | null;
  // Live cursor position in world space (Z-up), null when pointer is off the viewport
  cursorWorld: { x: number; y: number; z: number } | null;
  // Read-only share session. When set, the app is viewing a public share link;
  // every persistent-state mutation is blocked and redirected to the fork prompt.
  readOnlyShare: { token: string; docName: string } | null;

  select: (partId: string | null) => void;
  toggleSelect: (partId: string) => void;
  selectMultiple: (partIds: string[]) => void;
  clearSelection: () => void;
  setHoveredPartId: (partId: string | null) => void;
  toggleCommandPalette: () => void;
  setCommandPaletteOpen: (open: boolean) => void;
  setToolMode: (mode: ToolMode) => void;
  setTransformMode: (mode: TransformMode) => void;
  toggleFeatureTree: () => void;
  setFeatureTreeOpen: (open: boolean) => void;
  setTheme: (theme: Theme) => void;
  toggleTheme: () => void;
  toggleWireframe: () => void;
  toggleGridSnap: () => void;
  togglePointSnap: () => void;
  setSnapIncrement: (value: number) => void;
  setDraggingGizmo: (dragging: boolean) => void;
  setOrbiting: (orbiting: boolean) => void;
  setCaptureMode: (on: boolean) => void;
  copyToClipboard: (partIds: string[]) => void;
  showDeleteConfirm: (partIds: string[]) => void;
  hideDeleteConfirm: () => void;
  // Material selector actions
  setPreviewMaterial: (preview: MaterialPreview | null) => void;
  addRecentMaterial: (key: string) => void;
  toggleFavoriteMaterial: (key: string) => void;
  // Ray tracing actions
  setRenderMode: (mode: RenderMode) => void;
  toggleRenderMode: () => void;
  setRaytraceQuality: (quality: RaytraceQuality) => void;
  setRaytraceDebugMode: (mode: RaytraceDebugMode) => void;
  setRaytraceAvailable: (available: boolean) => void;
  setRaytraceEdgesEnabled: (enabled: boolean) => void;
  setRaytraceEdgeDepthThreshold: (threshold: number) => void;
  setRaytraceEdgeNormalThreshold: (threshold: number) => void;
  // Toolbar actions
  setToolbarExpanded: (expanded: boolean) => void;
  toggleToolbarExpanded: () => void;
  setToolbarTab: (tab: ToolbarTab) => void;
  // Sidebar pane actions
  setSidebarPane: (pane: SidebarPane) => void;
  setInspectorTarget: (target: InspectorTarget) => void;
  toggleStatusBar: () => void;
  setCursorWorld: (pos: { x: number; y: number; z: number } | null) => void;
  setReadOnlyShare: (share: { token: string; docName: string } | null) => void;
  setFollowMode: (mode: FollowMode) => void;
  setFollowingParticipant: (participantId: string | null) => void;
}

// Load persisted material preferences from localStorage
function loadPersistedMaterials(): { recent: string[]; favorites: string[] } {
  if (typeof window === "undefined") {
    return { recent: [], favorites: [] };
  }
  try {
    const recent = JSON.parse(localStorage.getItem("vcad:recentMaterials") ?? "[]");
    const favorites = JSON.parse(localStorage.getItem("vcad:favoriteMaterials") ?? "[]");
    return {
      recent: Array.isArray(recent) ? recent : [],
      favorites: Array.isArray(favorites) ? favorites : [],
    };
  } catch {
    return { recent: [], favorites: [] };
  }
}

// Load persisted toolbar preferences from localStorage
function loadToolbarExpanded(): boolean {
  if (typeof window === "undefined") {
    return true; // Default to expanded
  }
  try {
    const stored = localStorage.getItem("vcad:toolbarExpanded");
    return stored === null ? true : stored === "true";
  } catch {
    return true;
  }
}

const persistedMaterials = loadPersistedMaterials();
const persistedToolbarExpanded = loadToolbarExpanded();

export const useUiStore = create<UiState>((set) => ({
  selectedPartIds: new Set(),
  hoveredPartId: null,
  commandPaletteOpen: false,
  toolMode: "select",
  transformMode: "translate",
  featureTreeOpen: true,
  theme: "system",
  isDraggingGizmo: false,
  isOrbiting: false,
  captureMode: false,
  showWireframe: false,
  gridSnap: true,
  pointSnap: true,
  snapIncrement: 5,
  clipboard: [],
  deleteConfirmParts: null,
  previewMaterial: null,
  recentMaterials: persistedMaterials.recent,
  favoriteMaterials: persistedMaterials.favorites,
  renderMode: "standard",
  raytraceQuality: "draft",
  raytraceDebugMode: "off",
  raytraceAvailable: false,
  raytraceEdgesEnabled: true,
  raytraceEdgeDepthThreshold: 0.1,
  raytraceEdgeNormalThreshold: 30.0,
  toolbarExpanded: persistedToolbarExpanded,
  toolbarTab: "create" as ToolbarTab,
  sidebarPane: "tree" as SidebarPane,
  inspectorTarget: null as InspectorTarget,
  statusBarVisible: true,
  cursorWorld: null,
  readOnlyShare: null,
  followMode: "free",
  followingParticipantId: null,

  select: (partId) =>
    set({ selectedPartIds: partId ? new Set([partId]) : new Set() }),

  toggleSelect: (partId) =>
    set((s) => {
      const next = new Set(s.selectedPartIds);
      if (next.has(partId)) {
        next.delete(partId);
      } else {
        next.add(partId);
      }
      return { selectedPartIds: next };
    }),

  selectMultiple: (partIds) =>
    set({ selectedPartIds: new Set(partIds) }),

  clearSelection: () => set({ selectedPartIds: new Set() }),

  setHoveredPartId: (partId) => set({ hoveredPartId: partId }),

  toggleCommandPalette: () =>
    set((s) => ({ commandPaletteOpen: !s.commandPaletteOpen })),

  setCommandPaletteOpen: (open) => set({ commandPaletteOpen: open }),

  setToolMode: (mode) => set({ toolMode: mode }),

  setTransformMode: (mode) => set({ transformMode: mode }),

  toggleFeatureTree: () =>
    set((s) => ({ featureTreeOpen: !s.featureTreeOpen })),

  setFeatureTreeOpen: (open) => set({ featureTreeOpen: open }),

  setTheme: (theme) => set({ theme }),

  toggleTheme: () =>
    set((s) => ({
      theme: s.theme === "system" ? "light" : s.theme === "light" ? "dark" : "system",
    })),

  toggleWireframe: () =>
    set((s) => ({ showWireframe: !s.showWireframe })),

  toggleGridSnap: () =>
    set((s) => ({ gridSnap: !s.gridSnap })),

  togglePointSnap: () =>
    set((s) => ({ pointSnap: !s.pointSnap })),

  setSnapIncrement: (value) =>
    set({ snapIncrement: value, gridSnap: true }),

  setDraggingGizmo: (dragging) => set({ isDraggingGizmo: dragging }),

  setOrbiting: (orbiting) => set({ isOrbiting: orbiting }),

  setCaptureMode: (on) => set({ captureMode: on }),

  copyToClipboard: (partIds) => set({ clipboard: partIds }),

  showDeleteConfirm: (partIds) => set({ deleteConfirmParts: partIds }),

  hideDeleteConfirm: () => set({ deleteConfirmParts: null }),

  setPreviewMaterial: (preview) => set({ previewMaterial: preview }),

  addRecentMaterial: (key) =>
    set((s) => {
      // Remove if already exists, then add to front
      const filtered = s.recentMaterials.filter((k) => k !== key);
      const recent = [key, ...filtered].slice(0, 6);
      // Persist to localStorage
      try {
        localStorage.setItem("vcad:recentMaterials", JSON.stringify(recent));
      } catch {
        // Ignore storage errors
      }
      return { recentMaterials: recent };
    }),

  toggleFavoriteMaterial: (key) =>
    set((s) => {
      const isFavorite = s.favoriteMaterials.includes(key);
      const favorites = isFavorite
        ? s.favoriteMaterials.filter((k) => k !== key)
        : [...s.favoriteMaterials, key];
      // Persist to localStorage
      try {
        localStorage.setItem("vcad:favoriteMaterials", JSON.stringify(favorites));
      } catch {
        // Ignore storage errors
      }
      return { favoriteMaterials: favorites };
    }),

  setRenderMode: (mode) => set({ renderMode: mode }),

  toggleRenderMode: () =>
    set((s) => ({
      renderMode: s.renderMode === "standard" ? "raytrace" : "standard",
    })),

  setRaytraceQuality: (quality) => set({ raytraceQuality: quality }),

  setRaytraceDebugMode: (mode) => set({ raytraceDebugMode: mode }),

  setRaytraceAvailable: (available) => set({ raytraceAvailable: available }),

  setRaytraceEdgesEnabled: (enabled) => set({ raytraceEdgesEnabled: enabled }),

  setRaytraceEdgeDepthThreshold: (threshold) => set({ raytraceEdgeDepthThreshold: threshold }),

  setRaytraceEdgeNormalThreshold: (threshold) => set({ raytraceEdgeNormalThreshold: threshold }),

  setToolbarExpanded: (expanded) => {
    try {
      localStorage.setItem("vcad:toolbarExpanded", String(expanded));
    } catch {
      // Ignore storage errors
    }
    set({ toolbarExpanded: expanded });
  },

  toggleToolbarExpanded: () =>
    set((s) => {
      const expanded = !s.toolbarExpanded;
      try {
        localStorage.setItem("vcad:toolbarExpanded", String(expanded));
      } catch {
        // Ignore storage errors
      }
      return { toolbarExpanded: expanded };
    }),

  setToolbarTab: (tab) => set({ toolbarTab: tab }),

  setSidebarPane: (pane) => set({ sidebarPane: pane }),

  setInspectorTarget: (target) => set({ inspectorTarget: target }),

  toggleStatusBar: () => set((s) => ({ statusBarVisible: !s.statusBarVisible })),

  setCursorWorld: (pos) => set({ cursorWorld: pos }),

  setReadOnlyShare: (share) => set({ readOnlyShare: share }),

  setFollowMode: (mode) => set({ followMode: mode }),

  setFollowingParticipant: (participantId) => set({ followingParticipantId: participantId }),
}));
