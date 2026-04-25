import { useState, useCallback, useRef, useEffect } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { At } from "@phosphor-icons/react/dist/ssr/At";
import { Check } from "@phosphor-icons/react/dist/ssr/Check";
import { CircleNotch } from "@phosphor-icons/react/dist/ssr/CircleNotch";
import { WarningCircle } from "@phosphor-icons/react/dist/ssr/WarningCircle";
import {
  checkUsernameAvailable,
  createProfile,
  useAuthStore,
} from "@vcad/auth";
import { cn } from "@/lib/utils";

interface UsernamePickerModalProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onComplete: (username: string) => void;
}

const USERNAME_RE = /^[a-z0-9]([a-z0-9-]*[a-z0-9])?$/;
const MIN_LEN = 2;
const MAX_LEN = 24;

type ValidationState =
  | { status: "idle" }
  | { status: "checking" }
  | { status: "available" }
  | { status: "taken" }
  | { status: "invalid"; reason: string };

function validateFormat(value: string): string | null {
  if (value.length < MIN_LEN) return `At least ${MIN_LEN} characters`;
  if (value.length > MAX_LEN) return `At most ${MAX_LEN} characters`;
  if (!USERNAME_RE.test(value))
    return "Lowercase letters, numbers, and hyphens only";
  if (value.startsWith("-") || value.endsWith("-"))
    return "Cannot start or end with a hyphen";
  return null;
}

export function UsernamePickerModal({
  open,
  onOpenChange,
  onComplete,
}: UsernamePickerModalProps) {
  const user = useAuthStore((s) => s.user);
  const [username, setUsername] = useState("");
  const [displayName, setDisplayName] = useState(
    user?.user_metadata?.full_name ?? "",
  );
  const [validation, setValidation] = useState<ValidationState>({
    status: "idle",
  });
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const checkAvailability = useCallback(
    async (value: string) => {
      const formatErr = validateFormat(value);
      if (formatErr) {
        setValidation({ status: "invalid", reason: formatErr });
        return;
      }
      setValidation({ status: "checking" });
      try {
        const available = await checkUsernameAvailable(value);
        setValidation(available ? { status: "available" } : { status: "taken" });
      } catch {
        setValidation({ status: "invalid", reason: "Could not check availability" });
      }
    },
    [],
  );

  const handleUsernameChange = (value: string) => {
    const cleaned = value.toLowerCase().replace(/[^a-z0-9-]/g, "");
    setUsername(cleaned);
    setError(null);

    if (debounceRef.current) clearTimeout(debounceRef.current);

    if (!cleaned || cleaned.length < MIN_LEN) {
      setValidation({ status: "idle" });
      return;
    }

    const formatErr = validateFormat(cleaned);
    if (formatErr) {
      setValidation({ status: "invalid", reason: formatErr });
      return;
    }

    debounceRef.current = setTimeout(() => {
      checkAvailability(cleaned);
    }, 400);
  };

  useEffect(() => {
    return () => {
      if (debounceRef.current) clearTimeout(debounceRef.current);
    };
  }, []);

  const handleSubmit = async () => {
    if (validation.status !== "available" || submitting) return;
    setSubmitting(true);
    setError(null);
    try {
      await createProfile(username, displayName || undefined);
      onComplete(username);
      onOpenChange(false);
    } catch (err) {
      const msg = (err as Error).message;
      if (msg.includes("duplicate") || msg.includes("unique")) {
        setValidation({ status: "taken" });
      } else {
        setError(msg);
      }
    } finally {
      setSubmitting(false);
    }
  };

  const canSubmit = validation.status === "available" && !submitting;

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/50 backdrop-blur-sm" />
        <Dialog.Content
          data-tauri-drag-region=""
          className={cn(
            "fixed left-1/2 top-1/2 z-50 w-full max-w-sm -translate-x-1/2 -translate-y-1/2",
            "bg-surface p-6 shadow-2xl select-none",
            "focus:outline-none",
          )}
        >
          <Dialog.Close className="absolute right-3 top-3 p-1.5 text-text-muted hover:text-text transition-colors cursor-pointer">
            <X size={14} />
          </Dialog.Close>

          <div className="flex items-center gap-2 mb-5">
            <At size={16} className="text-brand" />
            <Dialog.Title className="text-sm font-semibold text-text">
              Pick your username
            </Dialog.Title>
          </div>
          <Dialog.Description className="text-xs text-text-muted mb-4 leading-relaxed">
            Your public profile will live at{" "}
            <span className="font-mono text-text">
              vcad.io/@{username || "you"}
            </span>
            . You can change it later.
          </Dialog.Description>

          <div className="space-y-3">
            {/* Username field */}
            <div>
              <label className="block text-[11px] text-text-muted mb-1">
                Username
              </label>
              <div className="relative">
                <span className="absolute left-2 top-1/2 -translate-y-1/2 text-text-muted text-xs">
                  @
                </span>
                <input
                  type="text"
                  value={username}
                  onChange={(e) => handleUsernameChange(e.target.value)}
                  placeholder="your-name"
                  maxLength={MAX_LEN}
                  autoFocus
                  className={cn(
                    "w-full pl-6 pr-8 py-1.5 text-xs font-mono",
                    "bg-bg border text-text",
                    "focus:outline-none focus:border-brand",
                    validation.status === "available"
                      ? "border-green-500"
                      : validation.status === "taken" ||
                          validation.status === "invalid"
                        ? "border-danger"
                        : "border-border",
                  )}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") handleSubmit();
                  }}
                />
                <div className="absolute right-2 top-1/2 -translate-y-1/2">
                  {validation.status === "checking" && (
                    <CircleNotch
                      size={12}
                      className="text-text-muted animate-spin"
                    />
                  )}
                  {validation.status === "available" && (
                    <Check size={12} className="text-green-500" />
                  )}
                  {(validation.status === "taken" ||
                    validation.status === "invalid") && (
                    <WarningCircle size={12} className="text-danger" />
                  )}
                </div>
              </div>
              {validation.status === "taken" && (
                <p className="text-[10px] text-danger mt-1">
                  Username is already taken
                </p>
              )}
              {validation.status === "invalid" && (
                <p className="text-[10px] text-danger mt-1">
                  {validation.reason}
                </p>
              )}
            </div>

            {/* Display name */}
            <div>
              <label className="block text-[11px] text-text-muted mb-1">
                Display name{" "}
                <span className="text-text-muted/50">(optional)</span>
              </label>
              <input
                type="text"
                value={displayName}
                onChange={(e) => setDisplayName(e.target.value)}
                placeholder={user?.email?.split("@")[0] ?? ""}
                maxLength={64}
                className={cn(
                  "w-full px-2 py-1.5 text-xs",
                  "bg-bg border border-border text-text",
                  "focus:outline-none focus:border-brand",
                )}
              />
            </div>
          </div>

          {error && (
            <p className="text-[10px] text-danger mt-3">{error}</p>
          )}

          <div className="flex items-center justify-end gap-2 mt-5">
            <button
              type="button"
              onClick={() => onOpenChange(false)}
              className="px-3 py-1.5 text-xs text-text-muted hover:text-text hover:bg-hover transition-colors"
            >
              Cancel
            </button>
            <button
              type="button"
              onClick={handleSubmit}
              disabled={!canSubmit}
              className={cn(
                "px-3 py-1.5 text-xs font-medium transition-colors",
                canSubmit
                  ? "bg-brand text-white hover:bg-brand/90"
                  : "bg-border text-text-muted cursor-not-allowed",
              )}
            >
              {submitting ? "Creating…" : "Claim username"}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
