import { Thermometer } from "@phosphor-icons/react/dist/ssr/Thermometer";
import { Clock } from "@phosphor-icons/react/dist/ssr/Clock";
import { Stack } from "@phosphor-icons/react/dist/ssr/Stack";
import { usePrinterStore, formatMinutes } from "@/stores/printer-store";

export function PrinterStatus() {
  const status = usePrinterStore((s) => s.status);
  const selectedPrinter = usePrinterStore((s) => s.selectedPrinter);
  const connectionState = usePrinterStore((s) => s.connectionState);

  if (!selectedPrinter || connectionState !== "connected" || !status) {
    return null;
  }

  const isActive = status.state === "printing" || status.state === "paused";

  return (
    <div className="space-y-3 border-t border-border pt-3">
      {/* Current state */}
      <div className="flex items-center gap-2">
        <div
          className={`w-2 h-2 rounded-full ${
            status.state === "printing"
              ? "bg-green-500 animate-pulse"
              : status.state === "paused"
              ? "bg-yellow-500"
              : status.state === "error"
              ? "bg-red-500"
              : "bg-text-muted"
          }`}
        />
        <span className="text-sm capitalize">{status.state}</span>
        {status.filename && (
          <span className="text-xs text-text-muted truncate flex-1">{status.filename}</span>
        )}
      </div>

      {/* Progress bar */}
      {isActive && (
        <div>
          <div className="flex justify-between text-xs text-text-muted mb-1">
            <span>{status.progressPercent.toFixed(1)}%</span>
            <span>{formatMinutes(status.timeRemainingMin)} remaining</span>
          </div>
          <div className="h-2 bg-hover rounded-full overflow-hidden">
            <div
              className="h-full bg-brand transition-all"
              style={{ width: `${status.progressPercent}%` }}
            />
          </div>
        </div>
      )}

      {/* Stats grid */}
      <div className="grid grid-cols-2 gap-2 text-sm">
        {/* Layer */}
        {isActive && (
          <div className="flex items-center gap-2">
            <Stack size={16} className="text-text-muted" />
            <span>
              {status.layerCurrent} / {status.layerTotal}
            </span>
          </div>
        )}

        {/* Time elapsed */}
        {isActive && (
          <div className="flex items-center gap-2">
            <Clock size={16} className="text-text-muted" />
            <span>{formatMinutes(status.timeElapsedMin)}</span>
          </div>
        )}

        {/* Nozzle temp */}
        <div className="flex items-center gap-2">
          <Thermometer size={16} className="text-orange-400" />
          <span>
            {status.nozzleTemp.toFixed(0)}°C
            {status.nozzleTarget > 0 && (
              <span className="text-text-muted"> / {status.nozzleTarget}°C</span>
            )}
          </span>
        </div>

        {/* Bed temp */}
        <div className="flex items-center gap-2">
          <Thermometer size={16} className="text-blue-400" />
          <span>
            {status.bedTemp.toFixed(0)}°C
            {status.bedTarget > 0 && (
              <span className="text-text-muted"> / {status.bedTarget}°C</span>
            )}
          </span>
        </div>
      </div>
    </div>
  );
}
