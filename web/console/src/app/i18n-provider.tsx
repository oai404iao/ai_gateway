import { useEffect, useState } from "react";
import { useStore } from "zustand";
import {
  I18nContext,
  STORAGE_KEY,
  browserLocale,
  createI18nStore,
  setCurrentI18nStore,
} from "@/app/i18n";

export function I18nProvider({ children }: { children: React.ReactNode }) {
  const [store] = useState(() => createI18nStore(browserLocale()));
  const locale = useStore(store, (state) => state.locale);
  setCurrentI18nStore(store);

  useEffect(() => {
    window.localStorage.setItem(STORAGE_KEY, locale);
    document.documentElement.lang = locale;
  }, [locale]);

  return <I18nContext.Provider value={store}>{children}</I18nContext.Provider>;
}
