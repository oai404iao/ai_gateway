import { expect, it } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import type { AuditLogView } from "@/api/types";
import { seedAuthenticatedSession, server } from "@/test/msw";

it("identifies deleted objects by their recorded name and retains exact IDs for investigation", async () => {
  seedAuthenticatedSession();
  const event: AuditLogView = {
    id: "00000000-0000-0000-0000-000000000001",
    occurred_at: "2026-08-01T00:00:00Z",
    actor_user_id: null, actor_type: "system", actor_role: null,
    action: "delete", object_type: "proxy", object_id: "00000000-0000-0000-0000-000000000002",
    before_redacted: { name: "Retired proxy" }, after_redacted: {},
    correlation_id: "00000000-0000-0000-0000-000000000003", reason: null,
  };
  server.use(http.get("/console/v1/audit-logs", () => HttpResponse.json([event])));
  window.history.replaceState({}, "", "/admin/audit-logs");
  render(<AppProviders><BrowserRouter><AppRouter /></BrowserRouter></AppProviders>);
  const user = userEvent.setup();
  await user.type(await screen.findByRole("searchbox", { name: "Search records" }), "Retired proxy");
  const row = await screen.findByRole("row", { name: /Retired proxy/ });
  expect(within(row).queryByText(event.object_id)).not.toBeInTheDocument();
  await user.click(within(row).getByRole("button", { name: "Inspect delete · Retired proxy" }));
  const properties = within(await screen.findByRole("complementary", { name: "delete · Retired proxy" }));
  expect(properties.getByText(event.object_id)).toBeVisible();
  expect(properties.getByText(event.correlation_id!)).toBeVisible();
});
