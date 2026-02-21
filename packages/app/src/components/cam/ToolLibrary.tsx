import { useState } from "react";
import { Plus } from "@phosphor-icons/react/dist/ssr/Plus";
import { Trash } from "@phosphor-icons/react/dist/ssr/Trash";
import { Wrench } from "@phosphor-icons/react/dist/ssr/Wrench";
import { Circle } from "@phosphor-icons/react/dist/ssr/Circle";
import { ArrowDown } from "@phosphor-icons/react/dist/ssr/ArrowDown";
import {
  useCamStore,
  type CamTool,
  type CamToolType,
} from "@/stores/cam-store";
import { cn } from "@/lib/utils";

const TOOL_TYPE_LABELS: Record<CamToolType, string> = {
  flat_endmill: "Flat Endmill",
  ball_endmill: "Ball Endmill",
  bull_endmill: "Bull Endmill",
  vbit: "V-Bit",
  drill: "Drill",
  face_mill: "Face Mill",
};

const TOOL_TYPE_ICONS: Record<CamToolType, typeof Wrench> = {
  flat_endmill: Wrench,
  ball_endmill: Circle,
  bull_endmill: Wrench,
  vbit: ArrowDown,
  drill: ArrowDown,
  face_mill: Wrench,
};

interface ToolLibraryProps {
  compact?: boolean;
}

export function ToolLibrary({ compact = false }: ToolLibraryProps) {
  const tools = useCamStore((s) => s.tools);
  const selectedToolId = useCamStore((s) => s.selectedToolId);
  const selectTool = useCamStore((s) => s.selectTool);
  const addTool = useCamStore((s) => s.addTool);
  const removeTool = useCamStore((s) => s.removeTool);

  const [showAddForm, setShowAddForm] = useState(false);
  const [newTool, setNewTool] = useState<Partial<CamTool>>({
    name: "",
    type: "flat_endmill",
    diameter: 6.0,
    fluteLength: 20.0,
    flutes: 2,
    defaultRpm: 12000,
    defaultFeed: 1000,
    defaultPlunge: 300,
  });

  const handleAddTool = () => {
    if (newTool.name && newTool.type && newTool.diameter) {
      addTool(newTool as Omit<CamTool, "id">);
      setNewTool({
        name: "",
        type: "flat_endmill",
        diameter: 6.0,
        fluteLength: 20.0,
        flutes: 2,
        defaultRpm: 12000,
        defaultFeed: 1000,
        defaultPlunge: 300,
      });
      setShowAddForm(false);
    }
  };

  if (compact) {
    return (
      <div className="space-y-1">
        <label className="text-xs text-text-muted">Tool</label>
        <select
          className="w-full bg-surface border border-border rounded px-2 py-1 text-sm"
          value={selectedToolId ?? ""}
          onChange={(e) => selectTool(e.target.value || null)}
        >
          <option value="">Select tool...</option>
          {tools.map((tool) => (
            <option key={tool.id} value={tool.id}>
              {tool.name} ({tool.diameter}mm)
            </option>
          ))}
        </select>
      </div>
    );
  }

  return (
    <div className="space-y-2">
      <div className="flex items-center justify-between">
        <h3 className="text-sm font-medium">Tool Library</h3>
        <button
          className="p-1 hover:bg-hover rounded text-text-muted hover:text-text"
          onClick={() => setShowAddForm(!showAddForm)}
        >
          <Plus size={14} />
        </button>
      </div>

      {showAddForm && (
        <div className="bg-surface-secondary p-2 rounded space-y-2 text-xs">
          <input
            type="text"
            placeholder="Tool name"
            className="w-full bg-surface border border-border rounded px-2 py-1"
            value={newTool.name}
            onChange={(e) => setNewTool({ ...newTool, name: e.target.value })}
          />
          <div className="grid grid-cols-2 gap-2">
            <select
              className="bg-surface border border-border rounded px-2 py-1"
              value={newTool.type}
              onChange={(e) =>
                setNewTool({ ...newTool, type: e.target.value as CamToolType })
              }
            >
              {Object.entries(TOOL_TYPE_LABELS).map(([value, label]) => (
                <option key={value} value={value}>
                  {label}
                </option>
              ))}
            </select>
            <input
              type="number"
              placeholder="Diameter"
              className="bg-surface border border-border rounded px-2 py-1"
              value={newTool.diameter}
              onChange={(e) =>
                setNewTool({ ...newTool, diameter: parseFloat(e.target.value) })
              }
            />
          </div>
          <div className="flex justify-end gap-2">
            <button
              className="px-2 py-1 text-text-muted hover:text-text"
              onClick={() => setShowAddForm(false)}
            >
              Cancel
            </button>
            <button
              className="px-2 py-1 bg-accent text-white rounded hover:bg-accent/90"
              onClick={handleAddTool}
            >
              Add
            </button>
          </div>
        </div>
      )}

      <div className="space-y-1 max-h-40 overflow-y-auto">
        {tools.map((tool) => {
          const Icon = TOOL_TYPE_ICONS[tool.type];
          return (
            <div
              key={tool.id}
              className={cn(
                "flex items-center gap-2 p-2 rounded cursor-pointer text-sm",
                "hover:bg-hover transition-colors",
                selectedToolId === tool.id && "bg-accent/20 border border-accent/40"
              )}
              onClick={() => selectTool(tool.id)}
            >
              <Icon size={14} className="text-text-muted" />
              <div className="flex-1 min-w-0">
                <div className="truncate">{tool.name}</div>
                <div className="text-xs text-text-muted">
                  {tool.diameter}mm {TOOL_TYPE_LABELS[tool.type]}
                </div>
              </div>
              <button
                className="p-1 hover:bg-error/20 rounded text-text-muted hover:text-error"
                onClick={(e) => {
                  e.stopPropagation();
                  removeTool(tool.id);
                }}
              >
                <Trash size={12} />
              </button>
            </div>
          );
        })}
      </div>
    </div>
  );
}
