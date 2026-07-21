import { afterEach, describe, expect, it } from "vitest";
import { setCurrentLocale } from "@/app/i18n";
import { apiFormatLabel } from "@/lib/permissions";

afterEach(() => {
  setCurrentLocale("en-US");
});

describe("apiFormatLabel", () => {
  it("keeps OpenAI API format names unchanged in Chinese", () => {
    setCurrentLocale("zh-CN");

    expect(apiFormatLabel("open_ai_chat_completions")).toBe("Chat Completions");
    expect(apiFormatLabel("open_ai_responses")).toBe("Responses");
  });
});
