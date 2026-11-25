import { afterEach, describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { seedAuthenticatedSession } from "@/test/msw";
import { API_KEY_POLICY, PROXY } from "@/test/fixtures";
import { STORAGE_KEY, setCurrentLocale } from "@/app/i18n";

function renderAppAt(path: string) {
  window.history.replaceState({}, "", path);
  render(
    <AppProviders>
      <BrowserRouter>
        <AppRouter />
      </BrowserRouter>
    </AppProviders>,
  );
}

afterEach(() => {
  window.localStorage.removeItem(STORAGE_KEY);
  setCurrentLocale("en-US");
});

const createRoutes = [
  ["/admin/user-groups/new", /create user group/i],
  ["/admin/models/new", /create client model/i],
  ["/admin/routing/logical-channels/new", /save channel/i],
  ["/admin/transforms/templates/new", /create template/i],
] as const;

const editRoutes = [
  [`/admin/api-key-policies/${API_KEY_POLICY.id}`, /save policy/i],
  [`/admin/network/proxies/${PROXY.id}`, /save proxy/i],
] as const;

describe("Admin detail routes", () => {
  it.each(createRoutes)("opens create mode at %s", async (path, buttonName) => {
    seedAuthenticatedSession();
    renderAppAt(path);

    expect(await screen.findByRole("button", { name: buttonName })).toBeInTheDocument();
  });

  it.each(editRoutes)("opens edit mode at %s", async (path, buttonName) => {
    seedAuthenticatedSession();
    renderAppAt(path);

    expect(await screen.findByRole("button", { name: buttonName })).toBeInTheDocument();
  });

  it("localizes logical channel editing", async () => {
    window.localStorage.setItem(STORAGE_KEY, "zh-CN");
    seedAuthenticatedSession();
    renderAppAt("/admin/routing/logical-channels/new");

    expect(await screen.findByRole("button", { name: "保存渠道" })).toBeInTheDocument();
  });
});
