import { useState, useRef, useEffect } from "react";
import { getSupabase } from "../client";
import { useAuthStore } from "../stores/auth-store";
import { useSyncStore } from "../stores/sync-store";
import { useUserPreferences } from "../hooks/useUserPreferences";

interface UserMenuProps {
  /** Callback when "Sync now" is clicked */
  onSyncNow?: () => void;
}

/**
 * User avatar dropdown menu showing account info, sync status, and sign-out option.
 */
export function UserMenu({ onSyncNow }: UserMenuProps) {
  const user = useAuthStore((s) => s.user);
  const { syncStatus, lastSyncAt } = useSyncStore();
  const { preferences, updatePreferences } = useUserPreferences();
  const [open, setOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);

  // Close menu when clicking outside
  useEffect(() => {
    if (!open) return;

    const handleClickOutside = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };

    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, [open]);

  const handleSignOut = async () => {
    const supabase = getSupabase();
    if (supabase) {
      await supabase.auth.signOut();
    }
    setOpen(false);
  };

  const formatRelativeTime = (timestamp: number): string => {
    const seconds = Math.floor((Date.now() - timestamp) / 1000);

    if (seconds < 60) return "just now";
    if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
    if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
    return `${Math.floor(seconds / 86400)}d ago`;
  };

  if (!user) return null;

  const avatarUrl = user.user_metadata?.avatar_url;
  const initials =
    user.email?.[0]?.toUpperCase() ||
    user.user_metadata?.full_name?.[0]?.toUpperCase() ||
    "U";

  return (
    <div ref={menuRef} className="relative">
      {/* Avatar button */}
      <button
        onClick={() => setOpen(!open)}
        className="w-8 h-8 rounded-full bg-accent text-white flex items-center justify-center text-sm font-medium relative overflow-hidden hover:opacity-80 transition-opacity"
        aria-haspopup="true"
        aria-expanded={open}
      >
        {avatarUrl ? (
          <img
            src={avatarUrl}
            alt=""
            className="w-full h-full object-cover"
            referrerPolicy="no-referrer"
          />
        ) : (
          initials
        )}

        {/* Sync status indicator dot */}
        <span
          className={`absolute -bottom-0.5 -right-0.5 w-3 h-3 rounded-full border-2 border-bg ${
            syncStatus === "syncing"
              ? "bg-yellow-500"
              : syncStatus === "error"
                ? "bg-danger"
                : "bg-green-500"
          }`}
          aria-label={`Sync status: ${syncStatus}`}
        />
      </button>

      {/* Dropdown menu */}
      {open && (
        <div className="absolute right-0 mt-2 w-56 border border-border bg-card/95 backdrop-blur-sm shadow-lg py-1 z-50">
          {/* User info with inline avatar */}
          <div className="px-3 py-2 border-b border-border flex items-center gap-2">
            <div className="w-6 h-6 rounded-full bg-accent text-white flex items-center justify-center text-[10px] font-medium overflow-hidden flex-shrink-0">
              {avatarUrl ? (
                <img
                  src={avatarUrl}
                  alt=""
                  className="w-full h-full object-cover"
                  referrerPolicy="no-referrer"
                />
              ) : (
                initials
              )}
            </div>
            <div className="min-w-0">
              <div className="text-xs text-text truncate">
                {user.user_metadata?.full_name || user.email}
              </div>
              {user.user_metadata?.full_name && (
                <div className="text-[10px] text-text-muted truncate">
                  {user.email}
                </div>
              )}
            </div>
          </div>

          {/* Sync status */}
          <div className="px-3 py-2 border-b border-border flex items-center gap-2 text-[10px] text-text-muted">
            {syncStatus === "synced" ? (
              <CloudIcon className="w-3 h-3 text-green-500" />
            ) : syncStatus === "syncing" ? (
              <CloudIcon className="w-3 h-3 text-yellow-500 animate-pulse" />
            ) : syncStatus === "error" ? (
              <CloudOffIcon className="w-3 h-3 text-danger" />
            ) : (
              <CloudIcon className="w-3 h-3 text-text-muted" />
            )}
            <span>
              {syncStatus === "syncing"
                ? "Syncing..."
                : syncStatus === "error"
                  ? "Sync failed"
                  : lastSyncAt
                    ? `Synced ${formatRelativeTime(lastSyncAt)}`
                    : "Not synced yet"}
            </span>
          </div>

          {/* Menu items */}
          <button
            onClick={() => {
              onSyncNow?.();
              setOpen(false);
            }}
            className="w-full px-3 py-2 text-left text-xs text-text hover:bg-border/50"
          >
            Sync now
          </button>

          {/* Share conversations toggle (SFT opt-out) */}
          <label className="flex items-start gap-2 px-3 py-2 border-t border-border cursor-pointer hover:bg-border/30">
            <input
              type="checkbox"
              checked={preferences.share_chat_conversations}
              onChange={(e) =>
                updatePreferences({ share_chat_conversations: e.target.checked })
              }
              className="mt-0.5"
            />
            <span className="text-[10px] text-text-muted leading-tight">
              Share chat conversations to improve vcad AI.
              <span className="text-text-muted/70 block">
                Uncheck to keep your prompts private.
              </span>
            </span>
          </label>

          <button
            onClick={handleSignOut}
            className="w-full px-3 py-2 text-left text-xs text-danger hover:bg-border/50 border-t border-border"
          >
            Sign out
          </button>
        </div>
      )}
    </div>
  );
}

// Simple cloud icons
function CloudIcon({ className }: { className?: string }) {
  return (
    <svg
      className={className}
      fill="none"
      stroke="currentColor"
      viewBox="0 0 24 24"
    >
      <path
        strokeLinecap="round"
        strokeLinejoin="round"
        strokeWidth={2}
        d="M3 15a4 4 0 004 4h9a5 5 0 10-.1-9.999 5.002 5.002 0 10-9.78 2.096A4.001 4.001 0 003 15z"
      />
    </svg>
  );
}

function CloudOffIcon({ className }: { className?: string }) {
  return (
    <svg
      className={className}
      fill="none"
      stroke="currentColor"
      viewBox="0 0 24 24"
    >
      <path
        strokeLinecap="round"
        strokeLinejoin="round"
        strokeWidth={2}
        d="M3 3l18 18M10.5 6.5A5 5 0 0116 10.9 5 5 0 0116 19H7a4 4 0 01-.85-7.91M3 15a4 4 0 014-4"
      />
    </svg>
  );
}
