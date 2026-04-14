import { useState, useEffect, useRef, useMemo, useCallback } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { VisuallyHidden } from "@radix-ui/react-visually-hidden";
import { Cube } from "@phosphor-icons/react/dist/ssr/Cube";
import { Cylinder } from "@phosphor-icons/react/dist/ssr/Cylinder";
import { Globe } from "@phosphor-icons/react/dist/ssr/Globe";
import { ArrowsOutCardinal } from "@phosphor-icons/react/dist/ssr/ArrowsOutCardinal";
import { ArrowClockwise } from "@phosphor-icons/react/dist/ssr/ArrowClockwise";
import { ArrowsOut } from "@phosphor-icons/react/dist/ssr/ArrowsOut";
import { ArrowCounterClockwise } from "@phosphor-icons/react/dist/ssr/ArrowCounterClockwise";
import { Unite } from "@phosphor-icons/react/dist/ssr/Unite";
import { Subtract } from "@phosphor-icons/react/dist/ssr/Subtract";
import { Intersect } from "@phosphor-icons/react/dist/ssr/Intersect";
import { FloppyDisk } from "@phosphor-icons/react/dist/ssr/FloppyDisk";
import { FolderOpen } from "@phosphor-icons/react/dist/ssr/FolderOpen";
import { Export } from "@phosphor-icons/react/dist/ssr/Export";
import { GridFour } from "@phosphor-icons/react/dist/ssr/GridFour";
import { CubeTransparent } from "@phosphor-icons/react/dist/ssr/CubeTransparent";
import { SidebarSimple } from "@phosphor-icons/react/dist/ssr/SidebarSimple";
import { Sun } from "@phosphor-icons/react/dist/ssr/Sun";
import { Info } from "@phosphor-icons/react/dist/ssr/Info";
import { Trash } from "@phosphor-icons/react/dist/ssr/Trash";
import { Copy } from "@phosphor-icons/react/dist/ssr/Copy";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { MagnifyingGlass } from "@phosphor-icons/react/dist/ssr/MagnifyingGlass";
import { Package } from "@phosphor-icons/react/dist/ssr/Package";
import { PlusSquare } from "@phosphor-icons/react/dist/ssr/PlusSquare";
import { Anchor } from "@phosphor-icons/react/dist/ssr/Anchor";
import { ArrowsHorizontal } from "@phosphor-icons/react/dist/ssr/ArrowsHorizontal";
import { Sparkle } from "@phosphor-icons/react/dist/ssr/Sparkle";
import { SpinnerGap } from "@phosphor-icons/react/dist/ssr/SpinnerGap";
import { Play } from "@phosphor-icons/react/dist/ssr/Play";
import { Plus } from "@phosphor-icons/react/dist/ssr/Plus";
import { fromVCode, type Document } from "@vcad/ir";
import { generateCADServer } from "@/lib/server-inference";
import { generateCAD, isWebGPUAvailable } from "@/lib/browser-inference";
import { useNotificationStore } from "@/stores/notification-store";
import { useRequireAuth, AuthModal, useAuthStore } from "@vcad/auth";
import { useOnboardingStore } from "@/stores/onboarding-store";
import type { Command } from "@vcad/core";
import { useUiStore, useDocumentStore, parseVcadFile, useChatStore } from "@vcad/core";
import type { SelectionContext } from "@vcad/core";
import { useAppCommands } from "@/hooks/useAppCommands";
import { cn } from "@/lib/utils";
import { examples, type Example } from "@/data/examples";

const ICONS: Record<string, typeof Cube> = {
  Cube,
  Cylinder,
  Globe,
  ArrowsOutCardinal,
  ArrowClockwise,
  ArrowsOut,
  ArrowCounterClockwise,
  Unite,
  Subtract,
  Intersect,
  FloppyDisk,
  FolderOpen,
  Export,
  GridFour,
  CubeTransparent,
  SidebarSimple,
  Sun,
  Info,
  Trash,
  Copy,
  X,
  Package,
  PlusSquare,
  Anchor,
  ArrowsClockwise: ArrowClockwise,
  ArrowsHorizontal,
  Sparkle,
};

/** Contextual AI suggestions based on current state */
interface AISuggestion {
  id: string;
  label: string;
  prompt: string;
}

