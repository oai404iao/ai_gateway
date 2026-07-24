# Responses API 参考

> 类型：外部参考。
>
> 最近核对：2026-07-23。
>
> 权威来源：[Create a model response](https://developers.openai.com/api/reference/resources/responses/methods/create) 和 [Streaming API responses](https://developers.openai.com/api/docs/guides/streaming-responses)。

## 外部接口关键语义

- 请求路径是 `POST /v1/responses`。
- 请求以 `model` 和 `input` 为核心，也可按模型能力使用 instructions、tools、结构化输出等字段。
- 非流式响应返回 Response JSON 对象。
- `stream=true` 时，响应是带语义事件类型的 SSE 流；客户端应根据 `type` 处理事件，而不是假设所有 `data:` 都是文本 delta。
- 成功流通常包含 `response.completed`；失败流可能包含 `response.failed`。完整事件集合以官方文档为准。

## ai-gateway 行为

- 此路径只匹配 `open_ai_responses` 模型规则、渠道组和渠道。
- 网关不会把 `input` 转换为 Chat Completions `messages`，也不会反向转换响应。
- 除最小路由字段和已配置变换外，请求字段由目标上游解释。
- 无变换时保留原始请求字节；模型别名只改写顶层 `model`。
- 普通 JSON 和 SSE 响应默认流式透传。
- SSE 变换按 Responses 事件类型匹配，不能使用 Chat Completions 事件规则。
- usage 采集识别 `input_tokens`、`output_tokens` 和 `input_tokens_details.cached_tokens`。
- 日志识别 `response.completed` 和 `response.failed` 作为应用层终态；终态可在底层连接 EOF 前完成日志结果判断。

## 接入检查

接入新的 Responses 兼容上游时，至少验证：

1. base URL 与 `/v1/responses` 拼接正确。
2. 非流式 Response 和 `usage` 字段形状。
3. SSE 是否同时提供 `event:` 与 JSON `type`，以及两者是否一致。
4. 成功、失败和连接中断分别使用什么终态事件。
5. tools、图片/文件输入或 provider 扩展是否真的被目标上游支持。
6. 上游是否对持久化、会话引用或 prompt cache 字段有额外语义。

转发路径变更后，按 [真实上游 smoke test](../development/real-upstream-smoke.md) 验证。
