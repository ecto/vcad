import { useState } from "react";
import { Plus } from "@phosphor-icons/react/dist/ssr/Plus";
import { Trash } from "@phosphor-icons/react/dist/ssr/Trash";
import { ArrowUp } from "@phosphor-icons/react/dist/ssr/ArrowUp";
import { ArrowDown } from "@phosphor-icons/react/dist/ssr/ArrowDown";
import { Eye } from "@phosphor-icons/react/dist/ssr/Eye";
import { EyeSlash } from "@phosphor-icons/react/dist/ssr/EyeSlash";
import { Square } from "@phosphor-icons/react/dist/ssr/Square";
import { Circle as CircleIcon } from "@phosphor-icons/react/dist/ssr/Circle";
import { Path } from "@phosphor-icons/react/dist/ssr/Path";
import { SelectionBackground } from "@phosphor-icons/react/dist/ssr/SelectionBackground";
import {
  useCamStore,
  type CamOperation,
  type CamOperationType,
} from "@/stores/cam-store";
import { cn } from "@/lib/utils";
import { ToolLibrary } from "./ToolLibrary";

const OPERATION_LABELS: Record<CamOperationType, string> = {
  face: "Face",
  pocket: "Pocket",
  pocket_circle: "Circular Pocket",
  contour: "Contour",
  roughing3d: "3D Roughing",
};

const OPERATION_ICONS: Record<CamOperationType, typeof Square> = {
  face: SelectionBackground,
  pocket: Square,
  pocket_circle: CircleIcon,
  contour: Path,
  roughing3d: SelectionBackground,
};

interface AddOperationFormProps {
  onAdd: (type: CamOperationType) => void;
  onCancel: () => void;
}

function AddOperationForm({ onAdd, onCancel }: AddOperationFormProps) {
  return (
    <div className="bg-surface-secondary p-2 rounded space-y-2 text-xs">
      <div className="text-text-muted mb-1">Select operation type:</div>
      <div className="grid grid-cols-2 gap-1">
        {(Object.entries(OPERATION_LABELS) as [CamOperationType, string][]).map(
          ([type, label]) => {
            const Icon = OPERATION_ICONS[type];
            return (
              <button
                key={type}
                className="flex items-center gap-2 p-2 hover:bg-hover rounded text-left"
                onClick={() => onAdd(type)}
              >
                <Icon size={14} />
                <span>{label}</span>
              </button>
            );
          }
        )}
      </div>
      <div className="flex justify-end">
        <button
          className="px-2 py-1 text-text-muted hover:text-text"
          onClick={onCancel}
        >
          Cancel
        </button>
      </div>
    </div>
  );
}

interface OperationEditorProps {
  operation: CamOperation;
  onUpdate: (updates: Partial<CamOperation>) => void;
}

