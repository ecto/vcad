/**
 * Persisted sheet-metal shop profile.
 *
 * The user's brake / die / grain capabilities drive the DFM inspector and
 * (later) costing. Stored in localStorage so it survives reloads and is
 * shared across documents — a shop's capabilities are a property of the
 * shop, not the part.
 *
 * `merge` rebases any persisted profile onto {@link DEFAULT_SHOP_PROFILE}
 * so a profile saved before a new capability existed still loads (mirrors
 * the kernel's field-tolerant `ShopProfile` deserialization).
 */

import { create } from "zustand";
import { persist } from "zustand/middleware";
import { DEFAULT_SHOP_PROFILE, type SheetMetalShopProfile } from "@vcad/engine";

/** Numeric, user-tunable fields (everything except `name`). */
export type ShopProfileNumberField = Exclude<
  keyof SheetMetalShopProfile,
  "name"
>;

interface ShopProfileState {
  profile: SheetMetalShopProfile;
  setName: (name: string) => void;
  setField: (field: ShopProfileNumberField, value: number) => void;
  resetToGeneric: () => void;
}

export const useShopProfileStore = create<ShopProfileState>()(
  persist(
    (set) => ({
      profile: { ...DEFAULT_SHOP_PROFILE },
      setName: (name) =>
        set((s) => ({ profile: { ...s.profile, name } })),
      setField: (field, value) =>
        set((s) => ({ profile: { ...s.profile, [field]: value } })),
      resetToGeneric: () => set({ profile: { ...DEFAULT_SHOP_PROFILE } }),
    }),
    {
      name: "vcad-sheet-shop-profile",
      merge: (persisted, current) => {
        const p = persisted as
          | { profile?: Partial<SheetMetalShopProfile> }
          | undefined;
        return {
          ...current,
          profile: { ...DEFAULT_SHOP_PROFILE, ...(p?.profile ?? {}) },
        };
      },
    },
  ),
);
