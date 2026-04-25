import { X } from "@phosphor-icons/react/dist/ssr/X";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { useOnboardingStore, type GuidedFlowStep } from "@/stores/onboarding-store";
import { t, type TranslationKey } from "@vcad/core";
import { useLocaleStore } from "@/stores/locale-store";

interface StepInfo {
  instructionKey: TranslationKey;
  detailKey?: TranslationKey;
}

const STEP_INFO: Record<Exclude<GuidedFlowStep, null>, StepInfo> = {
  "add-cube": {
    instructionKey: "tutorial.add_cube.instruction",
    detailKey: "tutorial.add_cube.detail",
  },
  "add-cylinder": {
    instructionKey: "tutorial.add_cylinder.instruction",
    detailKey: "tutorial.add_cylinder.detail",
  },
  "position-cylinder": {
    instructionKey: "tutorial.position_cylinder.instruction",
    detailKey: "tutorial.position_cylinder.detail",
  },
  subtract: {
    instructionKey: "tutorial.subtract.instruction",
    detailKey: "tutorial.subtract.detail",
  },
  celebrate: {
    instructionKey: "tutorial.celebrate.instruction",
    detailKey: "tutorial.celebrate.detail",
  },
};

const STEP_ORDER: Exclude<GuidedFlowStep, null>[] = [
  "add-cube",
  "add-cylinder",
  "position-cylinder",
  "subtract",
  "celebrate",
];

export function GuidedFlowOverlay() {
  const guidedFlowActive = useOnboardingStore((s) => s.guidedFlowActive);
  const guidedFlowStep = useOnboardingStore((s) => s.guidedFlowStep);
  const skipGuidedFlow = useOnboardingStore((s) => s.skipGuidedFlow);
  const completeGuidedFlow = useOnboardingStore((s) => s.completeGuidedFlow);
  useLocaleStore((s) => s.locale);

  if (!guidedFlowActive || !guidedFlowStep) return null;

  const stepInfo = STEP_INFO[guidedFlowStep];
  const currentStepIndex = STEP_ORDER.indexOf(guidedFlowStep);
  const isCelebrate = guidedFlowStep === "celebrate";

  return (
    <div className="absolute bottom-20 left-1/2 -translate-x-1/2 z-30 pointer-events-none">
      <div
        className={cn(
          "pointer-events-auto",
          "flex flex-col items-center gap-2 px-5 py-3",
          "bg-surface/95 backdrop-blur-sm",
          "border border-border",
          "shadow-lg shadow-black/20",
          "animate-in fade-in slide-in-from-bottom-2 duration-300"
        )}
      >
        {/* Close/skip button */}
        {!isCelebrate && (
          <button
            onClick={skipGuidedFlow}
            className="absolute right-2 top-2 p-1 text-text-muted hover:text-text"
            aria-label={t("tutorial.skip")}
          >
            <X size={12} />
          </button>
        )}

        {/* Instruction */}
        <div className="text-center">
          <p className="text-sm font-medium text-text">{t(stepInfo.instructionKey)}</p>
          {stepInfo.detailKey && (
            <p className="text-xs text-text-muted mt-0.5">{t(stepInfo.detailKey)}</p>
          )}
        </div>

        {/* Progress dots */}
        <div className="flex items-center gap-1.5">
          {STEP_ORDER.slice(0, -1).map((step, index) => (
            <div
              key={step}
              className={cn(
                "w-1.5 h-1.5 rounded-full transition-colors",
                index < currentStepIndex
                  ? "bg-brand"
                  : index === currentStepIndex
                    ? "bg-brand"
                    : "bg-border"
              )}
            />
          ))}
        </div>

        {/* Action buttons for celebrate step */}
        {isCelebrate && (
          <div className="flex gap-2 mt-1">
            <Button size="sm" onClick={completeGuidedFlow}>
              {t("tutorial.start_building")}
            </Button>
          </div>
        )}

        {/* Skip link for non-celebrate steps */}
        {!isCelebrate && (
          <button
            onClick={skipGuidedFlow}
            className="text-[10px] text-text-muted hover:text-text"
          >
            {t("tutorial.skip_short")}
          </button>
        )}
      </div>
    </div>
  );
}
