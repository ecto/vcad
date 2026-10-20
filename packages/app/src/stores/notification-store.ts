import { create } from "zustand";

// ============================================================================
// Types
// ============================================================================

/** Base notification interface */
interface NotificationBase {
  id: string;
  timestamp: number;
  dismissible: boolean;
}

/** Simple toast notification */
export interface Toast extends NotificationBase {
  kind: "toast";
  type: "success" | "error" | "warning" | "info";
  message: string;
  duration: number;
}

/** AI operation stage */
export interface AIStage {
  label: string;
  status: "pending" | "running" | "complete" | "error";
}

/** AI operation progress notification */
export interface AIProgress extends NotificationBase {
  kind: "ai-progress";
  prompt: string;
  stages: AIStage[];
  progress: number; // 0-100
  cancelable: boolean;
  onCancel?: () => void;
  /** Optional download progress in bytes (for model downloads) */
  downloadProgress?: {
    loaded: number;
    total: number;
  };
}

/** Option for decision cards */
export interface DecisionOption {
  id: string;
  label: string;
  thumbnail?: string; // base64 or URL
  description?: string;
}

/** Decision request notification */
export interface Decision extends NotificationBase {
  kind: "decision";
  title: string;
  description: string;
  options: DecisionOption[];
  onSelect: (optionId: string) => void;
}

/** Action button for result/warning cards */
export interface ActionButton {
  label: string;
  onClick: () => void;
  variant?: "primary" | "secondary" | "destructive";
}

/** Completed action with iteration options */
export interface ActionResult extends NotificationBase {
  kind: "action-result";
  type: "success" | "error";
  title: string;
  description?: string;
  actions: ActionButton[];
}

/** Warning category */
export type WarningCategory = "dfm" | "validation" | "performance" | "suggestion";

/** Proactive warning notification */
export interface Warning extends NotificationBase {
  kind: "warning";
  category: WarningCategory;
  title: string;
  description: string;
  actions: ActionButton[];
  learnMoreUrl?: string;
}

/** Union of all notification types */
export type NotificationItem =
  | Toast
  | AIProgress
  | Decision
  | ActionResult
  | Warning;

/** Activity log entry type */
export type ActivityType =
  | "ai-generation"
  | "operation"
  | "export"
  | "error"
  | "warning";

/** Activity log entry (persisted version of notifications) */
export interface ActivityEntry {
  id: string;
  timestamp: number;
  type: ActivityType;
  title: string;
  details: Record<string, unknown>;
  undoable: boolean;
  onUndo?: () => void;
  onRegenerate?: () => void;
}

/** Toast options */
export interface ToastOptions {
  duration?: number;
  dismissible?: boolean;
}

// ============================================================================
// Store Interface
// ============================================================================

interface NotificationStore {
  // Active notifications (visible in corner)
  notifications: NotificationItem[];

  // Activity history (visible in panel)
  activityLog: ActivityEntry[];

  // Activity panel visibility
  activityPanelOpen: boolean;
  setActivityPanelOpen: (open: boolean) => void;
  toggleActivityPanel: () => void;

  // Simple toasts (backwards compatible)
  toast: {
    success: (message: string, options?: ToastOptions) => string;
    error: (message: string, options?: ToastOptions) => string;
    warning: (message: string, options?: ToastOptions) => string;
    info: (message: string, options?: ToastOptions) => string;
  };

  // Legacy API (for backwards compatibility during migration)
  addToast: (
    message: string,
    type: Toast["type"],
    duration?: number
  ) => string;

  // AI progress
  startAIOperation: (
    prompt: string,
    stages: string[],
    onCancel?: () => void
  ) => string;
  updateAIProgress: (
    id: string,
    stageIndex: number,
    progress: number,
    downloadProgress?: { loaded: number; total: number }
  ) => void;
  completeAIOperation: (
    id: string,
    result?: Partial<Omit<ActionResult, "id" | "timestamp" | "kind" | "dismissible">>
  ) => void;
  failAIOperation: (id: string, error: string) => void;
  cancelAIOperation: (id: string) => void;

  // Decisions
  requestDecision: (
    decision: Omit<Decision, "id" | "timestamp" | "kind" | "dismissible">
  ) => Promise<string>;

  // Action results
  showActionResult: (
    result: Omit<ActionResult, "id" | "timestamp" | "kind" | "dismissible">
  ) => string;

