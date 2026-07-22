import { describe, expect, it } from "vitest";
import { translateFor } from "@/app/i18n";

describe("Console i18n", () => {
  it("translates channel automation controls and request-log sources into Chinese", () => {
    expect(translateFor("zh-CN", "Automatic channel disable")).toBe("渠道自动禁用");
    expect(translateFor("zh-CN", "Scheduled channel tests")).toBe("渠道定时测活");
    expect(translateFor("zh-CN", "Allow automatic disable")).toBe("允许自动禁用");
    expect(translateFor("zh-CN", "Scheduled test")).toBe("定时测试");
    expect(translateFor("zh-CN", "Client request")).toBe("客户端请求");
    expect(
      translateFor(
        "zh-CN",
        "Enter unique HTTP status codes from 100 through 599, separated by commas.",
      ),
    ).toBe("请输入以逗号分隔且唯一的 100 至 599 HTTP 状态码。");
    expect(translateFor("zh-CN", "Visual editor")).toBe("可视化编辑");
    expect(translateFor("zh-CN", "Response body (streaming SSE)")).toBe("响应体（流式 SSE）");
  });
});
