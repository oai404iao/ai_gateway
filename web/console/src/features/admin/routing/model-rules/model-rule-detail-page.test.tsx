import { describe, expect, it } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { MODEL, MODEL_RULE } from "@/test/fixtures";
import type { ModelRuleInput } from "@/api/types";

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

describe("ModelRuleDetailPage", () => {
  it("uses the shared Select UI for upstream and custom client models", async () => {
    seedAuthenticatedSession();
    let submitted: ModelRuleInput | undefined;
    server.use(
      http.put("/console/v1/routing/model-rules/:id", async ({ request }) => {
        submitted = (await request.json()) as ModelRuleInput;
        return HttpResponse.json({
          id: MODEL_RULE.id,
          correlation_id: "66666666-0000-0000-0000-000000000000",
        });
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/routing/model-rules/${MODEL_RULE.id}`);

    const clientModelSelect = await screen.findByRole("combobox", { name: "Client model" });
    await user.click(clientModelSelect);
    const listbox = await screen.findByRole("listbox");
    expect(within(listbox).getByText(MODEL.provider_name ?? "")).toBeInTheDocument();
    await user.click(
      within(listbox).getByRole("option", {
        name: `${MODEL.display_name} (${MODEL.source_model_id})`,
      }),
    );

    await user.click(clientModelSelect);
    await user.click(await screen.findByRole("option", { name: "Custom client model" }));
    await user.type(
      await screen.findByRole("textbox", { name: "Custom client model" }),
      "my-custom-client-model",
    );
    await user.click(screen.getByRole("button", { name: /save rule/i }));

    await waitFor(() => {
      expect(submitted).toBeDefined();
    });
    expect(submitted?.client_model).toBe("my-custom-client-model");
  });
});
