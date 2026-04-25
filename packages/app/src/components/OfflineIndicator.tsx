import { WifiSlash } from "@phosphor-icons/react/dist/ssr/WifiSlash";
import { useOfflineStatus } from "@/hooks/useOfflineStatus";
import { t } from "@vcad/core";
import { useLocaleStore } from "@/stores/locale-store";

export function OfflineIndicator() {
  const { isOffline } = useOfflineStatus();
  useLocaleStore((s) => s.locale);

  if (!isOffline) return null;

  return (
    <div className="fixed bottom-3 left-3 z-40 flex items-center gap-2 px-3 py-1.5 bg-warning/10 border border-warning/30 text-warning text-xs">
      <WifiSlash size={14} weight="bold" />
      <span>{t("status.offline")}</span>
    </div>
  );
}
