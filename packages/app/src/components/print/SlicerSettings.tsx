import { useSlicerStore, type InfillPattern } from "@/stores/slicer-store";

const INFILL_PATTERNS: { value: InfillPattern; label: string }[] = [
  { value: "grid", label: "Grid" },
  { value: "lines", label: "Lines" },
  { value: "triangles", label: "Triangles" },
  { value: "honeycomb", label: "Honeycomb" },
  { value: "gyroid", label: "Gyroid" },
];

const LAYER_HEIGHTS = [0.1, 0.15, 0.2, 0.25, 0.3, 0.4];

export function SlicerSettings() {
  const settings = useSlicerStore((s) => s.settings);
  const setSettings = useSlicerStore((s) => s.setSettings);

  return (
    <div className="space-y-4">
      {/* Layer Height */}
      <div>
        <label className="block text-sm text-text-muted mb-1">Layer Height</label>
        <select
          value={settings.layerHeight}
          onChange={(e) => setSettings({ layerHeight: parseFloat(e.target.value) })}
          className="w-full h-8 px-2 text-sm bg-surface border border-border rounded text-text"
        >
          {LAYER_HEIGHTS.map((h) => (
            <option key={h} value={h}>
              {h}mm
            </option>
          ))}
        </select>
      </div>

      {/* Infill Density */}
      <div>
        <label className="block text-sm text-text-muted mb-1">
          Infill: {Math.round(settings.infillDensity * 100)}%
        </label>
        <input
          type="range"
          min="0"
          max="100"
          value={settings.infillDensity * 100}
          onChange={(e) => setSettings({ infillDensity: parseInt(e.target.value) / 100 })}
          className="w-full h-2 accent-brand"
        />
      </div>

      {/* Infill Pattern */}
      <div>
        <label className="block text-sm text-text-muted mb-1">Pattern</label>
        <select
          value={settings.infillPattern}
          onChange={(e) => setSettings({ infillPattern: e.target.value as InfillPattern })}
          className="w-full h-8 px-2 text-sm bg-surface border border-border rounded text-text"
        >
          {INFILL_PATTERNS.map((p) => (
            <option key={p.value} value={p.value}>
              {p.label}
            </option>
          ))}
        </select>
      </div>

      {/* Wall Count */}
      <div>
        <label className="block text-sm text-text-muted mb-1">
          Walls: {settings.wallCount}
        </label>
        <input
          type="range"
          min="1"
          max="6"
          value={settings.wallCount}
          onChange={(e) => setSettings({ wallCount: parseInt(e.target.value) })}
          className="w-full h-2 accent-brand"
        />
      </div>

      {/* Support */}
      <div className="flex items-center gap-2">
        <input
          type="checkbox"
          id="support-enabled"
          checked={settings.supportEnabled}
          onChange={(e) => setSettings({ supportEnabled: e.target.checked })}
          className="w-4 h-4 accent-brand"
        />
        <label htmlFor="support-enabled" className="text-sm text-text">
          Enable Support
        </label>
      </div>

      {settings.supportEnabled && (
        <div className="pl-6">
          <label className="block text-sm text-text-muted mb-1">
            Overhang Angle: {settings.supportAngle}°
          </label>
          <input
            type="range"
            min="0"
            max="90"
            value={settings.supportAngle}
            onChange={(e) => setSettings({ supportAngle: parseInt(e.target.value) })}
            className="w-full h-2 accent-brand"
          />
        </div>
      )}
    </div>
  );
}
