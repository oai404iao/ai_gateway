import { useState } from "react";
import { beforeEach, describe, expect, it } from "vitest";
import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nProvider } from "@/app/i18n-provider";
import { STORAGE_KEY, setCurrentLocale } from "@/app/i18n";
import type { ApiFormat } from "@/api/types";
import { TransformDocumentEditor } from "@/features/admin/transforms/transform-document-editor";

function EditorHarness({
  initialValue = "{}",
  defaultApiFormat,
}: {
  initialValue?: string;
  defaultApiFormat?: ApiFormat;
}) {
  const [value, setValue] = useState(initialValue);
  const [validation, setValidation] = useState<string | null>(null);
  return (
    <>
      <TransformDocumentEditor
        value={value}
        onChange={setValue}
        defaultApiFormat={defaultApiFormat}
        onVisualValidationChange={setValidation}
      />
      <output data-testid="transform-document">{value}</output>
      <output data-testid="transform-validation">{validation ?? ""}</output>
    </>
  );
}

function renderEditor(options: { initialValue?: string; defaultApiFormat?: ApiFormat } = {}) {
  render(
    <I18nProvider>
      <EditorHarness {...options} />
    </I18nProvider>,
  );
}

describe("TransformDocumentEditor", () => {
  beforeEach(() => {
    window.localStorage.setItem(STORAGE_KEY, "en-US");
    setCurrentLocale("en-US");
  });

  it("builds a constrained request-header document from visual rules", async () => {
    const user = userEvent.setup();
    renderEditor();

    await user.click(screen.getByRole("button", { name: "Add request header rule" }));
    await user.type(screen.getByLabelText("Header name"), "x-gateway-source");
    await user.type(screen.getByLabelText("Header value"), "console");

    expect(JSON.parse(screen.getByTestId("transform-document").textContent ?? "")).toEqual({
      version: 1,
      api_format: "open_ai_chat_completions",
      request_headers: {
        set: {
          "x-gateway-source": "console",
        },
      },
    });
    expect(screen.getByTestId("transform-validation")).toHaveTextContent("");
  });

  it("keeps protected request headers out of generated configuration", async () => {
    const user = userEvent.setup();
    renderEditor();

    await user.click(screen.getByRole("button", { name: "Add request header rule" }));
    await user.type(screen.getByLabelText("Header name"), "authorization");

    expect(screen.getByTestId("transform-validation")).toHaveTextContent(
      "Header names must be valid and cannot be protected headers.",
    );
    expect(
      JSON.parse(screen.getByTestId("transform-document").textContent ?? "").request_headers.set,
    ).not.toHaveProperty("authorization");
  });

  it("generates a version-two array operation from the visual editor", async () => {
    const user = userEvent.setup();
    renderEditor();

    await user.click(screen.getByRole("button", { name: "Add request body rule" }));
    await user.click(screen.getByLabelText("JSON Patch operation"));
    await user.click(await screen.findByRole("option", { name: "Append to array" }));
    await user.type(screen.getByLabelText("JSON pointer"), "/messages");
    const value = screen.getByLabelText("Value (JSON)");
    fireEvent.change(value, { target: { value: '{"role":"system"}' } });

    expect(JSON.parse(screen.getByTestId("transform-document").textContent ?? "")).toEqual({
      version: 2,
      api_format: "open_ai_chat_completions",
      request_json: [
        {
          op: "array_append",
          path: "/messages",
          value: {
            role: "system",
          },
        },
      ],
    });
  });

  it("uses the template metadata format when replacing a redacted document", async () => {
    const user = userEvent.setup();
    renderEditor({
      initialValue: "",
      defaultApiFormat: "open_ai_responses",
    });

    await user.click(screen.getByRole("button", { name: "Add request header rule" }));
    await user.type(screen.getByLabelText("Header name"), "x-gateway-source");
    await user.type(screen.getByLabelText("Header value"), "console");

    expect(JSON.parse(screen.getByTestId("transform-document").textContent ?? "")).toMatchObject({
      api_format: "open_ai_responses",
    });
  });

  it("applies a version-two array rewrite reference example", async () => {
    const user = userEvent.setup();
    renderEditor();

    await user.click(screen.getByRole("tab", { name: "Reference" }));
    const exampleTitle = screen.getByText("Array rewrite example");
    const exampleCard = exampleTitle.closest('[data-slot="card"]');
    expect(exampleCard).not.toBeNull();
    await user.click(within(exampleCard as HTMLElement).getByRole("button", { name: "Apply example" }));

    expect(JSON.parse(screen.getByTestId("transform-document").textContent ?? "")).toEqual({
      version: 2,
      api_format: "open_ai_chat_completions",
      request_json: [
        {
          op: "array_prepend",
          path: "/messages",
          value: {
            role: "system",
            content: "Follow the gateway policy.",
          },
          when: {
            type: "array",
          },
        },
      ],
    });
  });

  it("applies a version-two current-value reference example", async () => {
    const user = userEvent.setup();
    renderEditor();

    await user.click(screen.getByRole("tab", { name: "Reference" }));
    const exampleTitle = screen.getByText("Current-value reference example");
    const exampleCard = exampleTitle.closest('[data-slot="card"]');
    expect(exampleCard).not.toBeNull();
    await user.click(within(exampleCard as HTMLElement).getByRole("button", { name: "Apply example" }));

    expect(JSON.parse(screen.getByTestId("transform-document").textContent ?? "")).toEqual({
      version: 2,
      api_format: "open_ai_chat_completions",
      request_json: [
        {
          op: "replace",
          path: "/metadata",
          value: {
            original: {
              $ref: "current",
            },
            gateway: "console",
          },
          when: {
            type: "object",
          },
        },
      ],
    });
  });

  it("applies a streaming response reference example", async () => {
    const user = userEvent.setup();
    renderEditor();

    await user.click(screen.getByRole("tab", { name: "Reference" }));
    const exampleTitle = screen.getByText("Streaming response example");
    const exampleCard = exampleTitle.closest('[data-slot="card"]');
    expect(exampleCard).not.toBeNull();
    await user.click(within(exampleCard as HTMLElement).getByRole("button", { name: "Apply example" }));

    expect(JSON.parse(screen.getByTestId("transform-document").textContent ?? "")).toEqual({
      version: 1,
      api_format: "open_ai_chat_completions",
      sse: [
        {
          event: "chat.completion.chunk",
          json: [
            {
              op: "add",
              path: "/choices/0/delta/gateway_trace",
              value: "proxied",
            },
          ],
        },
      ],
    });
    expect(screen.getByRole("tab", { name: "Visual editor" })).toHaveAttribute(
      "data-active",
    );
  });
});
