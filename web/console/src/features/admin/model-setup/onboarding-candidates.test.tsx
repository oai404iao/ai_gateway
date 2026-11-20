import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { http, HttpResponse } from "msw";
import { describe, expect, it, vi } from "vitest";
import type { ApiOperation, OnboardingOperationInput, OperationRuleView } from "@/api/types";
import { I18nProvider } from "@/app/i18n-provider";
import { CHANNEL_CAPABILITY, LOGICAL_CHANNEL, OPERATION_RULE, UPSTREAM_ACCESS } from "@/test/fixtures";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { OnboardingCandidates } from "./onboarding-candidates";

const exact = "gpt-5", alias = "gpt-5-stable";
const at = CHANNEL_CAPABILITY.updated_at;
const codexAccess = { ...UPSTREAM_ACCESS, id: "codex", connector_kind: "codex", name: "Codex account" };
const codexChannel = { ...LOGICAL_CHANNEL, access_id: codexAccess.id, credential_id: null, id: "c2", name: "West" };

function cap(operation: ApiOperation, channel = LOGICAL_CHANNEL, available_models = [exact, alias]) {
  return {
    ...CHANNEL_CAPABILITY, id: `${channel.id}-${operation}`, channel_id: channel.id,
    settings: { ...CHANNEL_CAPABILITY.settings, operation, available_models },
  };
}

function show(capabilities: ReturnType<typeof cap>[], allowRegistration: boolean, rules: OperationRuleView[] = []) {
  seedAuthenticatedSession();
  server.use(
    http.get("/console/v1/routing/capabilities", () => HttpResponse.json(capabilities)),
    http.get("/console/v1/routing/logical-channels", () => HttpResponse.json([LOGICAL_CHANNEL, codexChannel])),
    http.get("/console/v1/routing/accesses", () => HttpResponse.json([UPSTREAM_ACCESS, codexAccess])),
  );
  const onChange = vi.fn<(value: OnboardingOperationInput[]) => void>();
  const onEdit = vi.fn();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const editor = (currentRules: OperationRuleView[], key: number) => (
    <I18nProvider><QueryClientProvider client={client}>
      <OnboardingCandidates {...{ onChange, onEdit, allowRegistration }} rules={currentRules} modelName={exact} key={key} />
    </QueryClientProvider></I18nProvider>
  );
  const { rerender } = render(editor(rules, 0));
  return { client, onChange, onEdit, update: (currentRules: OperationRuleView[], key = 0) => rerender(editor(currentRules, key)) };
}

