import { describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, renderHook, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useQueryClient } from "@tanstack/react-query";
import { http, HttpResponse } from "msw";
import type { PropsWithChildren } from "react";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { CHANNEL, CHANNEL_GROUP, NEW_API_KEY_SECRET, OWN_API_KEY } from "@/test/fixtures";
import type { ApiKeyView, SelfApiKeyCreateInput, SelfApiKeyUpdateInput } from "@/api/types";

function TestApp({ children }: PropsWithChildren) {
  return (
    <AppProviders>
      <BrowserRouter>
        <AppRouter />
        {children}
      </BrowserRouter>
    </AppProviders>
  );
}

function renderAppAt(path: string) {
  window.history.replaceState({}, "", path);
  render(<TestApp />);
}

function keyResponse(key: ApiKeyView, version: string) {
  return HttpResponse.json(key, { headers: { ETag: `"${version}"` } });
}

const saveButton = () => screen.getByRole("button", { name: "Save changes" });
async function findReloadButton() {
  return within(await screen.findByRole("alert")).findByRole("button", { name: "Load latest version" });
}

async function openConflictedKey(readLatest: () => Response, draftName?: string) {
  seedAuthenticatedSession();
  const endpoint = `/console/v1/me/api-keys/${OWN_API_KEY.id}`;
  const reload = vi.fn(readLatest);
  const writes: { etag: string | null; input: SelfApiKeyUpdateInput }[] = [];
  server.use(
    http.get(endpoint, () => keyResponse(OWN_API_KEY, "v1"), { once: true }),
    http.get(endpoint, () => reload()),
    http.put(endpoint, async ({ request }) => {
      writes.push({
        etag: request.headers.get("if-match"),
        input: await request.json() as SelfApiKeyUpdateInput,
      });
      return HttpResponse.json({ error: "conflict" }, { status: 409 });
    }),
  );
  const user = userEvent.setup();
  renderAppAt(`/api-keys/${OWN_API_KEY.id}`);
  await user.click(await screen.findByRole("button", { name: "Edit" }));
  const name = await screen.findByDisplayValue(OWN_API_KEY.name);
  if (draftName) {
    await user.clear(name);
    await user.type(name, draftName);
  }
  await user.click(saveButton());
  expect(await findReloadButton()).toBeVisible();
  expect(saveButton()).toBeDisabled();
  await user.click(saveButton());
  expect(writes.map(({ etag }) => etag)).toEqual(['"v1"']);
  expect(reload).not.toHaveBeenCalled();
  return { user, writes, reload };
}

async function discardDialog(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await findReloadButton());
  return within(await screen.findByRole("alertdialog", { name: "Discard unsaved changes?" }));
}

async function confirmDiscard(user: ReturnType<typeof userEvent.setup>) {
  const dialog = await discardDialog(user);
  await user.click(dialog.getByRole("button", { name: "Discard changes" }));
}

