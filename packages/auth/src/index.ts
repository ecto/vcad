// Client
export {
  ensureSession,
  getAuthRedirectUrl,
  getSupabase,
  isAuthEnabled,
  isTauriRuntime,
  requireSupabase,
} from "./client";

// Desktop deep-link auth bridge
export {
  handleAuthDeepLink,
  isAuthDeepLink,
  type AuthDeepLinkResult,
} from "./auth-deep-link";

// Stores
export { useAuthStore } from "./stores/auth-store";
export {
  useSyncStore,
  type SyncStatus,
  type SyncConflict,
} from "./stores/sync-store";
export { useSignInDelightStore } from "./stores/sign-in-delight-store";

// Hooks
export { useAuth } from "./hooks/useAuth";
export { useRequireAuth, type GatedFeature } from "./hooks/useRequireAuth";
export { useUserPreferences, type UserPreferences } from "./hooks/useUserPreferences";

// Components
export { AuthProvider } from "./components/AuthProvider";
export { AuthModal } from "./components/AuthModal";
export { UserMenu } from "./components/UserMenu";
export { FeatureGate } from "./components/FeatureGate";
export { SignInButton } from "./components/SignInButton";
export { VersionHistoryPanel } from "./components/VersionHistoryPanel";

// Sync
export {
  triggerSync,
  debouncedSync,
  enableCloudSync,
  initSyncListeners,
  configureStorage,
  listCloudDocuments,
  fetchCloudDocument,
  createShare,
  revokeShare,
  listSharesForDocument,
  fetchSharedDocument,
  type StorageAdapter,
  type LocalDocument,
  type CloudDocument,
  type CloudDocumentMeta,
  type ShareRecord,
  type SharedDocumentResult,
} from "./sync";

// Profiles
export {
  getMyProfile,
  getProfileByUsername,
  checkUsernameAvailable,
  createProfile,
  updateProfile,
  fetchPublicDocument,
  listPublicDocuments,
  publishDocument,
  slugify,
  createShareRedirect,
  lookupShareRedirect,
  type Profile,
  type PublicDocumentResult,
  type PublicDocumentMeta,
} from "./profile";

// Collab transport
export {
  joinCollabChannel,
  isApplyingRemoteOps,
  type CollabCallbacks,
  type CollabChannel,
} from "./collab-channel";

// Version history
export {
  getVersionHistory,
  restoreVersion,
  getCloudIdForDocument,
  configureVersionHistoryStorage,
  labelVersion,
  unlabelVersion,
  listNamedVersions,
  type DocumentVersion,
  type NamedVersion,
} from "./version-history";

// AI
export { textToCAD, isAIAvailable } from "./ai";

// Chat persistence
export {
  loadOrCreateThread,
  hydrateThread,
  loadDeltas,
  subscribeToThread,
  persistToolResult,
  clearThreadMessages,
  type DbChatThread,
  type DbChatMessage,
  type DbChatToolCall,
  type DbChatMessageDelta,
  type ChatMessageStatus,
  type ChatToolStatus,
  type ThreadHydration,
  type ThreadSubscription,
  type ThreadSubscriptionCallbacks,
} from "./chat-persistence";
