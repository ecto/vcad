import { create } from "zustand";
import { persist } from "zustand/middleware";
import {
  setLocale as coreSetLocale,
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
      locale: "en" as SupportedLocale,
      setLocale: (locale: SupportedLocale) => {
        coreSetLocale(locale);
        set({ locale });
      },
    }),
    { name: "vcad-locale" },
  ),
);

export function initLocale() {
  const persisted = localStorage.getItem("vcad-locale");
  if (persisted) {
    try {
      const parsed = JSON.parse(persisted);
      if (parsed?.state?.locale) {
        coreSetLocale(parsed.state.locale as SupportedLocale);
        useLocaleStore.setState({ locale: parsed.state.locale });
        return;
      }
    } catch {}
  }
  const detected = detectLocale();
  coreSetLocale(detected);
  useLocaleStore.setState({ locale: detected });
}

export { supportedLocales, type SupportedLocale };
