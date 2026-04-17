import { useEffect, useRef } from "react";
import { useSignInDelightStore, useSyncStore, useAuthStore } from "@vcad/auth";
import { useNotificationStore } from "@/stores/notification-store";

/**
 * SignInDelight component handles:
 * 1. Showing welcome toast on first sign-in
 * 2. Showing first sync completion toast
 *
 * Confetti is triggered via event in AuthProvider and handled by CelebrationOverlay.
 */
export function SignInDelight() {
  const user = useAuthStore((s) => s.user);
  const isAnonymous = useAuthStore((s) => s.isAnonymous);
  // Anonymous Supabase sessions don't get the welcome / first-sync delight.
  const isSignedIn = !!user && !isAnonymous;
  const syncStatus = useSyncStore((s) => s.syncStatus);
  const hasSeenFirstSync = useSignInDelightStore((s) => s.hasSeenFirstSync);
  const markFirstSyncSeen = useSignInDelightStore((s) => s.markFirstSyncSeen);
  const addToast = useNotificationStore((s) => s.addToast);

  // Track previous sync status to detect transitions
  const prevSyncStatus = useRef(syncStatus);

  // Listen for welcome event to show toast
  useEffect(() => {
    const handleWelcome = (e: CustomEvent<{ firstName: string }>) => {
      const { firstName } = e.detail;
      addToast(`Welcome, ${firstName}! Your work now syncs to the cloud.`, "success", 6000);
    };

    window.addEventListener("vcad:welcome-sign-in", handleWelcome as EventListener);
    return () => {
      window.removeEventListener("vcad:welcome-sign-in", handleWelcome as EventListener);
    };
  }, [addToast]);

  // Watch for first sync completion
  useEffect(() => {
    // Only trigger for signed-in users who haven't seen the first sync toast
    if (!isSignedIn || hasSeenFirstSync) {
      prevSyncStatus.current = syncStatus;
      return;
    }

    // Detect syncing → synced transition
    if (prevSyncStatus.current === "syncing" && syncStatus === "synced") {
      markFirstSyncSeen();
      addToast("Documents synced to cloud", "success");
    }

    prevSyncStatus.current = syncStatus;
  }, [isSignedIn, syncStatus, hasSeenFirstSync, markFirstSyncSeen, addToast]);

  // This component doesn't render anything visible
  return null;
}
