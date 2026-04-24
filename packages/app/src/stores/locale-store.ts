import { create } from "zustand";
import { persist } from "zustand/middleware";
import {
  setLocale as coreSetLocale,
  getLocale,
  detectLocale,
  supportedLocales,
  type SupportedLocale,
} from "@vcad/core";

interface LocaleState {
  locale: SupportedLocale;
  setLocale: (locale: SupportedLocale) => void;
}

export const useLocaleStore = create<LocaleState>()(
  persist(
    (set) => ({
      locale: getLocale(),
      setLocale: (locale) => {
        coreSetLocale(locale);
        set({ locale });
      },
    }),
    {
      name: "vcad-locale",
      onRehydrate: (_state, _error, stored) => {
        if (stored?.locale) {
          coreSetLocale(stored.locale);
        }
      },
    },
  ),
);

export function initLocale() {
  const stored = useLocaleStore.getState().locale;
  const persisted = localStorage.getItem("vcad-locale");
  if (persisted) {
    try {
      const parsed = JSON.parse(persisted);
      if (parsed?.state?.locale) {
        coreSetLocale(parsed.state.locale);
        return;
      }
    } catch {}
  }
  coreSetLocale(detectLocale());
}

export { supportedLocales, type SupportedLocale };
