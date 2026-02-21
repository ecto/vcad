import { useCallback, useState } from "react";
import { Question } from "@phosphor-icons/react/dist/ssr/Question";
import { cn } from "@/lib/utils";
import type { Decision } from "@/stores/notification-store";

interface DecisionCardProps {
  decision: Decision;
  onDismiss?: () => void;
  onMouseEnter?: () => void;
  onMouseLeave?: () => void;
}

export function DecisionCard({
  decision,
  onMouseEnter,
  onMouseLeave,
}: DecisionCardProps) {
  const [selectedId, setSelectedId] = useState<string | null>(null);

  const handleSelect = useCallback(
    (optionId: string) => {
      setSelectedId(optionId);
      // Small delay to show selection feedback
      setTimeout(() => {
        decision.onSelect(optionId);
      }, 150);
    },
    [decision]
  );

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent, optionId: string) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        handleSelect(optionId);
      }
    },
    [handleSelect]
  );

  const hasThumbnails = decision.options.some((opt) => opt.thumbnail);

  return (
    <div
      role="dialog"
      aria-modal="false"
      aria-labelledby={`decision-title-${decision.id}`}
      aria-describedby={`decision-desc-${decision.id}`}
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
      className={cn(
        "border border-yellow-400/30 bg-card shadow-2xl",
        "focus-within:ring-2 focus-within:ring-accent/50"
      )}
    >
      {/* Header */}
      <div className="flex items-center gap-2 px-3 py-2 border-b border-border/50">
        <Question
          size={14}
          weight="fill"
          className="text-yellow-400"
          aria-hidden="true"
        />
        <span
          id={`decision-title-${decision.id}`}
          className="text-xs font-medium text-text"
        >
          {decision.title}
        </span>
      </div>

      {/* Description */}
      <div className="px-3 py-2">
        <p
          id={`decision-desc-${decision.id}`}
          className="text-[11px] text-text-muted"
        >
          {decision.description}
        </p>
      </div>

      {/* Options */}
      {hasThumbnails ? (
        // Grid layout with thumbnails
        <div className="px-3 pb-3 grid grid-cols-3 gap-2">
          {decision.options.map((option) => (
            <button
              key={option.id}
              onClick={() => handleSelect(option.id)}
              onKeyDown={(e) => handleKeyDown(e, option.id)}
              disabled={selectedId !== null}
              className={cn(
                "flex flex-col items-center gap-1 p-2 border rounded transition-all",
                selectedId === option.id
                  ? "border-accent bg-accent/20"
                  : "border-border hover:border-accent/50 hover:bg-accent/5",
                selectedId !== null &&
                  selectedId !== option.id &&
                  "opacity-50",
                "focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/50"
              )}
            >
              {option.thumbnail && (
                <div className="w-12 h-12 bg-bg rounded overflow-hidden">
                  <img
                    src={option.thumbnail}
                    alt=""
                    className="w-full h-full object-cover"
                  />
                </div>
              )}
              <span className="text-[10px] text-text text-center">
                {option.label}
              </span>
            </button>
          ))}
        </div>
      ) : (
        // Button list layout
        <div className="px-3 pb-3 flex flex-wrap gap-2">
          {decision.options.map((option) => (
            <button
              key={option.id}
              onClick={() => handleSelect(option.id)}
              onKeyDown={(e) => handleKeyDown(e, option.id)}
              disabled={selectedId !== null}
              title={option.description}
              className={cn(
                "px-3 py-1.5 text-[11px] border rounded transition-all",
                selectedId === option.id
                  ? "border-accent bg-accent text-white"
                  : "border-border hover:border-accent/50 hover:bg-accent/5",
                selectedId !== null &&
                  selectedId !== option.id &&
                  "opacity-50",
                "focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/50"
              )}
            >
              {option.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
