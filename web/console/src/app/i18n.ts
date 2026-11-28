import { createContext, useCallback, useContext } from "react";
import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";
import { adminModelsRoutingZhCN } from "@/app/i18n/admin-models-routing";
import { adminProvidersCodexZhCN } from "@/app/i18n/admin-providers-codex";
import { adminSystemPluginsUsersZhCN } from "@/app/i18n/admin-system-plugins-users";
import { commonZhCN } from "@/app/i18n/common";
import { navigationAndAuthZhCN } from "@/app/i18n/navigation-and-auth";
import { statisticsAndLogsZhCN } from "@/app/i18n/statistics-and-logs";
import { onboardingCandidatesZhCN } from "@/features/admin/model-setup/onboarding-candidates-i18n";

export type ConsoleLocale = "en-US" | "zh-CN";

// This stores only a display-language preference. Console access tokens remain
// memory-only and refresh credentials remain in their HttpOnly cookie.
const STORAGE_KEY = "ai-gateway-console.locale";

export const zhCN = {
  ...onboardingCandidatesZhCN,
  ...commonZhCN,
  ...navigationAndAuthZhCN,
  ...statisticsAndLogsZhCN,
  ...adminModelsRoutingZhCN,
  ...adminProvidersCodexZhCN,
  ...adminSystemPluginsUsersZhCN,
} as const satisfies Record<string, string>;

export type KnownI18nKey = keyof typeof zhCN;
export type I18nKey = KnownI18nKey | (string & {});

const zhCNLookup: Record<string, string> = zhCN;

interface I18nState {
  locale: ConsoleLocale;
  setLocale: (locale: ConsoleLocale) => void;
}

export function createI18nStore(locale: ConsoleLocale) {
  return createStore<I18nState>((set) => ({
    locale,
    setLocale: (locale) => set({ locale }),
  }));
}

let activeStore = createI18nStore("en-US");

// Imperative formatters share the active provider's locale, while React
// consumers subscribe only to their own provider's store.
export function setCurrentI18nStore(store: StoreApi<I18nState>): void {
  activeStore = store;
}

export function browserLocale(): ConsoleLocale {
  if (typeof window === "undefined") return "en-US";
  const stored = window.localStorage.getItem(STORAGE_KEY);
  if (stored === "en-US" || stored === "zh-CN") return stored;
  return window.navigator.language.toLowerCase().startsWith("zh") ? "zh-CN" : "en-US";
}

export function currentLocale(): ConsoleLocale {
  return activeStore.getState().locale;
}

export function setCurrentLocale(locale: ConsoleLocale): void {
  activeStore.getState().setLocale(locale);
}

export function translate(
  key: I18nKey,
  values: Record<string, string | number> = {},
): string {
  return translateFor(currentLocale(), key, values);
}

export function translateFor(
  locale: ConsoleLocale,
  key: I18nKey,
  values: Record<string, string | number> = {},
): string {
  const template = locale === "zh-CN" ? (zhCNLookup[key] ?? key) : key;
  return template.replace(/\{(\w+)\}/g, (_, name: string) => String(values[name] ?? `{${name}}`));
}

interface I18nContextValue {
  locale: ConsoleLocale;
  setLocale: (locale: ConsoleLocale) => void;
  t: (key: I18nKey, values?: Record<string, string | number>) => string;
}

const I18nContext = createContext<StoreApi<I18nState> | null>(null);
export { I18nContext, STORAGE_KEY };

export function useI18n(): I18nContextValue {
  const context = useContext(I18nContext);
  if (!context) {
    throw new Error("useI18n must be used within I18nProvider");
  }
  const { locale, setLocale } = useStore(context);
  const t = useCallback(
    (key: I18nKey, values?: Record<string, string | number>) => translateFor(locale, key, values),
    [locale],
  );
  return { locale, setLocale, t };
}
