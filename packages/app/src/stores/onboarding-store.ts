import { create } from "zustand";
import { persist } from "zustand/middleware";

export type GuidedFlowStep =
  | "add-cube"
  | "add-cylinder"
  | "position-cylinder"
  | "subtract"
  | "celebrate"
  | null;

interface OnboardingState {
  // Existing
  projectsCreated: number;
  welcomeModalDismissed: boolean;
  /** Session-only: the welcome has been closed this visit but may return
   * on next page load unless `welcomeModalDismissed` is also set. */
  welcomeHiddenThisSession: boolean;

  // Guided Flow
  guidedFlowActive: boolean;
  guidedFlowStep: GuidedFlowStep;
  guidedFlowCompleted: boolean;

  // Ghost Prompts
  sessionsCompleted: number;

  // Actions
  incrementProjectsCreated: () => void;
  dismissWelcomeModal: () => void;
  hideWelcomeThisSession: () => void;
  startGuidedFlow: () => void;
  advanceGuidedFlow: () => void;
  skipGuidedFlow: () => void;
  completeGuidedFlow: () => void;
  incrementSessions: () => void;
}

const STEP_ORDER: GuidedFlowStep[] = [
  "add-cube",
  "add-cylinder",
  "position-cylinder",
  "subtract",
  "celebrate",
];

export const useOnboardingStore = create<OnboardingState>()(
  persist(
    (set, get) => ({
      projectsCreated: 0,
      welcomeModalDismissed: false,
      welcomeHiddenThisSession: false,
      guidedFlowActive: false,
      guidedFlowStep: null,
      guidedFlowCompleted: false,
      sessionsCompleted: 0,

      incrementProjectsCreated: () =>
        set((state) => ({
          projectsCreated: state.projectsCreated + 1,
        })),

      dismissWelcomeModal: () =>
        set({ welcomeModalDismissed: true, welcomeHiddenThisSession: true }),

      hideWelcomeThisSession: () => set({ welcomeHiddenThisSession: true }),

      startGuidedFlow: () =>
        set({
          guidedFlowActive: true,
          guidedFlowStep: "add-cube",
          welcomeHiddenThisSession: true,
        }),

      advanceGuidedFlow: () => {
        const { guidedFlowStep } = get();
        const currentIndex = STEP_ORDER.indexOf(guidedFlowStep);
        if (currentIndex === -1 || currentIndex >= STEP_ORDER.length - 1) {
          return;
        }
        set({ guidedFlowStep: STEP_ORDER[currentIndex + 1] });
      },

      skipGuidedFlow: () =>
        set({
          guidedFlowActive: false,
          guidedFlowStep: null,
        }),

      completeGuidedFlow: () =>
        set({
          guidedFlowActive: false,
          guidedFlowStep: null,
          guidedFlowCompleted: true,
        }),

      incrementSessions: () =>
        set((state) => ({
          sessionsCompleted: state.sessionsCompleted + 1,
        })),
    }),
    {
      name: "vcad-onboarding",
      partialize: (state) => ({
        projectsCreated: state.projectsCreated,
        welcomeModalDismissed: state.welcomeModalDismissed,
        guidedFlowCompleted: state.guidedFlowCompleted,
        sessionsCompleted: state.sessionsCompleted,
      }),
    }
  )
);
