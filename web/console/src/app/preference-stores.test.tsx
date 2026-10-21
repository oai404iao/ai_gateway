import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { currentLocale, STORAGE_KEY, translate, useI18n } from "@/app/i18n";
import { I18nProvider } from "@/app/i18n-provider";
import { ThemeProvider, useTheme } from "@/app/theme";

beforeEach(() => {
  localStorage.removeItem("console-theme");
  localStorage.setItem(STORAGE_KEY, "en-US");
});

afterEach(() => {
  cleanup();
  localStorage.removeItem("console-theme");
  localStorage.removeItem(STORAGE_KEY);
  document.documentElement.classList.remove("dark");
  document.documentElement.lang = "en-US";
});

describe("provider-scoped preference stores", () => {
  it("keeps theme stores isolated while applying and persisting the chosen theme", () => {
    const first = renderHook(useTheme, { wrapper: ThemeProvider });
    const second = renderHook(useTheme, { wrapper: ThemeProvider });
    expect(first.result.current.theme).toBe("system");

    act(() => first.result.current.setTheme("dark"));
    expect(first.result.current.theme).toBe("dark");
    expect(second.result.current.theme).toBe("system");
    expect(document.documentElement).toHaveClass("dark");
    expect(localStorage.getItem("console-theme")).toBe("dark");

    first.rerender();
    expect(first.result.current.theme).toBe("dark");
    first.unmount();
    const remounted = renderHook(useTheme, { wrapper: ThemeProvider });
    expect(remounted.result.current.theme).toBe("dark");
    act(() => remounted.result.current.setTheme("light"));
    expect(document.documentElement).not.toHaveClass("dark");
    expect(second.result.current.theme).toBe("system");
  });

  it("keeps locale stores isolated and synchronizes imperative translations and persisted preferences", () => {
    const first = renderHook(useI18n, { wrapper: I18nProvider });
    const second = renderHook(useI18n, { wrapper: I18nProvider });
    expect(first.result.current.locale).toBe("en-US");

    act(() => first.result.current.setLocale("zh-CN"));
    expect(first.result.current.t("Language")).toBe("语言");
    expect(second.result.current.locale).toBe("en-US");
    expect(second.result.current.t("Language")).toBe("Language");
    expect(currentLocale()).toBe("zh-CN");
    expect(translate("Language")).toBe("语言");
    expect(document.documentElement.lang).toBe("zh-CN");
    expect(localStorage.getItem(STORAGE_KEY)).toBe("zh-CN");

    first.rerender();
    expect(first.result.current.locale).toBe("zh-CN");
    first.unmount();
    const remounted = renderHook(useI18n, { wrapper: I18nProvider });
    expect(remounted.result.current.locale).toBe("zh-CN");
    act(() => remounted.result.current.setLocale("en-US"));
    expect(translate("Language")).toBe("Language");
    expect(document.documentElement.lang).toBe("en-US");
    expect(localStorage.getItem(STORAGE_KEY)).toBe("en-US");
  });
});