describe("onboarding routing candidates", () => {
  it("selects available exact matches across providers and operations, without guessing model or operation support", async () => {
    const east = cap("responses"), west = cap("responses", codexChannel);
    const ws = cap("responses-ws", codexChannel, [alias]);
    const chat = cap("chat_completion");
    const paused = { ...cap("images_edit"), auto_disabled: true };
    const unsupported = cap("chat_completion", codexChannel);
    const { onChange, onEdit } = show([east, west, ws, chat, paused, unsupported], true);
    await waitFor(() => expect(onChange).toHaveBeenCalled());
    expect(onEdit).not.toHaveBeenCalled();
    const selected = (capability_id: string) => ({ capability_id, updated_at: at, upstream_model: exact, priority: 0, register_model: false });
    expect(onChange.mock.lastCall?.[0]).toEqual([
      { operation: "chat_completion", rule_updated_at: null, candidates: [selected(chat.id)] },
      { operation: "responses", rule_updated_at: null, candidates: [selected(west.id), selected(east.id)] },
    ]);
    expect(screen.getByText("Automatically disabled")).toBeVisible();
    expect(screen.getByText("Connector operation unavailable")).toBeVisible();
    expect(screen.queryByRole("group", { name: "Images generation" })).not.toBeInTheDocument();

    const choose = screen.getByRole("checkbox", { name: "Register and route gpt-5: Codex account / West · Responses WebSocket" });
    expect(choose).not.toBeChecked();
    const user = userEvent.setup();
    choose.focus();
    await user.keyboard("[Space]");
    expect(onEdit).toHaveBeenCalledOnce();
    expect(onChange.mock.lastCall?.[0].at(-1)).toEqual({
      operation: "responses-ws", rule_updated_at: null, candidates: [{ ...selected(ws.id), register_model: true }],
    });
  });

  it("does not duplicate an existing pair or change its weight, and allows an explicit addition in another tier", async () => {
    const east = cap("responses"), west = cap("responses", codexChannel);
    const rule: OperationRuleView = { ...OPERATION_RULE, enabled: false, routing_tiers: [
      { priority: 10, selection_strategy: "weighted_round_robin", candidates: [{ capability_id: east.id, upstream_model: exact, weight: 9 }] },
      { priority: 20, selection_strategy: "weighted_random", candidates: [{ capability_id: west.id, upstream_model: alias, weight: 16 }] },
    ] };
    const { onChange } = show([east, west], false, [rule]);
    await waitFor(() => expect(onChange).toHaveBeenCalledWith([]));
    expect(screen.getByText("Already in this tier · weight 9")).toBeVisible();
    expect(screen.getByText("Rule stays disabled · 0 to add")).toBeVisible();
    const user = userEvent.setup();
    const pickTier = screen.getByRole("combobox", { name: /Routing tier for Primary access/ });
    const add = screen.getByRole("checkbox", { name: /Add route: Primary access/ });
    await user.click(pickTier);
    await user.click(screen.getByRole("option", { name: "Priority 20 · Weighted random" }));
    await user.click(add);
    expect(onChange.mock.lastCall?.[0]).toEqual([{ operation: "responses", rule_updated_at: rule.updated_at, candidates: [
      { capability_id: east.id, upstream_model: exact, priority: 20, updated_at: at, register_model: false },
    ] }]);
    pickTier.focus();
    await user.keyboard("[Enter]");
    await user.click(await screen.findByRole("option", { name: "Priority 10 · Weighted round-robin" }));
    expect(onChange.mock.lastCall?.[0]).toEqual([]);
    expect(add).not.toBeChecked();
  });

  it("uses only explicitly picked advertised aliases in route-only mode; refreshes do not rebase pending drafts", async () => {
    const east = cap("responses", LOGICAL_CHANNEL, [alias]);
    const rule = { ...OPERATION_RULE, routing_tiers: [] };
    const { onChange, update, client } = show([east], false, [rule]);
    await waitFor(() => expect(onChange).toHaveBeenCalledWith([]));
    const section = within(screen.getByRole("group", { name: "Responses" }));
    expect(section.queryByRole("checkbox", { name: /Register and route/ })).not.toBeInTheDocument();
    const user = userEvent.setup();
    await user.click(section.getByRole("button", { name: /Upstream model for/ }));
    expect(screen.queryByRole("option", { name: exact })).not.toBeInTheDocument();
    await user.click(await screen.findByRole("option", { name: alias }));
    const newVersion = "2026-07-18T00:00:00Z";
    const changedRule = { ...rule, updated_at: newVersion };
    const refreshed = { ...east, updated_at: newVersion, settings: { ...east.settings, available_models: [exact] } };
    await act(() => { client.setQueryData(["console", "channel-capabilities"], [refreshed]); });
    update([changedRule]);
    await user.click(section.getByRole("checkbox", { name: /Add route/ }));
    expect(onChange.mock.lastCall?.[0]).toEqual([{ operation: "responses", rule_updated_at: rule.updated_at, candidates: [
      { capability_id: east.id, upstream_model: alias, updated_at: at, priority: 0, register_model: false },
    ] }]);
    update([changedRule], 1);
    await waitFor(() => expect(onChange.mock.lastCall?.[0]).toEqual([{ operation: "responses", rule_updated_at: newVersion, candidates: [
      { capability_id: east.id, upstream_model: exact, updated_at: newVersion, priority: 0, register_model: false },
    ] }]));
  });
});