function getContextualSuggestions(
  selectionCount: number,
  hasParts: boolean,
): AISuggestion[] {
  if (!hasParts) {
    // Empty scene - suggest starting points
    return [
      { id: "ai-bracket", label: "Create a mounting bracket", prompt: "mounting bracket with 4 corner holes, 50mm wide" },
      { id: "ai-enclosure", label: "Create an electronics enclosure", prompt: "electronics enclosure box 80x60x40mm with ventilation slots" },
      { id: "ai-standoff", label: "Create standoffs", prompt: "M3 standoff 10mm tall with mounting holes" },
    ];
  }

  if (selectionCount === 1) {
    // One part selected - suggest modifications
    return [
      { id: "ai-holes", label: "Add mounting holes", prompt: "add 4 M3 mounting holes to the corners" },
      { id: "ai-taller", label: "Make it taller", prompt: "make this part twice as tall" },
      { id: "ai-fillet", label: "Add rounded edges", prompt: "add 2mm fillets to all edges" },
    ];
  }

  if (selectionCount === 2) {
    // Two parts selected - suggest combinations
    return [
      { id: "ai-connect", label: "Connect these parts", prompt: "create a connector between these two parts" },
      { id: "ai-align", label: "Align and join", prompt: "align these parts and join them" },
    ];
  }

  // Has parts but nothing selected
  return [
    { id: "ai-complement", label: "Add a matching part", prompt: "create a complementary part that fits with the existing geometry" },
    { id: "ai-base", label: "Create a base plate", prompt: "create a base plate to mount the existing parts" },
  ];
}

function highlightMatch(text: string, query: string): React.ReactNode {
  if (!query) return text;
  const lower = text.toLowerCase();
  const idx = lower.indexOf(query.toLowerCase());
  if (idx === -1) return text;
  return (
    <>
      {text.slice(0, idx)}
      <span className="text-brand font-medium">{text.slice(idx, idx + query.length)}</span>
      {text.slice(idx + query.length)}
    </>
  );
}

interface CommandPaletteProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onAboutOpen: () => void;
}