function OperationEditor({ operation, onUpdate }: OperationEditorProps) {
  const renderFields = () => {
    switch (operation.type) {
      case "face":
        return (
          <>
            <div className="grid grid-cols-2 gap-2">
              <div>
                <label className="text-xs text-text-muted">Min X</label>
                <input
                  type="number"
                  className="w-full bg-surface border border-border rounded px-2 py-1 text-sm"
                  value={operation.minX}
                  onChange={(e) => onUpdate({ minX: parseFloat(e.target.value) })}
                />
              </div>
              <div>
                <label className="text-xs text-text-muted">Min Y</label>
                <input
                  type="number"
                  className="w-full bg-surface border border-border rounded px-2 py-1 text-sm"
                  value={operation.minY}
                  onChange={(e) => onUpdate({ minY: parseFloat(e.target.value) })}
                />
              </div>
              <div>
                <label className="text-xs text-text-muted">Max X</label>
                <input
                  type="number"
                  className="w-full bg-surface border border-border rounded px-2 py-1 text-sm"
                  value={operation.maxX}
                  onChange={(e) => onUpdate({ maxX: parseFloat(e.target.value) })}
                />
              </div>
              <div>
                <label className="text-xs text-text-muted">Max Y</label>
                <input
                  type="number"
                  className="w-full bg-surface border border-border rounded px-2 py-1 text-sm"
                  value={operation.maxY}
                  onChange={(e) => onUpdate({ maxY: parseFloat(e.target.value) })}
                />
              </div>
            </div>
          </>
        );

      case "pocket":
        return (
          <>
            <div className="grid grid-cols-2 gap-2">
              <div>
                <label className="text-xs text-text-muted">X</label>
                <input
                  type="number"
                  className="w-full bg-surface border border-border rounded px-2 py-1 text-sm"
                  value={operation.x}
                  onChange={(e) => onUpdate({ x: parseFloat(e.target.value) })}
                />
              </div>
              <div>
                <label className="text-xs text-text-muted">Y</label>
                <input
                  type="number"
                  className="w-full bg-surface border border-border rounded px-2 py-1 text-sm"
                  value={operation.y}
                  onChange={(e) => onUpdate({ y: parseFloat(e.target.value) })}
                />
              </div>
              <div>
                <label className="text-xs text-text-muted">Width</label>
                <input
                  type="number"
                  className="w-full bg-surface border border-border rounded px-2 py-1 text-sm"
                  value={operation.width}
                  onChange={(e) => onUpdate({ width: parseFloat(e.target.value) })}
                />
              </div>
              <div>
                <label className="text-xs text-text-muted">Height</label>
                <input
                  type="number"
                  className="w-full bg-surface border border-border rounded px-2 py-1 text-sm"
                  value={operation.height}
                  onChange={(e) => onUpdate({ height: parseFloat(e.target.value) })}
                />
              </div>
            </div>
            <div>
              <label className="text-xs text-text-muted">Stock to Leave</label>
              <input
                type="number"
                step="0.1"
                className="w-full bg-surface border border-border rounded px-2 py-1 text-sm"
                value={operation.stockToLeave}
                onChange={(e) => onUpdate({ stockToLeave: parseFloat(e.target.value) })}
              />
            </div>
          </>
        );

      case "pocket_circle":
        return (
          <>
            <div className="grid grid-cols-2 gap-2">
              <div>
                <label className="text-xs text-text-muted">Center X</label>
                <input
                  type="number"
                  className="w-full bg-surface border border-border rounded px-2 py-1 text-sm"
                  value={operation.centerX}
                  onChange={(e) => onUpdate({ centerX: parseFloat(e.target.value) })}
                />
              </div>
              <div>
                <label className="text-xs text-text-muted">Center Y</label>
                <input
                  type="number"
                  className="w-full bg-surface border border-border rounded px-2 py-1 text-sm"
                  value={operation.centerY}
                  onChange={(e) => onUpdate({ centerY: parseFloat(e.target.value) })}
                />
              </div>
            </div>
            <div>
              <label className="text-xs text-text-muted">Radius</label>
              <input
                type="number"
                className="w-full bg-surface border border-border rounded px-2 py-1 text-sm"
                value={operation.radius}
                onChange={(e) => onUpdate({ radius: parseFloat(e.target.value) })}
              />
            </div>
          </>
        );

      case "contour":
        return (
          <>
            <div className="grid grid-cols-2 gap-2">
              <div>
                <label className="text-xs text-text-muted">X</label>
                <input
                  type="number"
                  className="w-full bg-surface border border-border rounded px-2 py-1 text-sm"
                  value={operation.x}
                  onChange={(e) => onUpdate({ x: parseFloat(e.target.value) })}
                />
              </div>
              <div>
                <label className="text-xs text-text-muted">Y</label>
                <input
                  type="number"
                  className="w-full bg-surface border border-border rounded px-2 py-1 text-sm"
                  value={operation.y}
                  onChange={(e) => onUpdate({ y: parseFloat(e.target.value) })}
                />
              </div>
              <div>
                <label className="text-xs text-text-muted">Width</label>
                <input
                  type="number"
                  className="w-full bg-surface border border-border rounded px-2 py-1 text-sm"
                  value={operation.width}
                  onChange={(e) => onUpdate({ width: parseFloat(e.target.value) })}
                />
              </div>
              <div>
                <label className="text-xs text-text-muted">Height</label>
                <input
                  type="number"
                  className="w-full bg-surface border border-border rounded px-2 py-1 text-sm"
                  value={operation.height}
                  onChange={(e) => onUpdate({ height: parseFloat(e.target.value) })}
                />
              </div>
            </div>
            <div>
              <label className="text-xs text-text-muted">Offset</label>
              <input
                type="number"
                step="0.1"
                className="w-full bg-surface border border-border rounded px-2 py-1 text-sm"
                value={operation.offset}
                onChange={(e) => onUpdate({ offset: parseFloat(e.target.value) })}
              />
            </div>
            <div className="grid grid-cols-3 gap-2">
              <div>
                <label className="text-xs text-text-muted">Tab Count</label>
                <input
                  type="number"
                  min="0"
                  className="w-full bg-surface border border-border rounded px-2 py-1 text-sm"
                  value={operation.tabCount}
                  onChange={(e) => onUpdate({ tabCount: parseInt(e.target.value) })}
                />
              </div>
              <div>
                <label className="text-xs text-text-muted">Tab Width</label>
                <input
                  type="number"
                  step="0.5"
                  className="w-full bg-surface border border-border rounded px-2 py-1 text-sm"
                  value={operation.tabWidth}
                  onChange={(e) => onUpdate({ tabWidth: parseFloat(e.target.value) })}
                />
              </div>
              <div>
                <label className="text-xs text-text-muted">Tab Height</label>
                <input
                  type="number"
                  step="0.5"
                  className="w-full bg-surface border border-border rounded px-2 py-1 text-sm"
                  value={operation.tabHeight}
                  onChange={(e) => onUpdate({ tabHeight: parseFloat(e.target.value) })}
                />
              </div>
            </div>
          </>
        );

      case "roughing3d":
        return (
          <>
            <div className="grid grid-cols-2 gap-2">
              <div>
                <label className="text-xs text-text-muted">Top Z</label>
                <input
                  type="number"
                  className="w-full bg-surface border border-border rounded px-2 py-1 text-sm"
                  value={operation.topZ}
                  onChange={(e) => onUpdate({ topZ: parseFloat(e.target.value) })}
                />
              </div>
              <div>
                <label className="text-xs text-text-muted">Target Z</label>
                <input
                  type="number"
                  className="w-full bg-surface border border-border rounded px-2 py-1 text-sm"
                  value={operation.targetZ}
                  onChange={(e) => onUpdate({ targetZ: parseFloat(e.target.value) })}
                />
              </div>
            </div>
            <div className="grid grid-cols-2 gap-2">
              <div>
                <label className="text-xs text-text-muted">Stock Margin</label>
                <input
                  type="number"
                  step="0.1"
                  className="w-full bg-surface border border-border rounded px-2 py-1 text-sm"
                  value={operation.stockMargin}
                  onChange={(e) => onUpdate({ stockMargin: parseFloat(e.target.value) })}
                />
              </div>
              <div>
                <label className="text-xs text-text-muted">Direction (°)</label>
                <input
                  type="number"
                  step="45"
                  className="w-full bg-surface border border-border rounded px-2 py-1 text-sm"
                  value={operation.direction}
                  onChange={(e) => onUpdate({ direction: parseFloat(e.target.value) })}
                />
              </div>
            </div>
            <div className="text-xs text-text-muted mt-2">
              Note: 3D roughing requires a part with tessellated mesh.
            </div>
          </>
        );
    }
  };

  return (
    <div className="space-y-2 p-2 bg-surface-secondary rounded text-sm">
      <div>
        <label className="text-xs text-text-muted">Name</label>
        <input
          type="text"
          className="w-full bg-surface border border-border rounded px-2 py-1"
          value={operation.name}
          onChange={(e) => onUpdate({ name: e.target.value })}
        />
      </div>
      <div>
        <label className="text-xs text-text-muted">Depth</label>
        <input
          type="number"
          step="0.5"
          className="w-full bg-surface border border-border rounded px-2 py-1"
          value={operation.depth}
          onChange={(e) => onUpdate({ depth: parseFloat(e.target.value) })}
        />
      </div>
      <ToolLibrary compact />
      {renderFields()}
    </div>
  );
}

