import { render, screen, waitFor } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { http, HttpResponse } from "msw"
import { BrowserRouter } from "react-router"
import { describe, expect, it } from "vitest"

import type { InviteUserInput } from "@/api/types"
import { AppProviders } from "@/app/providers"
import { AppRouter } from "@/app/router"
import { server, seedAuthenticatedSession } from "@/test/msw"
import { CONTROL_PLANE_USER, DEFAULT_USER_GROUP } from "@/test/fixtures"

function renderAppAt(path: string) {
  window.history.replaceState({}, "", path)
  render(
    <AppProviders>
      <BrowserRouter>
        <AppRouter />
      </BrowserRouter>
    </AppProviders>,
  )
}

describe("UsersPage", () => {
  it("submits default Select values without requiring reselection", async () => {
    seedAuthenticatedSession()
    let submitted: InviteUserInput | undefined
    server.use(
      http.post("/console/v1/users", async ({ request }) => {
        submitted = (await request.json()) as InviteUserInput
        return HttpResponse.json(
          {
            id: "00000000-0000-0000-0000-000000000041",
            user_id: "00000000-0000-0000-0000-000000000042",
            invitation_token: "invite-once",
            expires_at: "2026-07-21T00:00:00.000Z",
            correlation_id: "00000000-0000-0000-0000-000000000043",
          },
          { status: 201 },
        )
      }),
    )
    const user = userEvent.setup()
    renderAppAt("/admin/users")

    await user.click(await screen.findByRole("button", { name: /invite user/i }))
    await user.type(screen.getByLabelText("Email"), "new-user@example.test")
    await user.type(screen.getByLabelText("Display name"), "New User")
    const initialBalance = screen.getByLabelText(/initial balance/i)
    await user.clear(initialBalance)
    await user.type(initialBalance, "50")
    await user.click(screen.getByRole("button", { name: /send invitation/i }))

    await waitFor(() => {
      expect(submitted).toEqual({
        email: "new-user@example.test",
        display_name: "New User",
        role: "user",
        user_group_id: DEFAULT_USER_GROUP.id,
        initial_balance_amount: "50",
        default_api_key_policy_id: null,
      })
    })
    expect(await screen.findByText("Invitation token")).toBeInTheDocument()
  })

  it("preserves selected users across server-paginated pages", async () => {
    seedAuthenticatedSession()
    const userPage2 = {
      ...CONTROL_PLANE_USER,
      id: "00000000-0000-0000-0000-000000000099",
      display_name: "Second Page User",
      email: "second@example.test",
    }
    server.use(
      http.get("/console/v1/users", ({ request }) => {
        const cursor = new URL(request.url).searchParams.get("cursor")
        if (cursor === "page-2") {
          return HttpResponse.json({ items: [userPage2], next_cursor: null, total: 2 })
        }
        return HttpResponse.json({ items: [CONTROL_PLANE_USER], next_cursor: "page-2", total: 2 })
      }),
    )
    const user = userEvent.setup()
    renderAppAt("/admin/users")

    await user.click(await screen.findByRole("checkbox", { name: `Select ${CONTROL_PLANE_USER.display_name}` }))
    expect(screen.getByRole("status")).toHaveTextContent("1 users selected")

    await user.click(screen.getByRole("button", { name: "Go to next page" }))
    await user.click(await screen.findByRole("checkbox", { name: `Select ${userPage2.display_name}` }))
    expect(screen.getByRole("status")).toHaveTextContent("2 users selected")
    expect(screen.getByRole("button", { name: "Batch edit (2)" })).toBeEnabled()
  })
})