export function CommandPalette({ open, onOpenChange, onAboutOpen }: CommandPaletteProps) {
  const [query, setQuery] = useState("");
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [aiGenerating, setAiGenerating] = useState(false);
  const [aiStatus, setAiStatus] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  // Auth gating for AI features
  const { requireAuth, showAuth, setShowAuth, feature } = useRequireAuth("ai");

  const loadDocument = useDocumentStore((s) => s.loadDocument);
  const startGuidedFlow = useOnboardingStore((s) => s.startGuidedFlow);
  const incrementProjectsCreated = useOnboardingStore((s) => s.incrementProjectsCreated);

  // Subscribe to bits of store state that affect palette UI — welcome-mode
  // detection, AI suggestion gating, handleNewProject, send-to-chat. The
  // subscription also ensures that when selection changes, the palette
  // re-renders and re-evaluates each command's enabled() callback inline.
  const parts = useDocumentStore((s) => s.parts);
  const addPrimitive = useDocumentStore((s) => s.addPrimitive);
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const select = useUiStore((s) => s.select);
  const setTransformMode = useUiStore((s) => s.setTransformMode);

  const dismissPalette = useCallback(() => onOpenChange(false), [onOpenChange]);
  const commands = useAppCommands({
    onDismiss: dismissPalette,
    onAboutOpen,
  });

  // AI generation handler (inner function that does the actual work)
  const doAIGenerate = useCallback(async (prompt: string, useBrowser: boolean) => {
    setAiGenerating(true);
    onOpenChange(false); // Close palette immediately

    const store = useNotificationStore.getState();
    const docStore = useDocumentStore.getState();

    // Start AI progress with semantic stages
    const stages = useBrowser
      ? ["Loading AI model", "Generating geometry", "Building mesh"]
      : ["Connecting to server", "Generating geometry", "Building mesh"];
    const progressId = store.startAIOperation(prompt, stages);

    try {
      let ir: string;
      let durationMs: number;

      if (useBrowser) {
        // Browser inference with cad0-mini
        store.updateAIProgress(progressId, 0, 10);
        setAiStatus("Loading AI model...");

        const result = await generateCAD(prompt, undefined, (loaded, total, status) => {
          const pct = Math.round((loaded / total) * 100);
          setAiStatus(status);
          if (status.includes("Loading") || status.includes("Initializing")) {
            store.updateAIProgress(progressId, 0, Math.min(pct * 0.7, 70));
          }
        });

        ir = result.ir;
        durationMs = result.durationMs;
      } else {
        // Server inference with cad0
        store.updateAIProgress(progressId, 0, 10);
        setAiStatus("Connecting to server...");

        const currentSession = useAuthStore.getState().session;
        if (!currentSession) {
          throw new Error("Not authenticated");
        }
        const result = await generateCADServer(prompt, {
          authToken: currentSession.access_token,
        });

        ir = result.ir;
        durationMs = result.durationMs;
      }

      // Stage 2: Building geometry
      store.updateAIProgress(progressId, 1, 80);
      setAiStatus("Building geometry...");

      // Parse the VCode to a Document
      const generatedDoc: Document = fromVCode(ir);

      // Stage 3: Validating
      store.updateAIProgress(progressId, 2, 95);

      // Merge into current document (not replace)
      docStore.addFromIR(generatedDoc, prompt.slice(0, 30));

      // Complete with action result
      store.completeAIOperation(progressId, {
        type: "success",
        title: useBrowser ? "Generated locally" : "Generated from server",
        description: `Created in ${(durationMs / 1000).toFixed(1)}s`,
        actions: [
          {
            label: "Undo",
            onClick: () => useDocumentStore.getState().undo(),
            variant: "secondary",
          },
        ],
      });
    } catch (err) {
      console.error("AI generation failed:", err);
      store.failAIOperation(
        progressId,
        err instanceof Error ? err.message : "Generation failed"
      );
    } finally {
      setAiGenerating(false);
      setAiStatus("");
    }
  }, [onOpenChange]);

  // Wrapper that chooses browser vs server inference
  const handleAIGenerate = useCallback(async (prompt: string) => {
    // Try browser inference first if WebGPU available
    const hasWebGPU = await isWebGPUAvailable();
    if (hasWebGPU) {
      doAIGenerate(prompt, true);
    } else {
      // Fall back to server (requires auth)
      requireAuth(() => doAIGenerate(prompt, false));
    }
  }, [requireAuth, doAIGenerate]);

  // Get contextual AI suggestions
  const aiSuggestions = useMemo(() => {
    return getContextualSuggestions(
      selectedPartIds.size,
      parts.length > 0
    );
  }, [selectedPartIds.size, parts.length]);

  // Welcome mode: show when canvas is empty and no query
  const isWelcomeMode = parts.length === 0 && !query.trim();

  // Handle new project (start guided flow)
  const handleNewProject = useCallback(() => {
    incrementProjectsCreated();
    startGuidedFlow();
    onOpenChange(false);
  }, [incrementProjectsCreated, startGuidedFlow, onOpenChange]);

  // Handle skip tutorial - just add a cube
  const handleSkipTutorial = useCallback(() => {
    incrementProjectsCreated();
    const partId = addPrimitive("cube");
    select(partId);
    setTransformMode("translate");
    onOpenChange(false);
  }, [incrementProjectsCreated, addPrimitive, select, setTransformMode, onOpenChange]);

  // Handle open file
  const handleOpenFile = useCallback(() => {
    fileInputRef.current?.click();
  }, []);

  // Handle file input change
  const handleFileChange = useCallback((e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;

    const reader = new FileReader();
    reader.onload = (event) => {
      try {
        const content = event.target?.result as string;
        const vcadFile = parseVcadFile(content);
        loadDocument(vcadFile);
        onOpenChange(false);
      } catch (err) {
        console.error("Failed to parse file:", err);
        useNotificationStore.getState().addToast("Failed to load file", "error");
      }
    };
    reader.readAsText(file);
    e.target.value = "";
  }, [loadDocument, onOpenChange]);

  // Handle example load
  const handleOpenExample = useCallback((example: Example) => {
    loadDocument(example.file);
    onOpenChange(false);
  }, [loadDocument, onOpenChange]);

  const filteredCommands = useMemo(() => {
    if (!query.trim()) return commands;
    const q = query.toLowerCase();
    return commands.filter((cmd) => {
      if (cmd.label.toLowerCase().includes(q)) return true;
      return cmd.keywords.some((kw) => kw.includes(q));
    });
  }, [commands, query]);

  // Determine if we should show AI section
  const showAISection = query.trim().length > 2 || filteredCommands.length === 0;
  const aiPrompt = query.trim();

  // Reset selection when query changes
  useEffect(() => {
    setSelectedIndex(0);
  }, [query]);

  // Reset query when opening
  useEffect(() => {
    if (open) {
      setQuery("");
      setSelectedIndex(0);
    }
  }, [open]);

  // Scroll selected item into view
  useEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const selected = list.querySelector("[data-selected=true]");
    if (selected) {
      selected.scrollIntoView({ block: "nearest" });
    }
  }, [selectedIndex]);

  const executeCommand = useCallback(
    (cmd: Command) => {
      if (cmd.enabled && !cmd.enabled()) return;
      cmd.action();
    },
    [],
  );

  function handleKeyDown(e: React.KeyboardEvent) {
    if (aiGenerating) {
      if (e.key === "Escape") {
        // TODO: Cancel generation
      }
      return;
    }

    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        setSelectedIndex((i) => Math.min(i + 1, filteredCommands.length - 1));
        break;
      case "ArrowUp":
        e.preventDefault();
        setSelectedIndex((i) => Math.max(i - 1, 0));
        break;
      case "Enter":
        e.preventDefault();
        // If we have a matching command, execute it
        if (filteredCommands.length > 0 && selectedIndex < filteredCommands.length) {
          const cmd = filteredCommands[selectedIndex];
          if (cmd) executeCommand(cmd);
        }
        // Otherwise, escalate to AI chat sidebar
        else if (aiPrompt) {
          const chatStore = useChatStore.getState();
          chatStore.setOpen(true);
          const selContext = Array.from(selectedPartIds).map((id) => {
            const part = parts.find((p) => p.id === id);
            return part
              ? { partId: id, partName: part.name, geometryType: "part" as const }
              : null;
          }).filter(Boolean) as SelectionContext[];
          chatStore.sendMessage(aiPrompt, selContext);
          onOpenChange(false);
        }
        break;
      case "Escape":
        onOpenChange(false);
        break;
    }
  }

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/30" />
        <Dialog.Content
          className="fixed left-1/2 top-[20%] z-50 w-full max-w-md -translate-x-1/2  border border-border bg-surface shadow-2xl"
          onKeyDown={handleKeyDown}
          aria-describedby={undefined}
        >
          <VisuallyHidden>
            <Dialog.Title>Command Palette</Dialog.Title>
          </VisuallyHidden>
          <div className="flex items-center gap-2 border-b border-border px-3 py-2">
            <MagnifyingGlass size={16} className="text-text-muted" />
            <input
              ref={inputRef}
              type="text"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Type a command or describe what to create..."
              className="flex-1 bg-transparent text-sm text-text outline-none placeholder:text-text-muted"
              autoFocus
            />
            <kbd className=" bg-border/50 px-1.5 py-0.5 text-[10px] text-text-muted">esc</kbd>
          </div>
          {/* Hidden file input */}
          <input
            ref={fileInputRef}
            type="file"
            accept=".vcad,.json"
            onChange={handleFileChange}
            className="hidden"
          />

          <div ref={listRef} className="max-h-[400px] overflow-y-auto p-1">
            {/* AI generating state */}
            {aiGenerating && (
              <div className="flex items-center gap-3 px-3 py-4 border-b border-border mb-1">
                <SpinnerGap size={16} className="text-brand animate-spin" />
                <span className="text-sm text-text-muted">{aiStatus || "Generating..."}</span>
              </div>
            )}

            {/* Welcome mode - show when canvas is empty and no query */}
            {!aiGenerating && isWelcomeMode && (
              <>
                {/* Branding */}
                <div className="flex flex-col items-center py-4 border-b border-border mb-1">
                  <h1 className="text-xl font-bold tracking-tighter text-text">
                    vcad<span className="text-brand">.</span>
                  </h1>
                  <p className="text-[10px] text-text-muted">
                    free parametric cad for everyone
                  </p>
                </div>

                {/* Quick actions */}
                <div className="px-3 py-1.5">
                  <span className="text-[10px] font-medium text-text-muted uppercase tracking-wider">
                    Get Started
                  </span>
                </div>
                <button
                  onClick={handleNewProject}
                  className="flex w-full items-center gap-3 px-3 py-2 text-left text-sm transition-colors hover:bg-border/30"
                >
                  <Plus size={16} className="shrink-0 text-brand" />
                  <span className="flex-1">New Project</span>
                  <span className="text-[10px] text-text-muted">guided tutorial</span>
                </button>
                <button
                  onClick={handleSkipTutorial}
                  className="flex w-full items-center gap-3 px-3 py-2 text-left text-sm transition-colors hover:bg-border/30"
                >
                  <Cube size={16} className="shrink-0 text-text-muted" />
                  <span className="flex-1">Blank Project</span>
                  <span className="text-[10px] text-text-muted">skip tutorial</span>
                </button>
                <button
                  onClick={handleOpenFile}
                  className="flex w-full items-center gap-3 px-3 py-2 text-left text-sm transition-colors hover:bg-border/30"
                >
                  <FolderOpen size={16} className="shrink-0 text-text-muted" />
                  <span className="flex-1">Open File</span>
                  <kbd className="bg-border/50 px-1.5 py-0.5 text-[10px] text-text-muted">⌘O</kbd>
                </button>

                {/* Examples */}
                <div className="border-t border-border my-1" />
                <div className="px-3 py-1.5">
                  <span className="text-[10px] font-medium text-text-muted uppercase tracking-wider">
                    Examples
                  </span>
                </div>
                {examples.map((example) => (
                  <button
                    key={example.id}
                    onClick={() => handleOpenExample(example)}
                    className="flex w-full items-center gap-3 px-3 py-2 text-left text-sm transition-colors hover:bg-border/30"
                  >
                    <Play size={16} className="shrink-0 text-text-muted" />
                    <span className="flex-1 text-text-muted">{example.name}</span>
                  </button>
                ))}

                {/* AI suggestions */}
                <div className="border-t border-border my-1" />
                <div className="px-3 py-1.5">
                  <span className="text-[10px] font-medium text-text-muted uppercase tracking-wider">
                    Or describe what to create
                  </span>
                </div>
                {aiSuggestions.map((suggestion) => (
                  <button
                    key={suggestion.id}
                    onClick={() => handleAIGenerate(suggestion.prompt)}
                    className="flex w-full items-center gap-3 px-3 py-2 text-left text-sm transition-colors hover:bg-border/30"
                  >
                    <Sparkle size={16} className="shrink-0 text-brand/60" />
                    <span className="flex-1 text-text-muted">{suggestion.label}</span>
                  </button>
                ))}
              </>
            )}

            {/* Commands section - hide in welcome mode */}
            {!aiGenerating && !isWelcomeMode && filteredCommands.length > 0 && (
              <>
                {filteredCommands.map((cmd, idx) => {
                  const Icon = ICONS[cmd.icon] ?? Cube;
                  const isDisabled = cmd.enabled && !cmd.enabled();
                  const isSelected = idx === selectedIndex;

                  return (
                    <button
                      key={cmd.id}
                      data-selected={isSelected}
                      disabled={isDisabled}
                      onClick={() => executeCommand(cmd)}
                      className={cn(
                        "flex w-full items-center gap-3 px-3 py-2 text-left text-sm transition-colors",
                        isSelected && !isDisabled && "bg-brand/20",
                        isDisabled && "opacity-40 cursor-not-allowed",
                        !isSelected && !isDisabled && "hover:bg-border/30",
                      )}
                    >
                      <Icon size={16} className="shrink-0 text-text-muted" />
                      <span className="flex-1">{highlightMatch(cmd.label, query)}</span>
                      {cmd.shortcut && (
                        <kbd className="bg-border/50 px-1.5 py-0.5 text-[10px] text-text-muted">
                          {cmd.shortcut}
                        </kbd>
                      )}
                    </button>
                  );
                })}
              </>
            )}

            {/* AI Generation section - hide in welcome mode */}
            {!aiGenerating && !isWelcomeMode && showAISection && (
              <>
                {filteredCommands.length > 0 && (
                  <div className="border-t border-border my-1" />
                )}
                <div className="px-3 py-1.5">
                  <span className="text-[10px] font-medium text-text-muted uppercase tracking-wider">
                    Ask AI
                  </span>
                </div>

                {/* Custom prompt option */}
                {aiPrompt && (
                  <button
                    onClick={() => handleAIGenerate(aiPrompt)}
                    className={cn(
                      "flex w-full items-center gap-3 px-3 py-2 text-left text-sm transition-colors",
                      filteredCommands.length === 0 && selectedIndex === 0
                        ? "bg-brand/20"
                        : "hover:bg-border/30",
                    )}
                  >
                    <Sparkle size={16} className="shrink-0 text-brand" />
                    <span className="flex-1">
                      Generate: <span className="text-brand">{aiPrompt}</span>
                    </span>
                    <kbd className="bg-border/50 px-1.5 py-0.5 text-[10px] text-text-muted">
                      server
                    </kbd>
                  </button>
                )}

                {/* Contextual suggestions */}
                {!aiPrompt && aiSuggestions.map((suggestion) => (
                  <button
                    key={suggestion.id}
                    onClick={() => handleAIGenerate(suggestion.prompt)}
                    className={cn(
                      "flex w-full items-center gap-3 px-3 py-2 text-left text-sm transition-colors",
                      "hover:bg-border/30",
                    )}
                  >
                    <Sparkle size={16} className="shrink-0 text-brand/60" />
                    <span className="flex-1 text-text-muted">{suggestion.label}</span>
                  </button>
                ))}
              </>
            )}

            {/* Empty state - hide in welcome mode */}
            {!aiGenerating && !isWelcomeMode && filteredCommands.length === 0 && !showAISection && (
              <div className="px-3 py-6 text-center text-xs text-text-muted">
                No commands found
              </div>
            )}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
      <AuthModal open={showAuth} onOpenChange={setShowAuth} feature={feature} />
    </Dialog.Root>
  );
}
