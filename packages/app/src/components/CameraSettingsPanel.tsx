import { cn } from "@/lib/utils";
import {
  useCameraSettingsStore,
  getEffectiveInputDevice,
} from "@/stores/camera-settings-store";
import {
  CONTROL_PRESETS,
  type InputDevice,
} from "@/types/camera-controls";

interface CameraSettingsPanelProps {
  className?: string;
}

export function CameraSettingsPanel({ className }: CameraSettingsPanelProps) {
  const {
    controlSchemeId,
    inputDevice,
    detectedDevice,
    zoomBehavior,
    orbitMomentum,
    setControlScheme,
    setInputDevice,
    setZoomBehavior,
    setOrbitMomentum,
    resetToDefaults,
  } = useCameraSettingsStore();

  const effectiveDevice = getEffectiveInputDevice(useCameraSettingsStore.getState());

  const presets = Object.values(CONTROL_PRESETS);

  return (
    <div className={cn("flex flex-col gap-4", className)}>
      {/* Control Scheme Presets */}
      <div>
        <div className="text-[10px] font-bold uppercase tracking-wider text-text-muted mb-2">
          Control Scheme
        </div>
        <div className="flex flex-col gap-1">
          {presets.map((preset) => (
            <label
              key={preset.id}
              className={cn(
                "flex items-start gap-2 px-2 py-1.5 cursor-pointer hover:bg-hover",
                controlSchemeId === preset.id && "bg-hover",
              )}
            >
              <input
                type="radio"
                name="controlScheme"
                value={preset.id}
                checked={controlSchemeId === preset.id}
                onChange={() => setControlScheme(preset.id)}
                className="mt-0.5 accent-brand"
              />
              <div className="flex-1 min-w-0">
                <div className="text-xs text-text">{preset.name}</div>
                <div className="text-[10px] text-text-muted truncate">
                  {preset.description}
                </div>
              </div>
            </label>
          ))}
        </div>
      </div>

      {/* Divider */}
      <div className="border-t border-border" />

      {/* Input Device */}
      <div>
        <div className="text-[10px] font-bold uppercase tracking-wider text-text-muted mb-2">
          Input Device
        </div>
        <div className="flex gap-1">
          {(["auto", "mouse", "trackpad"] as InputDevice[]).map((device) => (
            <button
              key={device}
              onClick={() => setInputDevice(device)}
              className={cn(
                "flex-1 px-2 py-1.5 text-xs border border-border",
                inputDevice === device
                  ? "bg-brand text-white border-brand"
                  : "text-text hover:bg-hover",
              )}
            >
              {device === "auto"
                ? `Auto${detectedDevice ? ` (${detectedDevice})` : ""}`
                : device.charAt(0).toUpperCase() + device.slice(1)}
            </button>
          ))}
        </div>
        {inputDevice === "auto" && (
          <div className="text-[10px] text-text-muted mt-1">
            Currently using: {effectiveDevice}
          </div>
        )}
      </div>

      {/* Divider */}
      <div className="border-t border-border" />

      {/* Zoom Behavior */}
      <div>
        <div className="text-[10px] font-bold uppercase tracking-wider text-text-muted mb-2">
          Zoom Behavior
        </div>
        <div className="flex flex-col gap-2">
          <label className="flex items-center gap-2 px-2 py-1 cursor-pointer hover:bg-hover">
            <input
              type="checkbox"
              checked={zoomBehavior.zoomTowardsCursor}
              onChange={(e) =>
                setZoomBehavior({ zoomTowardsCursor: e.target.checked })
              }
              className="accent-brand"
            />
            <span className="text-xs text-text">Zoom toward cursor</span>
          </label>
          <label className="flex items-center gap-2 px-2 py-1 cursor-pointer hover:bg-hover">
            <input
              type="checkbox"
              checked={zoomBehavior.invertDirection}
              onChange={(e) =>
                setZoomBehavior({ invertDirection: e.target.checked })
              }
              className="accent-brand"
            />
            <span className="text-xs text-text">Invert zoom direction</span>
          </label>
          <div className="px-2">
            <div className="flex items-center justify-between mb-1">
              <span className="text-xs text-text">Sensitivity</span>
              <span className="text-[10px] text-text-muted">
                {zoomBehavior.sensitivity.toFixed(1)}x
              </span>
            </div>
            <input
              type="range"
              min="0.5"
              max="2.0"
              step="0.1"
              value={zoomBehavior.sensitivity}
              onChange={(e) =>
                setZoomBehavior({ sensitivity: parseFloat(e.target.value) })
              }
              className="w-full h-1 bg-border rounded appearance-none cursor-pointer accent-brand"
            />
          </div>
        </div>
      </div>

      {/* Divider */}
      <div className="border-t border-border" />

      {/* Orbit Momentum */}
      <div>
        <div className="text-[10px] font-bold uppercase tracking-wider text-text-muted mb-2">
          Trackpad
        </div>
        <label className="flex items-center gap-2 px-2 py-1 cursor-pointer hover:bg-hover">
          <input
            type="checkbox"
            checked={orbitMomentum}
            onChange={(e) => setOrbitMomentum(e.target.checked)}
            className="accent-brand"
          />
          <span className="text-xs text-text">Orbit momentum</span>
        </label>
        <div className="text-[10px] text-text-muted px-2 mt-1">
          Continue orbiting after releasing trackpad
        </div>
      </div>

      {/* Divider */}
      <div className="border-t border-border" />

      {/* Reset */}
      <button
        onClick={resetToDefaults}
        className="text-xs text-text-muted hover:text-text px-2 py-1 hover:bg-hover text-left"
      >
        Reset to defaults
      </button>
    </div>
  );
}
