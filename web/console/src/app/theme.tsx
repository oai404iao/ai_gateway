import { createContext, useContext, useEffect, useState } from "react";
import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";

type Theme = "light" | "dark" | "system";

interface ThemeContextValue {
  theme: Theme;
  setTheme: (theme: Theme) => void;
}

const ThemeContext = createContext<StoreApi<ThemeContextValue> | null>(null);
const STORAGE_KEY = "console-theme";

function systemPrefersDark(): boolean {
  return window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? false;
}

function applyTheme(theme: Theme): void {
  const isDark = theme === "dark" || (theme === "system" && systemPrefersDark());
  document.documentElement.classList.toggle("dark", isDark);
}

export function ThemeProvider({ children }: { children: React.ReactNode }) {
  const [store] = useState(() => createStore<ThemeContextValue>((set) => ({
    theme: (localStorage.getItem(STORAGE_KEY) as Theme | null) ?? "system",
    setTheme: (theme) => {
      localStorage.setItem(STORAGE_KEY, theme);
      set({ theme });
    },
  })));
  const theme = useStore(store, (state) => state.theme);

  useEffect(() => {
    applyTheme(theme);
  }, [theme]);

  return (
    <ThemeContext value={store}>{children}</ThemeContext>
  );
}

export function useTheme(): ThemeContextValue {
  const context = useContext(ThemeContext);
  if (!context) throw new Error("useTheme must be used within ThemeProvider");
  return useStore(context);
}
