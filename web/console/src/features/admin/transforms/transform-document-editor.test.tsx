import { useState } from "react";
import { beforeEach, describe, expect, it } from "vitest";
import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nProvider } from "@/app/i18n-provider";
import { STORAGE_KEY, setCurrentLocale } from "@/app/i18n";
import type { ApiFormat, ApiOperation } from "@/api/types";
import { TransformDocumentEditor } from "@/features/admin/transforms/transform-document-editor";

interface EditorOptions {
  initialValue?: string;
  defaultApiFormat?: ApiFormat;
  operation?: ApiOperation;
}

function EditorHarness({
  initialValue = "{}",
  defaultApiFormat,
  operation,
}: EditorOptions) {
  const [value, setValue] = useState(initialValue);
  const [validation, setValidation] = useState<string | null>(null);
  return (
    <>
      <TransformDocumentEditor
        value={value}
        onChange={setValue}
        defaultApiFormat={defaultApiFormat}
        operation={operation}
        onVisualValidationChange={setValidation}
      />
      <output data-testid="transform-document">{value}</output>
      <output data-testid="transform-validation">{validation ?? ""}</output>
    </>
  );
}

function renderEditor(options: EditorOptions = {}) {
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

  it("keeps gateway-owned request content encoding out of configuration", async () => {
    const user = userEvent.setup();
    renderEditor();

    await user.click(screen.getByRole("button", { name: "Add request header rule" }));
    await user.type(screen.getByLabelText("Header name"), "content-encoding");

    expect(screen.getByTestId("transform-validation")).toHaveTextContent(
      "Header names must be valid and cannot be protected headers.",
    );
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

  it("allows Images request transforms but disables SSE rules", async () => {
    const user = userEvent.setup();
    renderEditor({
      initialValue: "",
      defaultApiFormat: "open_ai_images",
    });

    expect(
      screen.getByRole("button", { name: "Add streaming response rule" }),
    ).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Add request body rule" }));
    await user.type(screen.getByLabelText("JSON pointer"), "/quality");
    fireEvent.change(screen.getByLabelText("Value (JSON)"), {
      target: { value: '"high"' },
    });

    expect(JSON.parse(screen.getByTestId("transform-document").textContent ?? "")).toEqual({
      version: 1,
      api_format: "open_ai_images",
      request_json: [
        {
          op: "add",
          path: "/quality",
          value: "high",
        },
      ],
    });
  });

  it.each(["images_edit", "web_search"] as const)("restricts body rules for %s while allowing removal", async (operation) => {
    const user = userEvent.setup();
    const api_format = operation === "images_edit" ? "open_ai_images" : "open_ai_responses";
    const initialValue = JSON.stringify({ version: 1, api_format, request_json: [{ op: "remove", path: "/foo" }] });
    renderEditor({ operation, initialValue });

    expect(screen.getByTestId("transform-document")).toHaveTextContent(initialValue);
    expect(screen.getByTestId("transform-validation")).toHaveTextContent("This operation does not support request-body rules.");
    expect(screen.getByRole("button", { name: "Add request body rule" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Add streaming response rule" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Delete rule" }));
    expect(screen.getByTestId("transform-validation")).toBeEmptyDOMElement();
    expect(JSON.parse(screen.getByTestId("transform-document").textContent ?? "")).toEqual({});

    await user.click(screen.getByRole("tab", { name: "Reference" }));
    expect(screen.queryByText("Request body example")).not.toBeInTheDocument();
    expect(screen.queryByText("Streaming response example")).not.toBeInTheDocument();
    expect(screen.getByText("Request header example")).toBeVisible();
  });

  it("validates raw capability JSON without replacing the input or editing a stale visual document", async () => {
    const user = userEvent.setup();
    renderEditor({ operation: "web_search" });
    await user.click(screen.getByRole("tab", { name: "JSON configuration" }));
    const raw = screen.getByLabelText("Transform document JSON");
    const invalidFormat = JSON.stringify({ version: 1, api_format: "open_ai_chat_completions" });
    fireEvent.change(raw, { target: { value: invalidFormat } });
    expect(raw).toHaveValue(invalidFormat);
    expect(screen.getByTestId("transform-validation")).toHaveTextContent("The transform's API format must match the operation.");

    fireEvent.change(raw, { target: { value: JSON.stringify({ version: 1, api_format: "open_ai_responses", sse: [{ event: "response.completed", json: [] }] }) } });
    expect(screen.getByTestId("transform-validation")).toHaveTextContent("This operation does not support streaming response rules.");
    fireEvent.change(raw, { target: { value: "" } });
    expect(screen.getByTestId("transform-validation")).toHaveTextContent("Transform JSON is not valid.");
    await user.click(screen.getByRole("tab", { name: "Visual editor" }));
    expect(screen.queryByRole("button", { name: "Add request header rule" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: "JSON configuration" }));
    fireEvent.change(screen.getByLabelText("Transform document JSON"), { target: { value: "{}" } });
    expect(screen.getByTestId("transform-validation")).toBeEmptyDOMElement();
  });

  it("applies the proxy and CDN request-header filter template", async () => {
    const user = userEvent.setup();
    renderEditor();

    await user.click(screen.getByRole("tab", { name: "Reference" }));
    const exampleTitle = screen.getByText("Proxy and CDN header filter template");
    const exampleCard = exampleTitle.closest('[data-slot="card"]');
    expect(exampleCard).not.toBeNull();
    await user.click(
      within(exampleCard as HTMLElement).getByRole("button", { name: "Apply example" }),
    );

    expect(JSON.parse(screen.getByTestId("transform-document").textContent ?? "")).toEqual({
      version: 1,
      api_format: "open_ai_chat_completions",
      request_headers: {
        remove: [
          "forwarded",
          "via",
          "x-forwarded-for",
          "x-forwarded-host",
          "x-forwarded-proto",
          "x-forwarded-port",
          "x-real-ip",
          "x-client-ip",
          "x-original-forwarded-for",
          "true-client-ip",
          "cf-connecting-ip",
          "cf-connecting-ipv6",
          "cf-pseudo-ipv4",
          "cf-ipcountry",
          "cf-ray",
          "cf-visitor",
        ],
      },
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
