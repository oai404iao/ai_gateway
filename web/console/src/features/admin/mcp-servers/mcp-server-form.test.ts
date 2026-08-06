import { describe, expect, it } from "vitest";
import {
  defaultMcpServerFormValues,
  mcpServerCreateInput,
  mcpServerFormSchema,
  mcpServerInput,
} from "@/features/admin/mcp-servers/mcp-server-form";

describe("MCP server form", () => {
  it("normalizes web search domains and builds typed settings", () => {
    const values = mcpServerFormSchema.parse({
      ...defaultMcpServerFormValues,
      slug: "research",
      name: "Research search",
      description: "  Search public sources.  ",
      model_rule_id: "00000000-0000-0000-0000-000000000125",
      external_web_access: "indexed",
      search_context_size: "high",
      allowed_domains: [" Example.COM ", "", "docs.example.com"],
      blocked_domains: [" Ads.Example.com "],
      max_output_tokens_short: 800,
      max_output_tokens_medium: 2_400,
      max_output_tokens_long: 5_000,
    });

    expect(mcpServerCreateInput(values)).toEqual({
      slug: "research",
      kind: "web_search",
      name: "Research search",
      description: "Search public sources.",
      model_rule_id: "00000000-0000-0000-0000-000000000125",
      settings: {
        external_web_access: "indexed",
        search_context_size: "high",
        allowed_domains: ["example.com", "docs.example.com"],
        blocked_domains: ["ads.example.com"],
        max_output_tokens: {
          short: 800,
          medium: 2_400,
          long: 5_000,
        },
      },
      enabled: true,
    });
  });

  it("rejects overlapping domains and unordered token limits", () => {
    const result = mcpServerFormSchema.safeParse({
      ...defaultMcpServerFormValues,
      slug: "research",
      name: "Research search",
      model_rule_id: "00000000-0000-0000-0000-000000000125",
      allowed_domains: ["Example.com"],
      blocked_domains: ["example.COM"],
      max_output_tokens_short: 4_000,
      max_output_tokens_medium: 3_000,
      max_output_tokens_long: 6_000,
    });

    expect(result.success).toBe(false);
    if (result.success) return;
    expect(result.error.flatten().fieldErrors).toMatchObject({
      blocked_domains: [
        "A domain cannot appear in both allowed and blocked lists.",
      ],
      max_output_tokens_short: [
        "Token limits must be ordered as short ≤ medium ≤ long.",
      ],
    });
  });

  it("accepts canonical image dimensions and rejects out-of-range sizes", () => {
    const values = mcpServerFormSchema.parse({
      ...defaultMcpServerFormValues,
      slug: "studio",
      kind: "image",
      name: "Image studio",
      description: "",
      model_rule_id: "00000000-0000-0000-0000-000000000126",
      image_background: "transparent",
      image_quality: "high",
      image_size: " 1536X1024 ",
    });

    expect(mcpServerInput(values)).toEqual({
      name: "Image studio",
      description: null,
      model_rule_id: "00000000-0000-0000-0000-000000000126",
      settings: {
        background: "transparent",
        quality: "high",
        size: "1536x1024",
      },
      enabled: true,
    });

    expect(
      mcpServerFormSchema.safeParse({
        ...values,
        image_size: "32x1024",
      }).success,
    ).toBe(false);
  });
});
