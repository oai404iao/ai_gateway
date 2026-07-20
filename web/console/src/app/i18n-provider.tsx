import { useCallback, useEffect, useMemo, useState } from "react";
import {
  I18nContext,
  STORAGE_KEY,
  browserLocale,
  setCurrentLocale,
  translateFor,
  type ConsoleLocale,
} from "@/app/i18n";

export function I18nProvider({ children }: { children: React.ReactNode }) {
  const [locale, setLocale] = useState<ConsoleLocale>(browserLocale);
  setCurrentLocale(locale);

  useEffect(() => {
    window.localStorage.setItem(STORAGE_KEY, locale);
    document.documentElement.lang = locale;
  }, [locale]);

  const t = useCallback(
    (key: string, values?: Record<string, string | number>) =>
      translateFor(locale, key, values),
    [locale],
  );
  const value = useMemo(
    () => ({
      locale,
      setLocale,
      t,
    }),
    [locale, t],
  );

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}
