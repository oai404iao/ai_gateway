import { describe, expect, it } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { CHANNEL_GROUP, NEW_API_KEY_SECRET, OWN_API_KEY } from "@/test/fixtures";
import { maskApiKey } from "@/lib/api-keys";
import type { SelfApiKeyCreateInput } from "@/api/types";

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

describe("ApiKeysPage", () => {
  it("creates a key without a one-time secret dialog", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderAppAt("/api-keys");

    expect(await screen.findByText(OWN_API_KEY.name)).toBeInTheDocument();
    expect(screen.getByDisplayValue(maskApiKey(NEW_API_KEY_SECRET))).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /new api key/i }));
    const nameField = await screen.findByLabelText(/name/i);
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

    const value = await screen.findByLabelText(/api key value/i);
    expect(value).toHaveValue(maskApiKey(NEW_API_KEY_SECRET));

    await user.click(screen.getByRole("button", { name: /show full api key/i }));
    expect(value).toHaveValue(NEW_API_KEY_SECRET);

    await user.click(screen.getByRole("button", { name: /copy api key/i }));
    expect(await navigator.clipboard.readText()).toBe(NEW_API_KEY_SECRET);
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

  it("navigates to a key detail page on row click", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderAppAt("/api-keys");
    const row = await screen.findByText(OWN_API_KEY.name);
    await user.click(row);
    // The detail page renders the danger-zone revoke button.
    await waitFor(() => {
      expect(screen.getByRole("button", { name: /revoke api key/i })).toBeInTheDocument();
    });
  });

  it("shows allowed channel group names instead of UUIDs on key details", async () => {
    seedAuthenticatedSession();
    renderAppAt(`/api-keys/${OWN_API_KEY.id}`);

    const label = await screen.findByText(/^Allowed groups$/i);
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

    expect(await screen.findByDisplayValue(disabledKey.name)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /save changes/i }));

    await waitFor(() => {
      expect(submittedStatus).toBe("disabled");
    });
  });

  it("surfaces an optimistic-concurrency conflict as a toast", async () => {
    seedAuthenticatedSession();
    // Own API key detail GET returns a stable ETag; PUT with a mismatched
    // If-Match yields 409.
    let putHit = false;
    server.use(
      http.get("/console/v1/me/api-keys/:id", () =>
        HttpResponse.json(OWN_API_KEY, {
          headers: { ETag: `"v1"` },
        }),
      ),
      http.put("/console/v1/me/api-keys/:id", () => {
        putHit = true;
        return HttpResponse.json(
          { error: "Console operation rejected" },
          { status: 409 },
        );
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/api-keys/${OWN_API_KEY.id}`);

    // The detail form loads with reset (valid) values; renaming and saving
    // sends If-Match from the GET ETag, and the overridden PUT returns 409.
    const nameField = await screen.findByLabelText(/name/i);
    await user.clear(nameField);
    await user.type(nameField, "renamed");
    await user.click(screen.getByRole("button", { name: /save changes/i }));

    await waitFor(() => {
      expect(putHit).toBe(true);
      expect(screen.getByText(/changed by another session/i)).toBeInTheDocument();
    }, { timeout: 3000 });
  });
});
