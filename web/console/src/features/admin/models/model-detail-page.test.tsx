import { describe, expect, it } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import type { ModelInput } from "@/api/types";
import { MODEL } from "@/test/fixtures";
import { server, seedAuthenticatedSession } from "@/test/msw";

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

describe("ModelDetailPage", () => {
  it("initializes copy mode when navigating from the existing detail route", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderAppAt(`/admin/models/${MODEL.id}`);

    await user.click(
      await screen.findByRole("link", {
        name: `Copy ${MODEL.display_name}`,
      }),
    );

    await waitFor(() => {
      expect(window.location.pathname).toBe("/admin/models/new");
    });
    expect(await screen.findByLabelText("Client model id")).toHaveValue("");
    expect(screen.getByLabelText("Display name")).toHaveValue(
      `${MODEL.display_name} copy`,
    );
  });

  it("loads and submits the model-level advanced billing policy", async () => {
    seedAuthenticatedSession();
    let submitted: ModelInput | undefined;
    const advancedBilling = {
      long_context_tiers: [
        {
          input_tokens_threshold: 128_000,
          input_unit_price: "0.3",
          cached_input_unit_price: "0.15",
          cache_write_unit_price: "0.6",
          output_unit_price: "0.9",
        },
      ],
      request_multipliers: [
        {
          json_pointer: "/reasoning/effort",
          value: "high",
          multiplier: "2",
        },
      ],
    };
    server.use(
      http.put("/console/v1/models/:id", async ({ request }) => {
        submitted = (await request.json()) as ModelInput;
        return HttpResponse.json({
          id: MODEL.id,
          correlation_id: "33333333-0000-0000-0000-000000000000",
        });
      }),
    );

    const user = userEvent.setup();
    renderAppAt(`/admin/models/${MODEL.id}`);

    expect(await screen.findByLabelText("Client model id")).toBeDisabled();
    expect(
      screen.getByText(
        "The client model ID cannot change after a model rule is created.",
      ),
    ).toBeInTheDocument();
    const editor = await screen.findByLabelText(/advanced billing/i);
    expect(
      screen.getByRole("link", { name: /configure pricing/i }),
    ).toBeInTheDocument();
    fireEvent.change(editor, { target: { value: JSON.stringify(advancedBilling) } });
    await user.click(screen.getByRole("button", { name: /save client model/i }));

    await waitFor(() => {
      expect(submitted).toBeDefined();
    });
    expect(submitted?.advanced_billing).toEqual(advancedBilling);
    expect(submitted).not.toHaveProperty("source_payload");
  });

  it.each([{}, { provider: "example" }])("sends an explicit source payload %j", async (payload) => {
    seedAuthenticatedSession();
    let submitted: ModelInput | undefined;
    server.use(http.put(`/console/v1/models/${MODEL.id}`, async ({ request }) => {
      submitted = await request.json() as ModelInput;
      return HttpResponse.json({ id: MODEL.id, correlation_id: MODEL.id });
    }));
    const user = userEvent.setup();
    renderAppAt(`/admin/models/${MODEL.id}`);
    const source = await screen.findByLabelText("Source payload (JSON, optional)");
    expect(source).toHaveValue("");
    fireEvent.change(source, { target: { value: JSON.stringify(payload) } });
    await user.click(screen.getByRole("button", { name: "Save client model" }));
    await waitFor(() => expect(submitted?.source_payload).toEqual(payload));
  });

  it("keeps a stale draft through cancel and failed reload, then saves against the confirmed revision", async () => {
    seedAuthenticatedSession();
    const path = `/console/v1/models/${MODEL.id}`;
    const latest = { ...MODEL, display_name: "Changed elsewhere", updated_at: "2026-08-02T00:00:00Z" };
    let failReload = true;
    const writes: { etag: string | null; body: ModelInput }[] = [];
    server.use(
      http.get(path, () => {
        if (writes.length && failReload) return HttpResponse.json({ error: "unavailable" }, { status: 503 });
        const model = writes.length ? latest : MODEL;
        return HttpResponse.json(model, { headers: { ETag: `"${model.updated_at}"` } });
      }),
      http.put(path, async ({ request }) => {
        const etag = request.headers.get("if-match");
        writes.push({ etag, body: await request.json() as ModelInput });
        return etag === `"${latest.updated_at}"`
          ? HttpResponse.json({ id: MODEL.id, correlation_id: MODEL.id })
          : HttpResponse.json({ error: "conflict" }, { status: 409 });
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/models/${MODEL.id}`);
    const name = await screen.findByLabelText("Display name");
    fireEvent.change(name, { target: { value: "Local draft" } });
    const save = screen.getByRole("button", { name: "Save client model" });
    await user.click(save);
    const reload = await screen.findByRole("button", { name: "Load latest version" });
    expect(save).toBeDisabled();
    expect(name).toHaveValue("Local draft");
    await user.click(reload);
    await user.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Cancel" }));
    expect(name).toHaveValue("Local draft");
    await user.click(reload);
    await user.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Discard changes" }));
    expect(await screen.findByText("The latest version could not be loaded. Your draft has been kept.")).toBeVisible();
    expect(name).toHaveValue("Local draft");
    expect(save).toBeDisabled();
    failReload = false;
    await user.click(reload);
    await user.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Discard changes" }));
    await waitFor(() => expect(name).toHaveValue("Changed elsewhere"));
    fireEvent.change(name, { target: { value: "Reviewed" } });
    await user.click(save);
    await waitFor(() => expect(writes.map(({ etag, body }) => [etag, body.display_name])).toEqual([
      [`"${MODEL.updated_at}"`, "Local draft"], [`"${latest.updated_at}"`, "Reviewed"],
    ]));
  });
});
