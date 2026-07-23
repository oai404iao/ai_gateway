import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { CONFIG_TEMPLATE, CONFIG_TEMPLATE_DETAIL } from "@/test/fixtures";
import type { ConfigTemplateInput } from "@/api/types";

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

describe("ConfigTemplateDetailPage", () => {
  it("loads and resubmits the stored template document", async () => {
    seedAuthenticatedSession();
    let submitted: ConfigTemplateInput | undefined;
    server.use(
      http.put("/console/v1/transforms/templates/:id", async ({ request }) => {
        submitted = (await request.json()) as ConfigTemplateInput;
        return HttpResponse.json({
          id: CONFIG_TEMPLATE.id,
          correlation_id: "66666666-0000-0000-0000-000000000000",
        });
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/transforms/templates/${CONFIG_TEMPLATE.id}`);

    expect(await screen.findByDisplayValue(CONFIG_TEMPLATE.name)).toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: /json configuration/i }));
    expect(screen.getByLabelText(/transform document json/i)).toHaveValue(
      JSON.stringify(CONFIG_TEMPLATE_DETAIL.document, null, 2),
    );
    await user.click(screen.getByRole("button", { name: /save template/i }));

    await waitFor(() => {
      expect(submitted).toBeDefined();
    });
    expect(submitted?.document).toEqual(CONFIG_TEMPLATE_DETAIL.document);
  });
});
