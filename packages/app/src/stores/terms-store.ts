import { create } from "zustand";

export const TERMS_VERSION = "2026-04-23";
const STORAGE_KEY = `vcad.tos.accepted.${TERMS_VERSION}`;

interface TermsState {
  accepted: boolean;
  accept: () => void;
}

function readAccepted(): boolean {
  if (typeof window === "undefined") return true;
  try {
    return window.localStorage.getItem(STORAGE_KEY) === "1";
  } catch {
    return false;
  }
}

export const useTermsStore = create<TermsState>((set) => ({
  accepted: readAccepted(),
  accept: () => {
    try {
      window.localStorage.setItem(STORAGE_KEY, "1");
      window.localStorage.setItem(
        `${STORAGE_KEY}.at`,
        new Date().toISOString(),
      );
    } catch {
      // ignore — in-memory acceptance still lets the session proceed
    }
    set({ accepted: true });
  },
}));