describe("ApiKeysPage", () => {
  it("filters the owner's inventory and opens an edit sheet from its row menu", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderAppAt("/api-keys");
    await screen.findByText(OWN_API_KEY.name);
    await user.click(screen.getByRole("button", {name: /^Revoked/}));
    expect(await screen.findByText("No matching API keys")).toBeVisible();
    await user.click(screen.getByRole("button", {name: /^All \d/}));
    await user.type(screen.getByRole("searchbox", {name:"Filter key names"}), "not-in-inventory");
    expect(screen.getByText("No matching API keys")).toBeVisible();
    await user.clear(screen.getByRole("searchbox", {name:"Filter key names"}));
    await user.click(screen.getByRole("button", {name:`Actions for ${OWN_API_KEY.name}`}));
    await user.click(await screen.findByRole("menuitem", {name:"Edit"}));
    const editor = within(await screen.findByRole("dialog", {name:"Edit API key"}));
    expect(editor.getByLabelText("Name")).toHaveValue(OWN_API_KEY.name);
    await user.type(editor.getByLabelText("Name"), " draft");
    await user.keyboard("{Escape}");
    const guard = within(await screen.findByRole("alertdialog", {name:"Discard unsaved changes?"}));
    await user.click(guard.getByRole("button", {name:"Cancel"}));
    expect(editor.getByLabelText("Name")).toHaveValue(`${OWN_API_KEY.name} draft`);
  });

  it("opens the creation sheet from a direct link without creating a key", async () => {
    seedAuthenticatedSession();
    renderAppAt("/api-keys?create=1");
    expect(await screen.findByRole("dialog", {name:"New API key"})).toBeVisible();
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(new URLSearchParams(window.location.search).has("create")).toBe(false);
  });
  it("distinguishes same-named logical channel grants in different groups", async () => {
    seedAuthenticatedSession();
    const channels = ["one", "two"].map((suffix) => ({
      id: `channel-${suffix}`, channel_group_id: `group-${suffix}`, channel_group_name: `Group ${suffix}`,
      channel_group_enabled: true, name: "Primary", enabled: true, auto_disabled: false,
      api_formats: ["open_ai_responses"],
    }));
    server.use(
      http.get("/console/v1/me/api-keys", () => HttpResponse.json([{
        ...OWN_API_KEY, allowed_group_ids: [], allowed_channel_ids: channels.map((channel) => channel.id),
      }])),
      http.get("/console/v1/me/api-key-options", () => HttpResponse.json({
        policy_id: null, policy_name: null, policy_enabled: false,
        groups: [], channels,
      })),
    );
    renderAppAt("/api-keys");
    expect(await screen.findByText("Primary (Group one), Primary (Group two)")).toBeInTheDocument();
  });

  it("creates a key without a one-time secret dialog", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderAppAt("/api-keys");

    expect(await screen.findByText(OWN_API_KEY.name)).toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: "Channel groups" })).toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: "Logical channels" })).toBeInTheDocument();
    expect(screen.queryByRole("columnheader", { name: "Formats" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /new api key/i }));
    const nameField = await within(await screen.findByRole("dialog", { name: "New API key" })).findByLabelText(/^name$/i);
    await user.type(nameField, "spec key");
    await user.click(
      screen.getByRole("checkbox", { name: new RegExp(`^${CHANNEL_GROUP.name}`, "i") }),
    );
    await user.click(screen.getByRole("button", { name: /create key/i }));

    expect(await screen.findByText(/api key created/i)).toBeInTheDocument();
    expect(screen.queryByText(/save it now/i)).not.toBeInTheDocument();
  });

  it("masks, reveals, and copies the complete API key", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderAppAt("/api-keys");

    const value = await screen.findByLabelText<HTMLInputElement>(/api key value/i);
    expect(value.value === "sk-••••••••••••••••••••••••").toBe(true);

    await user.click(screen.getByRole("button", { name: /show full api key/i }));
    expect(value.value === NEW_API_KEY_SECRET).toBe(true);

    await user.click(screen.getByRole("button", { name: /copy api key/i }));
    expect(await navigator.clipboard.readText() === NEW_API_KEY_SECRET).toBe(true);
  });

  it("shows and copies administrator-configured API hosts", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderAppAt("/api-keys");

    expect(await screen.findByDisplayValue("https://api.example.test/v1")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Copy API host" }));

    expect(await navigator.clipboard.readText()).toBe("https://api.example.test/v1");
    expect(await screen.findByText("API host copied")).toBeInTheDocument();
  });

  it("serializes a datetime-local expiry as RFC 3339", async () => {
    seedAuthenticatedSession();
    let submitted: SelfApiKeyCreateInput | undefined;
    server.use(
      http.post("/console/v1/me/api-keys", async ({ request }) => {
        submitted = (await request.json()) as SelfApiKeyCreateInput;
        return HttpResponse.json(
          {
            id: OWN_API_KEY.id,
            secret: NEW_API_KEY_SECRET,
            correlation_id: "11111111-0000-0000-0000-000000000000",
          },
          { status: 201 },
        );
      }),
    );
    const user = userEvent.setup();
    renderAppAt("/api-keys");

    await user.click(await screen.findByRole("button", { name: /new api key/i }));
    await user.type(screen.getByLabelText(/^name$/i), "expiring key");
    await user.click(
      screen.getByRole("checkbox", { name: new RegExp(`^${CHANNEL_GROUP.name}`, "i") }),
    );
    const localExpiry = "2099-08-01T12:00";
    fireEvent.change(screen.getByLabelText(/expires at/i), {
      target: { value: localExpiry },
    });
    await user.click(screen.getByRole("button", { name: /create key/i }));

    await waitFor(() => {
      expect(submitted).toEqual({
        name: "expiring key",
        expires_at: new Date(localExpiry).toISOString(),
        allowed_group_ids: [CHANNEL_GROUP.id],
        allowed_channel_ids: [],
        requests_per_minute: null,
        max_concurrent_requests: null,
        quota_limit_amount: null,
      });
    });
  });

  it("explains when a default API key policy is missing", async () => {
    seedAuthenticatedSession();
    server.use(
      http.post("/console/v1/me/api-keys", () =>
        HttpResponse.json(
          { error: "default_api_key_policy_required" },
          { status: 422 },
        ),
      ),
    );
    const user = userEvent.setup();
    renderAppAt("/api-keys");

    await user.click(await screen.findByRole("button", { name: /new api key/i }));
    await user.type(screen.getByLabelText(/^name$/i), "policy-required");
    await user.click(
      screen.getByRole("checkbox", { name: new RegExp(`^${CHANNEL_GROUP.name}`, "i") }),
    );
    await user.click(screen.getByRole("button", { name: /create key/i }));

    expect(
      await screen.findByText(/create an api key policy, then assign it/i),
    ).toBeInTheDocument();
  });

  it.each([
    ["default_api_key_policy_required", /create an api key policy, then assign it/i],
    ["default_api_key_policy_disabled", /your default api key policy is disabled/i],
  ])("requires policy options before creating a key: %s", async (code, message) => {
    seedAuthenticatedSession();
    let submitted = false;
    server.use(
      http.get("/console/v1/me/api-key-options", () =>
        HttpResponse.json({ error: code }, { status: 422 })),
      http.post("/console/v1/me/api-keys", () => {
        submitted = true;
        return HttpResponse.json({}, { status: 201 });
      }),
    );
    const user = userEvent.setup();
    renderAppAt("/api-keys");

    await user.click(await screen.findByRole("button", { name: /new api key/i }));
    expect(await screen.findByText(message)).toBeInTheDocument();
    expect(screen.getByText("No selectable channel groups.")).toBeInTheDocument();
    await user.type(screen.getByLabelText(/^name$/i), "policy-required");

    expect(screen.getByRole("button", { name: /create key/i })).toBeDisabled();
    expect(submitted).toBe(false);
  });

  it("shows allowed channel group names instead of UUIDs on key details", async () => {
    seedAuthenticatedSession();
    renderAppAt(`/api-keys/${OWN_API_KEY.id}`);

    const label = await screen.findByText(/^Channel groups$/i);
    expect(screen.queryByText("Formats")).not.toBeInTheDocument();
    const value = label.nextElementSibling;

    expect(value).toHaveTextContent(CHANNEL_GROUP.name);
    expect(value).not.toHaveTextContent(CHANNEL_GROUP.id);
  });

  it("saves a loaded disabled status without requiring reselection", async () => {
    seedAuthenticatedSession();
    const disabledKey = { ...OWN_API_KEY, status: "disabled" };
    let submittedStatus: string | undefined;
    server.use(
      http.get("/console/v1/me/api-keys/:id", () =>
        HttpResponse.json(disabledKey, {
          headers: { ETag: `"${disabledKey.updated_at}"` },
        }),
      ),
      http.put("/console/v1/me/api-keys/:id", async ({ request }) => {
        const body = (await request.json()) as { status: string };
        submittedStatus = body.status;
        return HttpResponse.json({
          id: disabledKey.id,
          correlation_id: "33333333-0000-0000-0000-000000000000",
        });
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/api-keys/${disabledKey.id}`);
    await user.click(await screen.findByRole("button", { name: "Edit" }));
    expect(await screen.findByDisplayValue(disabledKey.name)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /save changes/i }));

    await waitFor(() => {
      expect(submittedStatus).toBe("disabled");
    });
  });

  it("keeps the conflicted draft on cancel or read failure and permits another reload", async () => {
    let fail = true;
    const { user, writes, reload } = await openConflictedKey(() => fail
      ? HttpResponse.json({ error: "unavailable" }, { status: 503 })
      : keyResponse({ ...OWN_API_KEY, name: "recovered" }, "v2"), "my draft");

    const confirm = await discardDialog(user);
    await user.click(confirm.getByRole("button", { name: "Cancel" }));
    expect(screen.getByLabelText("Name")).toHaveValue("my draft");
    expect(reload).not.toHaveBeenCalled();

    await confirmDiscard(user);
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent(/could not be loaded/i));
    expect(reload).toHaveBeenCalledTimes(1);
    expect(await findReloadButton()).toBeEnabled();
    expect(screen.getByLabelText("Name")).toHaveValue("my draft");
    expect(saveButton()).toBeDisabled();

    fail = false;
    await confirmDiscard(user);
    expect(await screen.findByDisplayValue("recovered")).toBeInTheDocument();
    expect(reload).toHaveBeenCalledTimes(2);
    expect(writes).toHaveLength(1);
    expect(saveButton()).toBeEnabled();
  });

  it("keeps a dirty draft paired with its original ETag during background refetch", async () => {
    seedAuthenticatedSession();
    const endpoint = `/console/v1/me/api-keys/${OWN_API_KEY.id}`;
    const remoteRead = vi.fn(() => keyResponse({ ...OWN_API_KEY, name: "remote rename", requests_per_minute: 99 }, "v2"));
    const writes: (string | null)[] = [];
    server.use(
      http.get(endpoint, () => keyResponse(OWN_API_KEY, "v1"), { once: true }),
      http.get(endpoint, () => new HttpResponse(null, { status: 503 }), { once: true }),
      http.get(endpoint, remoteRead),
      http.put(endpoint, ({ request }) => {
        writes.push(request.headers.get("if-match"));
        return HttpResponse.json({ error: "conflict" }, { status: 409 });
      }),
    );
    window.history.replaceState({}, "", `/api-keys/${OWN_API_KEY.id}`);
    const { result } = renderHook(() => useQueryClient(), { wrapper: TestApp });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Edit" }));
    const name = await screen.findByDisplayValue(OWN_API_KEY.name);
    await user.clear(name);
    await user.type(name, "draft before refresh");

    await act(() => result.current.refetchQueries({ type: "active" }));
    expect(await findReloadButton()).toBeEnabled();
    expect(screen.getByLabelText("Name")).toHaveValue("draft before refresh");

    await act(() => result.current.refetchQueries({ type: "active" }));
    expect(remoteRead).toHaveBeenCalledTimes(1);
    expect(screen.getByLabelText("Name")).toHaveValue("draft before refresh");
    expect(screen.getByLabelText("Requests / minute")).toHaveValue(OWN_API_KEY.requests_per_minute);
    expect(writes).toHaveLength(0);
    await user.click(saveButton());
    await waitFor(() => expect(writes).toEqual(['"v1"']));
  });

  it("replaces all fields and the write version together, including after a second conflict", async () => {
    const localExpiry = "2099-08-01T12:00";
    const remoteFields: SelfApiKeyUpdateInput = {
      name: "remote rename", status: "disabled", expires_at: new Date(`${localExpiry}:42.125`).toISOString(),
      allowed_group_ids: [], allowed_channel_ids: [CHANNEL.id], requests_per_minute: 81,
      max_concurrent_requests: 7, quota_limit_amount: "123.45",
    };
    let revoked = false;
    const { user, writes, reload } = await openConflictedKey(() => revoked
      ? keyResponse({ ...OWN_API_KEY, status: "revoked" }, "v3")
      : keyResponse({ ...OWN_API_KEY, ...remoteFields }, "v2"), "old local edit");

    await confirmDiscard(user);
    const name = await screen.findByDisplayValue(remoteFields.name);
    expect(screen.getByLabelText("Status")).toHaveTextContent(/disabled/i);
    expect(screen.getByLabelText(/expires at/i)).toHaveValue(localExpiry);
    expect(screen.getByLabelText("Requests / minute")).toHaveValue(81);
    expect(screen.getByLabelText("Max concurrent requests")).toHaveValue(7);
    expect(screen.getByLabelText("Quota limit amount")).toHaveValue("123.45");
    expect(screen.getByRole("checkbox", { name: CHANNEL_GROUP.name })).not.toBeChecked();
    await user.click(screen.getByRole("button", { name: /show individual channels/i }));
    expect(screen.getByRole("checkbox", { name: `${CHANNEL.name} (${CHANNEL_GROUP.name})` })).toBeChecked();
    expect(reload).toHaveBeenCalledTimes(1);
    expect(writes).toHaveLength(1);

    await user.clear(name);
    await user.type(name, "new local edit");
    await user.click(saveButton());
    expect(await findReloadButton()).toBeVisible();
    expect(saveButton()).toBeDisabled();
    expect(writes).toHaveLength(2);
    expect(writes[1]).toEqual({ etag: '"v2"', input: { ...remoteFields, name: "new local edit" } });

    revoked = true;
    await confirmDiscard(user);
    expect(await screen.findByText("Revoked", { exact: true })).toBeInTheDocument();
    expect(screen.queryByLabelText("Name")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Save changes" })).not.toBeInTheDocument();
    expect(reload).toHaveBeenCalledTimes(2);
    expect(writes).toHaveLength(2);
  });

  it("removes the stale editor and key controls when the reload returns 404", async () => {
    const { user, writes, reload } = await openConflictedKey(() =>
      HttpResponse.json({ error: "not_found" }, { status: 404 }));

    await user.click(await findReloadButton());
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent(/no longer available/i));
    expect(screen.queryByLabelText("Name")).not.toBeInTheDocument();
    expect(reload).toHaveBeenCalledTimes(1);
    expect(screen.queryByLabelText("API key value")).not.toBeInTheDocument();
    for (const name of ["Save changes", "Show full API key", "Copy API key", "Revoke API key", "Delete API key"]) {
      expect(screen.queryByRole("button", { name })).not.toBeInTheDocument();
    }
    expect(writes).toHaveLength(1);
  });
});
