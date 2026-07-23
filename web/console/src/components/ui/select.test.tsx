import { render, screen, waitFor } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"

describe("Select", () => {
  it("renders the selected label without emitting an empty change", async () => {
    const onValueChange = vi.fn()

    render(
      <form>
        <Select value="admin" onValueChange={onValueChange}>
          <SelectTrigger aria-label="Role">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectGroup>
              <SelectItem value="user">User</SelectItem>
              <SelectItem value="admin">Administrator</SelectItem>
            </SelectGroup>
          </SelectContent>
        </Select>
      </form>,
    )

    await waitFor(() => {
      expect(screen.getByLabelText("Role")).toHaveTextContent("Administrator")
    })
    expect(onValueChange).not.toHaveBeenCalled()
  })
})
