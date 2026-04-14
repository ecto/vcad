/**
 * Floating panel for length tuning / meander parameters.
 * Shown when the PCB tool is "length-tune".
 */

import { useElectronicsStore } from "@/stores/electronics-store";
import type { MeanderStyle } from "@vcad/ir";

export function LengthTunePanel() {
  const pcbTool = useElectronicsStore((s) => s.pcbTool);
  const params = useElectronicsStore((s) => s.lengthTuneParams);
  const lengthTuneNet = useElectronicsStore((s) => s.lengthTuneNet);
  const setParams = useElectronicsStore((s) => s.setLengthTuneParams);
  const cancel = useElectronicsStore((s) => s.cancelLengthTune);

  if (pcbTool !== "length-tune") return null;

  return (
    <div className="absolute top-3 left-3 w-56 rounded-lg border border-border bg-surface/95 backdrop-blur-sm shadow-lg p-3 text-[11px] pointer-events-auto">
      <div className="flex justify-between items-center mb-2">
        <span className="font-medium text-text">Length Tuning</span>
        <button
          className="px-1.5 py-0.5 text-[10px] text-text-muted hover:text-text rounded transition-colors"
          onClick={cancel}
        >
          Done
        </button>
      </div>

      {lengthTuneNet && (
        <div className="text-[10px] text-brand mb-2">Net: {lengthTuneNet}</div>
      )}

      <div className="space-y-2 text-text-muted">
        {/* Target length */}
        <div className="flex justify-between items-center">
          <span>Target (mm)</span>
          <input
            type="number"
            value={params.target_length}
            onChange={(e) =>
              setParams({ target_length: parseFloat(e.target.value) || 0 })
            }
            className="w-20 text-[11px] text-text bg-transparent border border-border rounded px-1 py-0.5 text-right"
            step={1}
            min={0}
          />
        </div>

        {/* Max amplitude */}
        <div>
          <div className="flex justify-between items-center mb-0.5">
            <span>Amplitude (mm)</span>
            <span className="text-text">{params.max_amplitude.toFixed(1)}</span>
          </div>
          <input
            type="range"
            value={params.max_amplitude}
            onChange={(e) =>
              setParams({ max_amplitude: parseFloat(e.target.value) })
            }
            className="w-full h-1 accent-brand"
            min={0.1}
            max={10}
            step={0.1}
          />
        </div>

        {/* Spacing */}
        <div>
          <div className="flex justify-between items-center mb-0.5">
            <span>Spacing (mm)</span>
            <span className="text-text">{params.spacing.toFixed(1)}</span>
          </div>
          <input
            type="range"
            value={params.spacing}
            onChange={(e) =>
              setParams({ spacing: parseFloat(e.target.value) })
            }
            className="w-full h-1 accent-brand"
            min={0.1}
            max={5}
            step={0.1}
          />
        </div>

        {/* Style dropdown */}
        <div className="flex justify-between items-center">
          <span>Style</span>
          <select
            value={params.style}
            onChange={(e) =>
              setParams({ style: e.target.value as MeanderStyle })
            }
            className="text-[11px] text-text bg-transparent border border-border rounded px-1 py-0.5"
          >
            <option value="Trombone">Trombone</option>
            <option value="Sawtooth">Sawtooth</option>
          </select>
        </div>
      </div>
    </div>
  );
}