export function OperationList() {
  const operations = useCamStore((s) => s.operations);
  const selectedOperationId = useCamStore((s) => s.selectedOperationId);
  const selectedToolId = useCamStore((s) => s.selectedToolId);
  const tools = useCamStore((s) => s.tools);
  const selectTool = useCamStore((s) => s.selectTool);
  const selectOperation = useCamStore((s) => s.selectOperation);
  const addOperation = useCamStore((s) => s.addOperation);
  const updateOperation = useCamStore((s) => s.updateOperation);
  const removeOperation = useCamStore((s) => s.removeOperation);
  const moveOperation = useCamStore((s) => s.moveOperation);

  const [showAddForm, setShowAddForm] = useState(false);

  const handleAddOperation = (type: CamOperationType) => {
    const baseName = OPERATION_LABELS[type];
    const count = operations.filter((op) => op.type === type).length + 1;

    // Use selected tool, or auto-select first tool if none selected
    let toolId = selectedToolId;
    if (!toolId && tools.length > 0) {
      toolId = tools[0]!.id;
      selectTool(toolId);
    }

    const baseOp = {
      name: `${baseName} ${count}`,
      type,
      toolId: toolId ?? "",
      depth: 5.0,
      enabled: true,
    };

    switch (type) {
      case "face":
        addOperation({
          ...baseOp,
          type: "face",
          minX: 0,
          minY: 0,
          maxX: 100,
          maxY: 50,
        });
        break;
      case "pocket":
        addOperation({
          ...baseOp,
          type: "pocket",
          x: 10,
          y: 10,
          width: 30,
          height: 20,
          stockToLeave: 0,
        });
        break;
      case "pocket_circle":
        addOperation({
          ...baseOp,
          type: "pocket_circle",
          centerX: 25,
          centerY: 25,
          radius: 15,
        });
        break;
      case "contour":
        addOperation({
          ...baseOp,
          type: "contour",
          x: 0,
          y: 0,
          width: 50,
          height: 40,
          offset: 0,
          tabCount: 4,
          tabWidth: 5,
          tabHeight: 2,
        });
        break;
      case "roughing3d":
        addOperation({
          ...baseOp,
          type: "roughing3d",
          targetZ: -10,
          topZ: 0,
          stockMargin: 0.5,
          direction: 0,
        });
        break;
    }

    setShowAddForm(false);
  };

  const selectedOperation = operations.find((op) => op.id === selectedOperationId);

  return (
    <div className="space-y-2">
      <div className="flex items-center justify-between">
        <h3 className="text-sm font-medium">Operations</h3>
        <button
          className="p-1 hover:bg-hover rounded text-text-muted hover:text-text"
          onClick={() => setShowAddForm(!showAddForm)}
        >
          <Plus size={14} />
        </button>
      </div>

      {showAddForm && (
        <AddOperationForm
          onAdd={handleAddOperation}
          onCancel={() => setShowAddForm(false)}
        />
      )}

      {operations.length === 0 && !showAddForm && (
        <div className="text-center text-text-muted text-sm py-4">
          No operations yet. Click + to add one.
        </div>
      )}

      <div className="space-y-1">
        {operations.map((op, index) => {
          const Icon = OPERATION_ICONS[op.type];
          return (
            <div
              key={op.id}
              className={cn(
                "flex items-center gap-2 p-2 rounded cursor-pointer text-sm",
                "hover:bg-hover transition-colors",
                selectedOperationId === op.id && "bg-accent/20 border border-accent/40"
              )}
              onClick={() => selectOperation(op.id)}
            >
              <button
                className="p-1 hover:bg-hover rounded"
                onClick={(e) => {
                  e.stopPropagation();
                  updateOperation(op.id, { enabled: !op.enabled });
                }}
              >
                {op.enabled ? (
                  <Eye size={14} className="text-text-muted" />
                ) : (
                  <EyeSlash size={14} className="text-text-muted" />
                )}
              </button>
              <Icon size={14} className="text-text-muted" />
              <div className="flex-1 min-w-0">
                <div className="truncate">{op.name}</div>
                <div className="text-xs text-text-muted">
                  {OPERATION_LABELS[op.type]}, depth: {op.depth}mm
                </div>
              </div>
              <div className="flex items-center gap-1">
                <button
                  className="p-1 hover:bg-hover rounded text-text-muted"
                  onClick={(e) => {
                    e.stopPropagation();
                    moveOperation(op.id, "up");
                  }}
                  disabled={index === 0}
                >
                  <ArrowUp size={12} />
                </button>
                <button
                  className="p-1 hover:bg-hover rounded text-text-muted"
                  onClick={(e) => {
                    e.stopPropagation();
                    moveOperation(op.id, "down");
                  }}
                  disabled={index === operations.length - 1}
                >
                  <ArrowDown size={12} />
                </button>
                <button
                  className="p-1 hover:bg-error/20 rounded text-text-muted hover:text-error"
                  onClick={(e) => {
                    e.stopPropagation();
                    removeOperation(op.id);
                  }}
                >
                  <Trash size={12} />
                </button>
              </div>
            </div>
          );
        })}
      </div>

      {selectedOperation && (
        <OperationEditor
          operation={selectedOperation}
          onUpdate={(updates) => updateOperation(selectedOperation.id, updates)}
        />
      )}
    </div>
  );
}