  // Warnings
  showWarning: (
    warning: Omit<Warning, "id" | "timestamp" | "kind" | "dismissible">
  ) => string;

  // Activity log
  logActivity: (entry: Omit<ActivityEntry, "id" | "timestamp">) => void;
  clearActivityLog: () => void;

  // General
  dismiss: (id: string) => void;
  dismissAll: () => void;

  // Pause/resume auto-dismiss (for hover)
  pauseAutoDismiss: (id: string) => void;
  resumeAutoDismiss: (id: string) => void;
}

// ============================================================================
// Implementation
// ============================================================================

let notificationId = 0;
const generateId = () => `notification-${++notificationId}`;

// Track auto-dismiss timeouts
const dismissTimeouts = new Map<string, NodeJS.Timeout>();
const pausedDurations = new Map<string, number>();

export const useNotificationStore = create<NotificationStore>((set, get) => {
  // Helper to schedule auto-dismiss
  const scheduleAutoDismiss = (id: string, duration: number) => {
    if (duration <= 0) return;

    const timeout = setTimeout(() => {
      get().dismiss(id);
      dismissTimeouts.delete(id);
    }, duration);

    dismissTimeouts.set(id, timeout);
  };

  // Helper to add a toast
  const addToastInternal = (
    message: string,
    type: Toast["type"],
    options?: ToastOptions
  ): string => {
    const id = generateId();
    const duration = options?.duration ?? 4000;
    const dismissible = options?.dismissible ?? true;

    const toast: Toast = {
      id,
      timestamp: Date.now(),
      kind: "toast",
      type,
      message,
      duration,
      dismissible,
    };

    set((state) => ({
      notifications: [...state.notifications, toast],
    }));

    scheduleAutoDismiss(id, duration);

    return id;
  };

  return {
    notifications: [],
    activityLog: [],
    activityPanelOpen: false,

    setActivityPanelOpen: (open) => set({ activityPanelOpen: open }),
    toggleActivityPanel: () =>
      set((state) => ({ activityPanelOpen: !state.activityPanelOpen })),

    // Toast helpers
    toast: {
      success: (message, options) => addToastInternal(message, "success", options),
      error: (message, options) =>
        addToastInternal(message, "error", { duration: 6000, ...options }),
      warning: (message, options) =>
        addToastInternal(message, "warning", { duration: 5000, ...options }),
      info: (message, options) => addToastInternal(message, "info", options),
    },

    // Legacy API
    addToast: (message, type, duration = 4000) =>
      addToastInternal(message, type, { duration }),

    // AI Progress
    startAIOperation: (prompt, stages, onCancel) => {
      const id = generateId();
      const notification: AIProgress = {
        id,
        timestamp: Date.now(),
        kind: "ai-progress",
        prompt,
        stages: stages.map((label) => ({ label, status: "pending" })),
        progress: 0,
        cancelable: !!onCancel,
        onCancel,
        dismissible: false,
      };

      set((state) => ({
        notifications: [...state.notifications, notification],
      }));

      // Log to activity
      get().logActivity({
        type: "ai-generation",
        title: `Generating: ${prompt.slice(0, 50)}${prompt.length > 50 ? "..." : ""}`,
        details: { prompt },
        undoable: false,
      });

      return id;
    },

    updateAIProgress: (id, stageIndex, progress, downloadProgress) => {
      set((state) => ({
        notifications: state.notifications.map((n) => {
          if (n.id !== id || n.kind !== "ai-progress") return n;

          const stages = n.stages.map((stage, idx) => {
            if (idx < stageIndex) return { ...stage, status: "complete" as const };
            if (idx === stageIndex) return { ...stage, status: "running" as const };
            return stage;
          });

          return {
            ...n,
            stages,
            progress,
            ...(downloadProgress && { downloadProgress }),
          };
        }),
      }));
    },

    completeAIOperation: (id, result) => {
      const state = get();
      const notification = state.notifications.find(
        (n) => n.id === id && n.kind === "ai-progress"
      ) as AIProgress | undefined;

      if (!notification) return;

      // Remove the progress notification
      set((state) => ({
        notifications: state.notifications.filter((n) => n.id !== id),
      }));

      // Show success result if provided
      if (result) {
        get().showActionResult({
          type: "success",
          title: result.title ?? "Generation complete",
          description: result.description,
          actions: result.actions ?? [],
        });
      } else {
        // Default success toast
        get().toast.success("Generation complete");
      }
    },

    failAIOperation: (id, error) => {
      const state = get();
      const notification = state.notifications.find(
        (n) => n.id === id && n.kind === "ai-progress"
      ) as AIProgress | undefined;

      // Remove the progress notification
      set((state) => ({
        notifications: state.notifications.filter((n) => n.id !== id),
      }));

      // Show error toast
      get().toast.error(error);

      // Update activity log
      if (notification) {
        get().logActivity({
          type: "error",
          title: `Failed: ${notification.prompt.slice(0, 50)}`,
          details: { prompt: notification.prompt, error },
          undoable: false,
        });
      }
    },

    cancelAIOperation: (id) => {
      const notification = get().notifications.find(
        (n) => n.id === id && n.kind === "ai-progress"
      ) as AIProgress | undefined;

      if (notification?.onCancel) {
        notification.onCancel();
      }

      set((state) => ({
        notifications: state.notifications.filter((n) => n.id !== id),
      }));

      get().toast.info("Generation cancelled");
    },

    // Decisions
    requestDecision: (decision) => {
      return new Promise((resolve) => {
        const id = generateId();
        const notification: Decision = {
          id,
          timestamp: Date.now(),
          kind: "decision",
          dismissible: false,
          ...decision,
          onSelect: (optionId) => {
            decision.onSelect(optionId);
            get().dismiss(id);
            resolve(optionId);
          },
        };

        set((state) => ({
          notifications: [...state.notifications, notification],
        }));
      });
    },

    // Action results
    showActionResult: (result) => {
      const id = generateId();
      const notification: ActionResult = {
        id,
        timestamp: Date.now(),
        kind: "action-result",
        dismissible: true,
        ...result,
      };

      set((state) => ({
        notifications: [...state.notifications, notification],
      }));

      // Auto-dismiss after 8 seconds if no actions
      if (result.actions.length === 0) {
        scheduleAutoDismiss(id, 8000);
      }

      return id;
    },

    // Warnings
    showWarning: (warning) => {
      const id = generateId();
      const notification: Warning = {
        id,
        timestamp: Date.now(),
        kind: "warning",
        dismissible: true,
        ...warning,
      };

      set((state) => ({
        notifications: [...state.notifications, notification],
      }));

      // Log to activity
      get().logActivity({
        type: "warning",
        title: warning.title,
        details: { description: warning.description, category: warning.category },
        undoable: false,
      });

      return id;
    },

    // Activity log
    logActivity: (entry) => {
      const activityEntry: ActivityEntry = {
        id: generateId(),
        timestamp: Date.now(),
        ...entry,
      };

      set((state) => ({
        activityLog: [activityEntry, ...state.activityLog].slice(0, 100), // Keep last 100
      }));
    },

    clearActivityLog: () => set({ activityLog: [] }),

    // General
    dismiss: (id) => {
      // Clear any pending timeout
      const timeout = dismissTimeouts.get(id);
      if (timeout) {
        clearTimeout(timeout);
        dismissTimeouts.delete(id);
      }
      pausedDurations.delete(id);

      set((state) => ({
        notifications: state.notifications.filter((n) => n.id !== id),
      }));
    },

    dismissAll: () => {
      // Clear all timeouts
      dismissTimeouts.forEach((timeout) => clearTimeout(timeout));
      dismissTimeouts.clear();
      pausedDurations.clear();

      set({ notifications: [] });
    },

    // Pause/resume (for hover)
    pauseAutoDismiss: (id) => {
      const timeout = dismissTimeouts.get(id);
      if (!timeout) return;

      clearTimeout(timeout);
      dismissTimeouts.delete(id);

      // Store remaining time
      const notification = get().notifications.find((n) => n.id === id);
      if (notification && "duration" in notification) {
        const elapsed = Date.now() - notification.timestamp;
        const remaining = Math.max(0, notification.duration - elapsed);
        pausedDurations.set(id, remaining);
      }
    },

    resumeAutoDismiss: (id) => {
      const remaining = pausedDurations.get(id);
      if (remaining === undefined) return;

      pausedDurations.delete(id);
      scheduleAutoDismiss(id, remaining);
    },
  };
});

// ============================================================================
// Convenience exports for backwards compatibility
// ============================================================================

/** @deprecated Use useNotificationStore instead */
export const useToastStore = useNotificationStore;
