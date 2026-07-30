# Responses API 参考

> 类型：外部参考。
>
> 最近核对：2026-07-27。
>
> 权威来源：[Create a model response](https://developers.openai.com/api/reference/resources/responses/methods/create)、[Streaming API responses](https://developers.openai.com/api/docs/guides/streaming-responses) 和 [WebSocket mode](https://developers.openai.com/api/docs/guides/websocket-mode)。

## 外部接口关键语义

- 请求路径是 `POST /v1/responses`。
- 请求以 `model` 和 `input` 为核心，也可按模型能力使用 instructions、tools、结构化输出等字段。
- 非流式响应返回 Response JSON 对象。
- `stream=true` 时，响应是带语义事件类型的 SSE 流；客户端应根据 `type` 处理事件，而不是假设所有 `data:` 都是文本 delta。
- 成功流通常包含 `response.completed`；失败流可能包含 `response.failed`。完整事件集合以官方文档为准。
- WebSocket 模式通过 `/v1/responses` Upgrade 建立长连接，客户端发送
  `response.create` JSON 文本消息，并接收与 SSE 相同的 Responses 事件对象。
- 一个 WebSocket 同时只能有一个 Response 在途；后续请求必须等待前一个 Response 完成。
- `previous_response_id` 可让同一连接只发送新增输入；服务端缓存属于具体 WebSocket
  连接，断开后不可假设仍然存在。
- 官方服务当前把单条 Responses WebSocket 的最长连接时间限制为 60 分钟。

## ai-gateway 行为

- 此路径只匹配 `open_ai_responses` 模型规则、渠道组和渠道。
- 网关不会把 `input` 转换为 Chat Completions `messages`，也不会反向转换响应。
- 除最小路由字段和已配置变换外，请求字段由目标上游解释。
- 无变换时保留原始请求字节；模型别名只改写顶层 `model`。
- 普通 JSON 和 SSE 响应默认流式透传。
- SSE 变换按 Responses 事件类型匹配，不能使用 Chat Completions 事件规则。
- WebSocket 事件使用同一套 Responses 事件选择器和 JSON patch 规则，但没有 SSE 文本 envelope。
- usage 采集识别 `input_tokens`、`output_tokens`、
  `input_tokens_details.cached_tokens`、`input_tokens_details.cache_write_tokens`
  和 `output_tokens_details.reasoning_tokens`。
- 日志识别 `response.completed` 和 `response.failed` 作为应用层终态；终态可在底层连接 EOF 前完成日志结果判断。
- WebSocket Upgrade 使用 Gateway Bearer Key；每个 `response.create` 重新鉴权、独立准入、选路、
  usage、计费和日志。
- 下游未设置时，网关默认注入
  `OpenAI-Beta: responses_websockets=2026-02-06`；渠道请求 Header 变换可覆盖该值。
- 网关不在连接内多路复用；成功终态后的干净上游连接立即进入有界池，同一下游 Session 的下一条消息
  优先取回同一连接。池按 Gateway API Key、下游握手身份、渠道、目标、网络策略和最终 Header 精确
  隔离。
- 上游 WebSocket 池最多保留 128 条空闲连接；同一精确身份只保留最近一条，空闲 5 分钟或总龄
  55 分钟后淘汰，以避开官方 60 分钟上限。
- 上游 Upgrade 前的连接类失败可以按全局重试策略故障转移；请求消息发出后不重试。
- 上游握手发生在下游 Upgrade 和首条消息之后，因此上游握手响应 Header 与响应 Header 变换不能
  回填到下游握手。请求 Header、请求 JSON 和 Responses 事件变换仍然生效。

## 接入检查

接入新的 Responses 兼容上游时，至少验证：

1. base URL 与 `/v1/responses` 拼接正确。
2. 非流式 Response 和 `usage` 字段形状。
3. SSE 是否同时提供 `event:` 与 JSON `type`，以及两者是否一致。
4. 成功、失败和连接中断分别使用什么终态事件。
5. tools、图片/文件输入或 provider 扩展是否真的被目标上游支持。
6. 上游是否对持久化、会话引用或 prompt cache 字段有额外语义。
7. 上游是否支持 Responses WebSocket、要求哪个 `OpenAI-Beta` 值，以及连接时长限制。
8. `previous_response_id` 是否严格绑定 WebSocket 连接，并验证断线后的错误事件。

转发路径变更后，按 [真实上游 smoke test](../development/real-upstream-smoke.md) 验证。
Codex 的具体请求形状、连接缓存、增量请求和 fallback 实现见
[Codex Responses WebSocket 实现参考](codex-responses-websocket.md)。
