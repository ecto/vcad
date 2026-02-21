import { MagnifyingGlass } from "@phosphor-icons/react/dist/ssr/MagnifyingGlass";
import { Spinner } from "@phosphor-icons/react/dist/ssr/Spinner";
import { WifiHigh } from "@phosphor-icons/react/dist/ssr/WifiHigh";
import { WifiSlash } from "@phosphor-icons/react/dist/ssr/WifiSlash";
import { usePrinterStore } from "@/stores/printer-store";

export function PrinterSelect() {
  const isDiscovering = usePrinterStore((s) => s.isDiscovering);
  const discoveredPrinters = usePrinterStore((s) => s.discoveredPrinters);
  const selectedPrinter = usePrinterStore((s) => s.selectedPrinter);
  const connectionState = usePrinterStore((s) => s.connectionState);
  const selectPrinter = usePrinterStore((s) => s.selectPrinter);
  const setDiscovering = usePrinterStore((s) => s.setDiscovering);
  const setDiscoveredPrinters = usePrinterStore((s) => s.setDiscoveredPrinters);
  const profiles = usePrinterStore((s) => s.profiles);
  const selectedProfile = usePrinterStore((s) => s.selectedProfile);
  const setSelectedProfile = usePrinterStore((s) => s.setSelectedProfile);

  async function handleDiscover() {
    setDiscovering(true);
    // In real implementation, this would call the WASM/native printer discovery
    // For now, simulate discovery
    await new Promise((r) => setTimeout(r, 2000));
    setDiscoveredPrinters([
      {
        id: "mock-1",
        name: "Bambu X1C - Workshop",
        model: "X1C",
        ip: "192.168.1.100",
        serial: "00M00A2B012345",
      },
    ]);
    setDiscovering(false);
  }

  return (
    <div className="space-y-3">
      {/* Printer Profile Selection */}
      <div>
        <label className="block text-sm text-text-muted mb-1">Printer Profile</label>
        <select
          value={selectedProfile}
          onChange={(e) => setSelectedProfile(e.target.value)}
          className="w-full h-8 px-2 text-sm bg-surface border border-border rounded text-text"
        >
          {profiles.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
        </select>
      </div>

      {/* Network Printer (optional) */}
      <div className="border-t border-border pt-3">
        <div className="flex items-center justify-between mb-2">
          <span className="text-sm text-text-muted">Network Printer</span>
          <button
            onClick={handleDiscover}
            disabled={isDiscovering}
            className="flex items-center gap-1 px-2 py-1 text-xs bg-hover hover:bg-border rounded disabled:opacity-50"
          >
            {isDiscovering ? (
              <Spinner className="animate-spin" size={14} />
            ) : (
              <MagnifyingGlass size={14} />
            )}
            {isDiscovering ? "Scanning..." : "Discover"}
          </button>
        </div>

        {/* Discovered printers */}
        {discoveredPrinters.length > 0 && (
          <div className="space-y-1">
            {discoveredPrinters.map((printer) => (
              <button
                key={printer.id}
                onClick={() => selectPrinter(printer)}
                className={`w-full flex items-center gap-2 p-2 text-left text-sm rounded ${
                  selectedPrinter?.id === printer.id
                    ? "bg-accent text-white"
                    : "bg-hover hover:bg-border"
                }`}
              >
                {connectionState === "connected" && selectedPrinter?.id === printer.id ? (
                  <WifiHigh size={16} className="text-green-400" />
                ) : (
                  <WifiSlash size={16} className="text-text-muted" />
                )}
                <div className="flex-1 min-w-0">
                  <div className="truncate">{printer.name}</div>
                  <div className="text-xs opacity-70 truncate">{printer.ip}</div>
                </div>
              </button>
            ))}
          </div>
        )}

        {/* No printers found message */}
        {!isDiscovering && discoveredPrinters.length === 0 && (
          <div className="text-xs text-text-muted text-center py-2">
            No printers found. Make sure your printer is on the same network.
          </div>
        )}
      </div>
    </div>
  );
}
