import { t } from "@vcad/core";

export function ErrorScreen({ message }: { message: string }) {
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-bg">
      <div className="flex flex-col items-center gap-2 text-center">
        <div className="text-sm font-bold text-danger">{t("error.engine")}</div>
        <div className="max-w-md text-xs text-text-muted">{message}</div>
      </div>
    </div>
  );
}
