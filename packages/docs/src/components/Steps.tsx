"use client";

import { useState } from "react";
import dynamic from "next/dynamic";
import type { Document } from "@vcad/ir";
import { cn } from "@/lib/utils";

const Viewport = dynamic(
  () => import("./Playground/Viewport").then((m) => m.Viewport),
  {
    ssr: false,
    loading: () => (
      <div className="h-full bg-surface animate-pulse rounded-lg" />
    ),
  }
);

interface Step {
  document: Document;
  label: string;
}

interface StepsProps {
  steps: Step[];
  height?: string;
}

/**
 * Progressive build visualization — step indicator + single viewport.
 * User clicks a step to see that stage of construction.
 */
export function Steps({ steps, height = "350px" }: StepsProps) {
  const [active, setActive] = useState(0);

  if (steps.length === 0) return null;

  return (
    <div className="my-6">
      {/* Step indicators */}
      <div className="flex items-center gap-2 mb-4 overflow-x-auto pb-2">
        {steps.map((step, i) => (
          <button
            key={i}
            onClick={() => setActive(i)}
            className={cn(
              "flex items-center gap-2 px-3 py-1.5 rounded-full text-sm whitespace-nowrap transition-colors",
              i === active
                ? "bg-accent text-white"
                : "bg-surface border border-border text-text-muted hover:text-text hover:border-accent/50"
            )}
          >
            <span
              className={cn(
                "w-5 h-5 rounded-full flex items-center justify-center text-xs font-medium",
                i === active
                  ? "bg-white/20"
                  : "bg-accent/10 text-accent"
              )}
            >
              {i + 1}
            </span>
            {step.label}
          </button>
        ))}
      </div>

      {/* Viewport */}
      <div
        className="rounded-lg border border-border overflow-hidden"
        style={{ height }}
      >
        <Viewport document={steps[active]!.document} />
      </div>
    </div>
  );
}
