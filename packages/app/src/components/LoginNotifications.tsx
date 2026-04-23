import { useEffect } from "react";
import { useNotificationStore } from "@/stores/notification-store";
import { notify } from "@/lib/native-notify";

interface SignInSuccessDetail {
  firstName: string;
  email?: string;
  provider?: string;
}

interface SignInAttemptDetail {
  email: string;
}

interface SignInAttemptFailedDetail {
  email: string;
  message: string;
}

/**
 * Surfaces login and login-attempt events as in-app toasts and OS
 * notifications. The AuthProvider and AuthModal dispatch window CustomEvents
 * — this component listens and routes them to the notification store.
 */
export function LoginNotifications() {
  const toast = useNotificationStore((s) => s.toast);

  useEffect(() => {
    const handleSignInSuccess = (e: CustomEvent<SignInSuccessDetail>) => {
      const { firstName, provider } = e.detail;
      const providerLabel = provider && provider !== "email" ? ` via ${provider}` : "";
      toast.success(`Signed in as ${firstName}${providerLabel}`);
      void notify({
        title: "vcad — Signed in",
        body: `Welcome back, ${firstName}.`,
      });
    };

    const handleSignOut = () => {
      toast.info("Signed out");
    };

    const handleAttempt = (e: CustomEvent<SignInAttemptDetail>) => {
      toast.info(`Sending sign-in link to ${e.detail.email}…`, { duration: 2500 });
    };

    const handleAttemptSent = (e: CustomEvent<SignInAttemptDetail>) => {
      toast.success(`Magic link sent to ${e.detail.email}. Check your inbox.`);
    };

    const handleAttemptFailed = (e: CustomEvent<SignInAttemptFailedDetail>) => {
      toast.error(`Sign-in failed: ${e.detail.message}`);
      void notify({
        title: "vcad — Sign-in failed",
        body: e.detail.message,
      });
    };

    window.addEventListener("vcad:sign-in-success", handleSignInSuccess as EventListener);
    window.addEventListener("vcad:sign-out", handleSignOut as EventListener);
    window.addEventListener("vcad:sign-in-attempt", handleAttempt as EventListener);
    window.addEventListener("vcad:sign-in-attempt-sent", handleAttemptSent as EventListener);
    window.addEventListener("vcad:sign-in-attempt-failed", handleAttemptFailed as EventListener);

    return () => {
      window.removeEventListener("vcad:sign-in-success", handleSignInSuccess as EventListener);
      window.removeEventListener("vcad:sign-out", handleSignOut as EventListener);
      window.removeEventListener("vcad:sign-in-attempt", handleAttempt as EventListener);
      window.removeEventListener("vcad:sign-in-attempt-sent", handleAttemptSent as EventListener);
      window.removeEventListener("vcad:sign-in-attempt-failed", handleAttemptFailed as EventListener);
    };
  }, [toast]);

  return null;
}
