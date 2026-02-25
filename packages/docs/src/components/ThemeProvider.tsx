"use client";

import { createContext, useContext, useEffect, useState, useCallback } from "react";

type ThemeSetting = "system" | "dark" | "light";
type ResolvedTheme = "dark" | "light";

interface ThemeContextValue {
  theme: ResolvedTheme;
  setting: ThemeSetting;
  setSetting: (setting: ThemeSetting) => void;
  /** Cycle: system → light → dark → system */
  toggleTheme: () => void;
}

const ThemeContext = createContext<ThemeContextValue>({
  theme: "dark",
  setting: "system",
  setSetting: () => {},
  toggleTheme: () => {},
});

function getSystemTheme(): ResolvedTheme {
  if (typeof window === "undefined") return "dark";
  return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

function resolveTheme(setting: ThemeSetting): ResolvedTheme {
  if (setting === "system") return getSystemTheme();
  return setting;
}

const CYCLE: ThemeSetting[] = ["system", "light", "dark"];

export function ThemeProvider({ children }: { children: React.ReactNode }) {
  const [setting, setSettingState] = useState<ThemeSetting>("system");
  const [theme, setTheme] = useState<ResolvedTheme>("dark");
  const [mounted, setMounted] = useState(false);

  // Init from localStorage
  useEffect(() => {
    setMounted(true);
    const stored = localStorage.getItem("vcad-docs-theme") as ThemeSetting | null;
    const initial = stored ?? "system";
    setSettingState(initial);
    setTheme(resolveTheme(initial));
  }, []);

  // Listen for system preference changes when in "system" mode
  useEffect(() => {
    if (!mounted || setting !== "system") return;

    const mq = window.matchMedia("(prefers-color-scheme: light)");
    const handler = () => setTheme(getSystemTheme());
    mq.addEventListener("change", handler);
    return () => mq.removeEventListener("change", handler);
  }, [mounted, setting]);

  // Apply theme class to <html>
  useEffect(() => {
    if (!mounted) return;
    const root = document.documentElement;
    if (theme === "light") {
      root.classList.add("light");
    } else {
      root.classList.remove("light");
    }
  }, [theme, mounted]);

  const setSetting = useCallback((newSetting: ThemeSetting) => {
    setSettingState(newSetting);
    setTheme(resolveTheme(newSetting));
    localStorage.setItem("vcad-docs-theme", newSetting);
  }, []);

  const toggleTheme = useCallback(() => {
    setSettingState(prev => {
      const idx = CYCLE.indexOf(prev);
      const next = CYCLE[(idx + 1) % CYCLE.length]!;
      setTheme(resolveTheme(next));
      localStorage.setItem("vcad-docs-theme", next);
      return next;
    });
  }, []);

  return (
    <ThemeContext.Provider value={{ theme, setting, setSetting, toggleTheme }}>
      {children}
    </ThemeContext.Provider>
  );
}

export function useTheme() {
  return useContext(ThemeContext);
}
