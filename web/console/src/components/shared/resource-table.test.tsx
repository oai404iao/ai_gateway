import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { AppProviders } from "@/app/providers";
import { ResourceTable } from "@/components/shared/resource-table";
import { seedAuthenticatedSession } from "@/test/msw";

interface TestRow {
  id: string;
  name: string;
  provider: string;
}

function renderTable(rows: TestRow[]) {
  render(
    <AppProviders>
      <ResourceTable
        columns={[{ key: "name", header: "Name", render: (row) => row.name }]}
        rows={rows}
        rowKey={(row) => row.id}
        groupBy={(row) => row.provider}
      />
    </AppProviders>,
  );
}

describe("ResourceTable", () => {
  it("groups rows and paginates large collections", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    const rows = Array.from({ length: 25 }, (_, index) => ({
      id: String(index + 1),
      name: `Model ${index + 1}`,
      provider: index < 20 ? "OpenAI" : "Anthropic",
    }));
    renderTable(rows);

    expect(screen.getByText("OpenAI")).toBeInTheDocument();
    expect(screen.getByText("Model 20")).toBeInTheDocument();
    expect(screen.queryByText("Model 21")).not.toBeInTheDocument();

    await user.click(screen.getByRole("link", { name: /go to next page/i }));

    expect(screen.getByText("Anthropic")).toBeInTheDocument();
    expect(screen.getByText("Model 21")).toBeInTheDocument();
    expect(screen.queryByText("Model 20")).not.toBeInTheDocument();
  });
});
