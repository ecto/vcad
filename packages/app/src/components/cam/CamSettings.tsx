import { useCamStore } from "@/stores/cam-store";
import { ArrowCounterClockwise } from "@phosphor-icons/react/dist/ssr/ArrowCounterClockwise";

export function CamSettings() {
  const settings = useCamStore((s) => s.settings);
  const setSettings = useCamStore((s) => s.setSettings);
  const resetSettings = useCamStore((s) => s.resetSettings);

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between">
        <h3 className="text-sm font-medium">CAM Settings</h3>
        <button
          className="p-1 hover:bg-hover rounded text-text-muted hover:text-text"
          onClick={resetSettings}
          title="Reset to defaults"
        >
          <ArrowCounterClockwise size={14} />
        </button>
      </div>

      <div className="space-y-2 text-sm">
        <div className="grid grid-cols-2 gap-2">
          <div>
            <label className="text-xs text-text-muted">Stepover (mm)</label>
            <input
              type="number"
              step="0.5"
              className="w-full bg-surface border border-border rounded px-2 py-1"
              value={settings.stepover}
              onChange={(e) => setSettings({ stepover: parseFloat(e.target.value) })}
            />
          </div>
          <div>
            <label className="text-xs text-text-muted">Stepdown (mm)</label>
            <input
              type="number"
              step="0.5"
              className="w-full bg-surface border border-border rounded px-2 py-1"
              value={settings.stepdown}
              onChange={(e) => setSettings({ stepdown: parseFloat(e.target.value) })}
            />
          </div>
        </div>

        <div className="grid grid-cols-2 gap-2">
          <div>
            <label className="text-xs text-text-muted">Feed Rate (mm/min)</label>
            <input
              type="number"
              step="100"
              className="w-full bg-surface border border-border rounded px-2 py-1"
              value={settings.feedRate}
              onChange={(e) => setSettings({ feedRate: parseFloat(e.target.value) })}
            />
          </div>
          <div>
            <label className="text-xs text-text-muted">Plunge Rate (mm/min)</label>
            <input
              type="number"
              step="50"
              className="w-full bg-surface border border-border rounded px-2 py-1"
              value={settings.plungeRate}
              onChange={(e) => setSettings({ plungeRate: parseFloat(e.target.value) })}
            />
          </div>
        </div>

        <div>
          <label className="text-xs text-text-muted">Spindle RPM</label>
          <input
            type="number"
            step="1000"
            className="w-full bg-surface border border-border rounded px-2 py-1"
            value={settings.spindleRpm}
            onChange={(e) => setSettings({ spindleRpm: parseFloat(e.target.value) })}
          />
        </div>

        <div className="grid grid-cols-2 gap-2">
          <div>
            <label className="text-xs text-text-muted">Safe Z (mm)</label>
            <input
              type="number"
              step="1"
              className="w-full bg-surface border border-border rounded px-2 py-1"
              value={settings.safeZ}
              onChange={(e) => setSettings({ safeZ: parseFloat(e.target.value) })}
            />
          </div>
          <div>
            <label className="text-xs text-text-muted">Retract Z (mm)</label>
            <input
              type="number"
              step="1"
              className="w-full bg-surface border border-border rounded px-2 py-1"
              value={settings.retractZ}
              onChange={(e) => setSettings({ retractZ: parseFloat(e.target.value) })}
            />
          </div>
        </div>
      </div>
    </div>
  );
}
