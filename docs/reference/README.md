# 协议与外部参考

> 类型：外部参考索引。

[OpenAI API 兼容性总览](openai-compatibility.md)列出网关支持的方法、路径和响应方式。外部语义以各页注明的官方来源、版本和核对日期为准。

| 要查的问题 | 参考 |
| --- | --- |
| 哪些字段会被转发？ | [请求白名单](request-allowlists.md) · [机器可读策略](request-allowlists.json) |
| 如何发送消息、工具调用和流式请求？ | [Chat Completions](chat-completions.md) · [Responses](responses.md) |
| 图片生成与编辑接受什么输入？ | [Images](openai-images.md) |
| Codex 如何认证、搜索和续接上下文？ | [OAuth 与搜索](codex-oauth-connect.md) · [WebSocket 源码研究](codex-responses-websocket.md) |
| 哪些凭证文件可以导入？ | [Codex 导入格式](codex-credential-portability.md) |
| 定价预设和代理诊断依据什么？ | [DeepSeek 峰谷定价](deepseek-peak-pricing.md) · [ip-api.com](ip-api-proxy-test.md) |

实际调用和配置步骤见[用户指南](../user/README.md)。
