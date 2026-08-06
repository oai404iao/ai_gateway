import { afterEach, describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { seedAuthenticatedSession } from "@/test/msw";
import {
  API_KEY_POLICY,
  CHANNEL,
  CHANNEL_GROUP,
  CONFIG_TEMPLATE,
  CONTROL_PLANE_USER,
  SEARCH_MCP_SERVER,
  MODEL,
  MODEL_RULE,
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
  ["/admin/models/new", /create upstream model/i],
  ["/admin/routing/channel-groups/new", /create group/i],
  ["/admin/routing/channels/new", /create channel/i],
  ["/admin/routing/model-rules/new", /create rule/i],
  ["/admin/mcp-servers/new", /create mcp server/i],
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
  [`/admin/models/${MODEL.id}`, /save upstream model/i],
  [`/admin/routing/channel-groups/${CHANNEL_GROUP.id}`, /save group/i],
  [`/admin/routing/channels/${CHANNEL.id}`, /save channel/i],
  [`/admin/routing/model-rules/${MODEL_RULE.id}`, /save rule/i],
  [`/admin/mcp-servers/${SEARCH_MCP_SERVER.id}`, /save mcp server/i],
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

  it("localizes channel editing while retaining API format product names", async () => {
    window.localStorage.setItem(STORAGE_KEY, "zh-CN");
    seedAuthenticatedSession();
    renderAppAt("/admin/routing/channels/new");

    expect(await screen.findByRole("button", { name: "创建渠道" })).toBeInTheDocument();
    expect(screen.getByText("API 格式")).toBeInTheDocument();
    expect(screen.getByDisplayValue("Chat Completions")).toBeInTheDocument();
  });
});
