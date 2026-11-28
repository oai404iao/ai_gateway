import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createMemoryRouter, RouterProvider } from "react-router";
import { I18nProvider } from "@/app/i18n-provider";
import { useConfigurationDraft } from "./use-configuration-draft";
import { useState } from "react";

function DraftHarness() {
  const [externalDirty, setExternalDirty] = useState(true);
  const draft = useConfigurationDraft(false, externalDirty);

  return (
    <div>
      {draft.navigationGuard}
      <button
        type="button"
        onClick={() => {
          // Simulate calling markSaved() before form.reset() flushes externalDirty=false.
          draft.markSaved();
          draft.navigate("/next");
        }}
      >
        Save and leave
      </button>
      <button type="button" onClick={() => setExternalDirty(false)}>
        Reset dirty
      </button>
      <button type="button" onClick={() => setExternalDirty(true)}>
        Edit again
      </button>
      <button type="button" onClick={() => draft.navigate("/next")}>
        Leave
      </button>
    </div>
  );
}

function renderDraftRouter() {
  const router = createMemoryRouter(
    [
      { path: "/", element: <DraftHarness /> },
      { path: "/next", element: <div>Next page</div> },
    ],
    { initialEntries: ["/"] },
  );
  render(
    <I18nProvider>
      <RouterProvider router={router} />
    </I18nProvider>,
  );
  return router;
}

describe("useConfigurationDraft", () => {
  it("allows navigation after markSaved even while externalDirty has not flushed", async () => {
    const user = userEvent.setup();
    renderDraftRouter();

    await user.click(screen.getByRole("button", { name: "Save and leave" }));
    expect(await screen.findByText("Next page")).toBeInTheDocument();
    expect(screen.queryByRole("alertdialog", { name: "Discard unsaved changes?" })).not.toBeInTheDocument();
  });

  it("re-enables navigation blocking when externalDirty transitions back to true", async () => {
    const user = userEvent.setup();
    renderDraftRouter();

    await user.click(screen.getByRole("button", { name: "Reset dirty" }));
    await user.click(screen.getByRole("button", { name: "Edit again" }));
    await user.click(screen.getByRole("button", { name: "Leave" }));

    expect(await screen.findByRole("alertdialog", { name: "Discard unsaved changes?" })).toBeInTheDocument();
  });
});
