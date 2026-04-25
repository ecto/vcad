import {
  translations,
  type TranslationKey,
  type SupportedLocale,
} from "./translations.js";

let currentLocale: SupportedLocale = "en";
let currentStrings: Record<string, string> = { ...translations.en };

export type { TranslationKey, SupportedLocale };

export function setLocale(locale: SupportedLocale) {
  currentLocale = locale;
  currentStrings = { ...translations.en, ...translations[locale] };
}

export function getLocale(): SupportedLocale {
  return currentLocale;
}

export function t(key: TranslationKey): string {
  return currentStrings[key] ?? key;
}

export function tFmt(key: TranslationKey, args: Record<string, string>): string {
  let result = currentStrings[key] ?? key;
  for (const [name, value] of Object.entries(args)) {
    result = result.replaceAll(`{${name}}`, value);
  }
  return result;
}

export function detectLocale(): SupportedLocale {
  if (typeof navigator === "undefined") return "en";
  const full = navigator.language.toLowerCase();
  if (full in translations) return full as SupportedLocale;
  const prefix = full.split("-")[0];
  if (prefix && prefix in translations) return prefix as SupportedLocale;
  return "en";
}

export function supportedLocales(): SupportedLocale[] {
  return Object.keys(translations) as SupportedLocale[];
}
