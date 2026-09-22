import { afterEach, describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { seedAuthenticatedSession } from "@/test/msw";
import {
  API_KEY_POLICY,
  LOGICAL_CHANNEL,
  ROUTING_GROUP,
  CONFIG_TEMPLATE,
  CONTROL_PLANE_USER,
  MODEL,
  OPERATION_RULE,
  PROXY,
  REGISTRATION_INVITATION_CODE,
  USER_GROUP,
} from "@/test/fixtures";
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
  ["/admin/registration-invitation-codes/new", /create registration code/i],
  ["/admin/api-key-policies/new", /create policy/i],
  ["/admin/models/new", /create pricing model/i],
  ["/admin/routing/groups/new", /save group/i],
  ["/admin/routing/logical-channels/new", /save channel/i],
  ["/admin/network/proxies/new", /create proxy/i],
  ["/admin/transforms/templates/new", /create template/i],
] as const;

const editRoutes = [
  [`/admin/users/${CONTROL_PLANE_USER.id}`, /save account details/i],
  [`/admin/user-groups/${USER_GROUP.id}`, /save user group/i],
  [
    `/admin/registration-invitation-codes/${REGISTRATION_INVITATION_CODE.id}`,
    /save registration code/i,
  ],
  [`/admin/api-key-policies/${API_KEY_POLICY.id}`, /save policy/i],
  [`/admin/models/${MODEL.id}`, /save pricing model/i],
  [`/admin/models/${MODEL.id}/pricing`, /save model pricing/i],
  [`/admin/routing/groups/${ROUTING_GROUP.id}`, /save group/i],
  [`/admin/routing/logical-channels/${LOGICAL_CHANNEL.id}`, /save channel/i],
  [
    `/admin/routing/operation-rules/${OPERATION_RULE.id}`,
    /save operation rule/i,
  ],
  [`/admin/network/proxies/${PROXY.id}`, /save proxy/i],
  [`/admin/transforms/templates/${CONFIG_TEMPLATE.id}`, /save template/i],
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

  it.each([
    ["/admin/routing/channel-groups/new", "/admin/routing/groups", "Routing groups"],
    ["/admin/routing/channels/old-id", "/admin/routing/logical-channels", "Logical channels"],
    ["/admin/routing/model-rules/old-id/protocols/old-child", "/admin/routing/operation-rules", "Operation rules"],
  ])("redirects retired route %s to canonical management", async (path, target, heading) => {
    seedAuthenticatedSession();
    renderAppAt(path);
    expect(await screen.findByRole("heading", { name: heading })).toBeInTheDocument();
    expect(window.location.pathname).toBe(target);
  });

  it("localizes logical channel editing", async () => {
    window.localStorage.setItem(STORAGE_KEY, "zh-CN");
    seedAuthenticatedSession();
    renderAppAt("/admin/routing/logical-channels/new");

    expect(await screen.findByRole("button", { name: "保存渠道" })).toBeInTheDocument();
  });
});
