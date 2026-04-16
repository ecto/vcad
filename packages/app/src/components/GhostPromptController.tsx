import { useMemo } from "react";
import { GhostPrompt } from "./GhostPrompt";
import { useOnboardingStore } from "@/stores/onboarding-store";
import { useDocumentStore, useUiStore } from "@vcad/core";

export function GhostPromptController() {
  const guidedFlowActive = useOnboardingStore((s) => s.guidedFlowActive);
  const sessionsCompleted = useOnboardingStore((s) => s.sessionsCompleted);
  const parts = useDocumentStore((s) => s.parts);
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);

  // Disable ghost prompts during guided flow or after 3 sessions
  const enabled = !guidedFlowActive && sessionsCompleted < 3;

  const prompt = useMemo(() => {
    if (!enabled) return null;

    if (parts.length === 1) {
      return "Open Create to add another shape";
    }

    if (parts.length >= 2 && selectedPartIds.size === 0) {
      return "Select two parts to combine them";
    }

    if (selectedPartIds.size === 2) {
      return "Open Combine to merge or cut";
    }

    return null;
  }, [enabled, parts.length, selectedPartIds.size]);

  return <GhostPrompt message={prompt ?? ""} visible={!!prompt} />;
}
