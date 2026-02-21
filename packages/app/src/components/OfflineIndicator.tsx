import { WifiSlash } from "@phosphor-icons/react/dist/ssr/WifiSlash";
import { useOfflineStatus } from "@/hooks/useOfflineStatus";

export function OfflineIndicator() {
  const { isOffline } = useOfflineStatus();

  if (!isOffline) return null;

  return (
    <div className="fixed bottom-3 left-3 z-40 flex items-center gap-2 px-3 py-1.5 bg-warning/10 border border-warning/30 text-warning text-xs">
      <WifiSlash size={14} weight="bold" />
      <span>Offline</span>
    </div>
  );
}
