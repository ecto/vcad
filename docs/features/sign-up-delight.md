# Sign-Up Delight

Confetti, greeting, and sync toast to celebrate new user sign-ups.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | `unassigned` |
| Priority | `p1` |
| Effort | `s` |

> Verified against repo 2026-07-30. Corrected `in-progress` → `shipped`: `packages/auth/src/stores/sign-in-delight-store.ts`, `packages/app/src/components/SignInDelight.tsx`, `vcad:celebrate-sign-in` listener in `CelebrationOverlay.tsx`, and dispatch in `AuthProvider.tsx` all exist.

## Problem

First impressions matter. When a new user signs up via OAuth, there's no celebration — they just see the app. This misses an opportunity to:
1. Create a memorable first experience
2. Confirm their account is working
3. Explain that their work now syncs to the cloud

## Solution

On first sign-in, trigger a celebration sequence:

1. **Confetti burst** — Dispatch `vcad:celebrate-sign-in` event, reuse existing `CelebrationOverlay` particle system
2. **Welcome toast** — "Welcome, [FirstName]! Your work now syncs to the cloud."
3. **First sync toast** — On first successful sync, show "Documents synced to cloud"

### Detection Logic

First sign-in is detected when:
- `localStorage['vcad:seenSignInCelebration']` is not set, AND
- `user.created_at` is within the last hour (prevents confetti on new device for existing users)

### State

```typescript
// sign-in-delight-store.ts
interface SignInDelightState {
  hasSeenSignInCelebration: boolean;
  hasSeenFirstSync: boolean;
  markSignInCelebrationSeen: () => void;
  markFirstSyncSeen: () => void;
}
```

**Not included:** Animations, sound effects, email verification celebration.

## UX Details

### Interaction States

| State | Behavior |
|-------|----------|
| First sign-in detected | Dispatch confetti event, show welcome toast |
| Welcome toast | Auto-dismiss after 5 seconds, accent color |
| First sync (syncing → synced) | Show sync completion toast |

### Edge Cases

- **Existing user, new device**: Check `user.created_at` to avoid confetti for established users
- **Sign out + sign in**: No confetti (localStorage persists unless cleared)
- **Offline sign-up**: Still show welcome toast, skip sync toast until online

## Implementation

### Files to Modify

| File | Changes |
|------|---------|
| `packages/auth/src/stores/sign-in-delight-store.ts` | NEW — persisted store for celebration state |
| `packages/auth/src/index.ts` | Export new store |
| `packages/auth/src/components/AuthProvider.tsx` | Detect first sign-in, dispatch event, show toast |
| `packages/app/src/components/SignInDelight.tsx` | NEW — component that watches for first sync |
| `packages/app/src/components/CelebrationOverlay.tsx` | Add event listener for `vcad:celebrate-sign-in` |
| `packages/app/src/App.tsx` | Add `<SignInDelight />` component |

### State Flow

1. User completes OAuth flow
2. `AuthProvider.onAuthStateChange` fires with `SIGNED_IN`
3. Check if first-time sign-in (localStorage + created_at)
4. If first time:
   - Set `hasSeenSignInCelebration = true` in store
   - Dispatch `vcad:celebrate-sign-in` event
   - Show welcome toast via notification store
5. `SignInDelight` component watches sync store
6. On first `syncing → synced` transition:
   - Check `hasSeenFirstSync`
   - If false, show sync toast and mark seen

## Tasks

### Phase 1: Store and Provider (`xs`)

- [x] Create `sign-in-delight-store.ts` with persist middleware
- [x] Export store from `@vcad/auth` package
- [x] Add first-sign-in detection to `AuthProvider.tsx`

### Phase 2: Celebration Components (`s`)

- [x] Add `vcad:celebrate-sign-in` event listener to `CelebrationOverlay.tsx`
- [x] Create `SignInDelight.tsx` component for sync toast
- [x] Wire up in `App.tsx`

### Phase 3: Polish (`xs`)

- [ ] Test fresh signup flow end-to-end
- [ ] Test existing user scenarios
- [ ] Verify localStorage persistence

## Acceptance Criteria

- [ ] Fresh OAuth signup triggers confetti animation
- [ ] Welcome toast shows user's first name
- [ ] Refreshing page does not trigger duplicate celebration
- [ ] First successful sync shows "Documents synced" toast
- [ ] Existing user signing in on new device sees no confetti
- [ ] Sign out + sign in (same user) shows no confetti

## Future Enhancements

- [ ] Celebrate account upgrades (free → paid)
- [ ] Celebrate milestones (first model exported, etc.)
- [ ] Sound effect option (disabled by default)
