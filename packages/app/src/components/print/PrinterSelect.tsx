import { useEffect, useState } from "react";
import { MagnifyingGlass } from "@phosphor-icons/react/dist/ssr/MagnifyingGlass";
import { Spinner } from "@phosphor-icons/react/dist/ssr/Spinner";
import { WifiHigh } from "@phosphor-icons/react/dist/ssr/WifiHigh";
import { WifiSlash } from "@phosphor-icons/react/dist/ssr/WifiSlash";
import { usePrinterStore } from "@/stores/printer-store";
import { isRelayAvailable, discoverPrinters, connectPrinter } from "@/lib/print-relay";
import { useNotificationStore } from "@/stores/notification-store";

const ACCESS_CODE_KEY_PREFIX = "bambu-access-code:";

export function PrinterSelect() {
  const isDiscovering = usePrinterStore((s) => s.isDiscovering);
  const discoveredPrinters = usePrinterStore((s) => s.discoveredPrinters);
  const selectedPrinter = usePrinterStore((s) => s.selectedPrinter);
  const connectionState = usePrinterStore((s) => s.connectionState);
  const selectPrinter = usePrinterStore((s) => s.selectPrinter);
  const setDiscovering = usePrinterStore((s) => s.setDiscovering);
  const setDiscoveredPrinters = usePrinterStore((s) => s.setDiscoveredPrinters);
  const setConnectionState = usePrinterStore((s) => s.setConnectionState);
  const setConnectionError = usePrinterStore((s) => s.setConnectionError);
  const profiles = usePrinterStore((s) => s.profiles);
  const selectedProfile = usePrinterStore((s) => s.selectedProfile);
  const setSelectedProfile = usePrinterStore((s) => s.setSelectedProfile);
  const addToast = useNotificationStore((s) => s.addToast);
  const [relayAvailable, setRelayAvailable] = useState<boolean | null>(null);
  const [accessCode, setAccessCode] = useState("");

  // Pre-fill access code from localStorage when a printer is selected.
  useEffect(() => {
    if (!selectedPrinter) {
      setAccessCode("");
      return;
    }
    const stored = localStorage.getItem(ACCESS_CODE_KEY_PREFIX + selectedPrinter.serial);
    setAccessCode(stored ?? "");
  }, [selectedPrinter]);

  async function handleConnect() {
    if (!selectedPrinter) return;
    const code = accessCode.trim();
    if (code.length === 0) {
      addToast("Enter the printer's access code", "error");
      return;
    }

    setConnectionState("connecting");
    setConnectionError(null);
    try {
      await connectPrinter(selectedPrinter.ip, selectedPrinter.serial, code);
      localStorage.setItem(ACCESS_CODE_KEY_PREFIX + selectedPrinter.serial, code);
      setConnectionState("connected");
      addToast(`Connected to ${selectedPrinter.name}`, "success");
    } catch (err) {
      const message = err instanceof Error ? err.message : "Connect failed";
      setConnectionState("error");
      setConnectionError(message);
      addToast(message, "error");
    }
  }

  async function handleDiscover() {
    setDiscovering(true);

    try {
      // Check if relay server is available
      const available = await isRelayAvailable();
      setRelayAvailable(available);

      if (available) {
        // Real discovery via relay
        const printers = await discoverPrinters();
        setDiscoveredPrinters(
          printers.map((p) => ({
            id: `${p.serial}`,
            name: `${p.model} - ${p.name}`,
            model: p.model,
            ip: p.ip,
            serial: p.serial,
          }))
        );
        if (printers.length === 0) {
          addToast("No printers found on network", "info");
        }
      } else {
        // No relay — tell user to start it
        setDiscoveredPrinters([]);
        addToast(
          "Print relay not running. Start with: vcad print-server",
          "info"
        );
      }
    } catch (err) {
      console.error("Discovery failed:", err);
      addToast("Printer discovery failed", "error");
      setDiscoveredPrinters([]);
    } finally {
      setDiscovering(false);
    }
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

        {/* Relay status */}
        {relayAvailable === false && (
          <div className="text-xs text-text-muted text-center py-1 mb-2 bg-hover rounded p-2">
            Print relay not running. Start with:
            <code className="block mt-1 text-brand">vcad print-server</code>
          </div>
        )}

        {/* Discovered printers */}
        {discoveredPrinters.length > 0 && (
          <div className="space-y-1">
            {discoveredPrinters.map((printer) => (
              <button
                key={printer.id}
                onClick={() => selectPrinter(printer)}
                className={`w-full flex items-center gap-2 p-2 text-left text-sm rounded ${
                  selectedPrinter?.id === printer.id
                    ? "bg-brand text-white"
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
        {!isDiscovering && discoveredPrinters.length === 0 && relayAvailable !== false && (
          <div className="text-xs text-text-muted text-center py-2">
            No printers found. Make sure your printer is on the same network.
          </div>
        )}

        {/* Access code + Connect for the selected printer */}
        {selectedPrinter && (
          <div className="mt-3 space-y-2">
            <label className="block text-xs text-text-muted">
              Access Code
              <span className="ml-1 opacity-70">(Settings → WLAN on the printer)</span>
            </label>
            <input
              type="text"
              inputMode="numeric"
              autoComplete="off"
              maxLength={16}
              value={accessCode}
              onChange={(e) => setAccessCode(e.target.value)}
              placeholder="12345678"
              className="w-full h-8 px-2 text-sm bg-surface border border-border rounded text-text font-mono"
            />
            <button
              onClick={handleConnect}
              disabled={connectionState === "connecting" || accessCode.trim().length === 0}
              className="w-full flex items-center justify-center gap-2 py-2 bg-brand hover:bg-brand/90 disabled:opacity-50 disabled:cursor-not-allowed text-white rounded text-sm"
            >
              {connectionState === "connecting" ? (
                <>
                  <Spinner className="animate-spin" size={14} />
                  Connecting...
                </>
              ) : connectionState === "connected" ? (
                "Reconnect"
              ) : (
                "Connect"
              )}
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
