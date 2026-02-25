"use client";

import { useState, useCallback, useEffect } from "react";
import { useSearchParams } from "next/navigation";
import dynamic from "next/dynamic";
import { examples } from "@/lib/examples";
import { cn } from "@/lib/utils";
import {
  CaretDown,
  Share,
  Download,
  ArrowsHorizontal,
} from "@phosphor-icons/react";
import * as DropdownMenu from "@radix-ui/react-dropdown-menu";

const Editor = dynamic(() => import("./Editor").then(m => m.Editor), {
  ssr: false,
  loading: () => <div className="h-full bg-surface animate-pulse" />,
});

const Viewport = dynamic(() => import("./Viewport").then(m => m.Viewport), {
  ssr: false,
  loading: () => (
    <div className="h-full bg-surface flex items-center justify-center text-text-muted">
      Loading...
    </div>
  ),
});

const ParametricSliders = dynamic(
  () => import("./ParametricSliders").then(m => m.ParametricSliders),
  { ssr: false }
);

export function FullPlayground() {
  const searchParams = useSearchParams();
  const initialId = searchParams.get("example");
  const initialExample = (initialId && examples.find(e => e.id === initialId)) || examples[0]!;

  const [selectedExample, setSelectedExample] = useState(initialExample);
  const [code, setCode] = useState(initialExample.code);
  const [splitRatio, setSplitRatio] = useState(50);
  const [isDragging, setIsDragging] = useState(false);

  const handleExampleChange = (id: string) => {
    const example = examples.find(e => e.id === id);
    if (example) {
      setSelectedExample(example);
      setCode(example.code);
    }
  };

  const handleMouseDown = useCallback(() => {
    setIsDragging(true);
  }, []);

  const handleMouseMove = useCallback(
    (e: React.MouseEvent) => {
      if (!isDragging) return;
      const container = e.currentTarget as HTMLElement;
      const rect = container.getBoundingClientRect();
      const x = e.clientX - rect.left;
      const percent = (x / rect.width) * 100;
      setSplitRatio(Math.min(80, Math.max(20, percent)));
    },
    [isDragging]
  );

  const handleMouseUp = useCallback(() => {
    setIsDragging(false);
  }, []);

  return (
    <div
      className="h-screen flex flex-col"
      onMouseMove={handleMouseMove}
      onMouseUp={handleMouseUp}
      onMouseLeave={handleMouseUp}
    >
      {/* Toolbar */}
      <header className="h-12 flex items-center justify-between px-4 border-b border-border bg-bg">
        <div className="flex items-center gap-4">
          {/* Example selector */}
          <DropdownMenu.Root>
            <DropdownMenu.Trigger asChild>
              <button className="flex items-center gap-2 px-3 py-1.5 text-sm bg-surface hover:bg-hover rounded-md border border-border transition-colors">
                {selectedExample.name}
                <CaretDown size={14} />
              </button>
            </DropdownMenu.Trigger>
            <DropdownMenu.Portal>
              <DropdownMenu.Content
                className="min-w-[180px] bg-card border border-border rounded-lg shadow-xl py-1 z-50"
                sideOffset={4}
              >
                {examples.map(example => (
                  <DropdownMenu.Item
                    key={example.id}
                    className={cn(
                      "px-3 py-2 text-sm cursor-pointer outline-none transition-colors",
                      selectedExample.id === example.id
                        ? "bg-accent/10 text-accent"
                        : "hover:bg-hover"
                    )}
                    onSelect={() => handleExampleChange(example.id)}
                  >
                    <div className="font-medium">{example.name}</div>
                    <div className="text-xs text-text-muted">
                      {example.description}
                    </div>
                  </DropdownMenu.Item>
                ))}
              </DropdownMenu.Content>
            </DropdownMenu.Portal>
          </DropdownMenu.Root>
        </div>

        <div className="flex items-center gap-2">
          <button className="flex items-center gap-2 px-3 py-1.5 text-sm text-text-muted hover:text-text hover:bg-hover rounded-md transition-colors">
            <Share size={16} />
            Share
          </button>
          <button className="flex items-center gap-2 px-3 py-1.5 text-sm bg-accent hover:bg-accent-hover text-white rounded-md transition-colors">
            <Download size={16} />
            Export
          </button>
        </div>
      </header>

      {/* Main content area */}
      <div className="flex-1 flex overflow-hidden">
        {/* Editor pane */}
        <div
          className="h-full overflow-hidden"
          style={{ width: `${splitRatio}%` }}
        >
          <Editor value={code} onChange={setCode} language="rust" />
        </div>

        {/* Resize handle */}
        <div
          className={cn(
            "w-1 bg-border hover:bg-accent cursor-col-resize flex items-center justify-center transition-colors",
            isDragging && "bg-accent"
          )}
          onMouseDown={handleMouseDown}
        >
          <ArrowsHorizontal
            size={12}
            className={cn(
              "text-text-muted",
              isDragging && "text-white"
            )}
          />
        </div>

        {/* Viewport pane */}
        <div
          className="h-full overflow-hidden flex flex-col"
          style={{ width: `${100 - splitRatio}%` }}
        >
          <div className="flex-1">
            <Viewport document={selectedExample.document} />
          </div>

          {/* Parametric sliders */}
          <div className="border-t border-border">
            <ParametricSliders
              document={selectedExample.document}
              onUpdate={() => {
                // In future: update document parameters
              }}
            />
          </div>
        </div>
      </div>
    </div>
  );
}
