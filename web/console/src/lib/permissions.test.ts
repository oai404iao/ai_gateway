import { afterEach, describe, expect, it } from "vitest";
import { setCurrentLocale } from "@/app/i18n";
import { API_OPERATIONS, apiFormatLabel, apiOperationLabel } from "@/lib/permissions";

afterEach(() => {
  setCurrentLocale("en-US");
});

describe("apiOperationLabel", () => {
  it("keeps capability names in English in Chinese", () => {
    setCurrentLocale("zh-CN");

    expect(API_OPERATIONS.map(apiOperationLabel)).toEqual([
      "Chat Completions", "Responses", "Responses WebSocket", "Standalone web search",
      "Images generation", "Images edit",
    ]);
  });
});

describe("apiFormatLabel", () => {
  it("keeps OpenAI API format names unchanged in Chinese", () => {
    setCurrentLocale("zh-CN");

    expect(apiFormatLabel("open_ai_chat_completions")).toBe("Chat Completions");
    expect(apiFormatLabel("open_ai_responses")).toBe("Responses");
    expect(apiFormatLabel("open_ai_images")).toBe("Images");
  });
});
