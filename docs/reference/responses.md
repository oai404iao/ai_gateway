# Responses API 参考

> 类型：外部参考与网关兼容边界。
>
> 最近核对：2026-08-04。
>
> 权威来源：[Create a model response](https://developers.openai.com/api/reference/resources/responses/methods/create)、
> [Streaming API responses](https://developers.openai.com/api/docs/guides/streaming-responses)、
> [WebSocket mode](https://developers.openai.com/api/docs/guides/websocket-mode)。

## 选择传输

| 需要 | 客户端入口 | 网关操作 |
| --- | --- | --- |
| 一次 JSON 或 SSE 调用 | `POST /v1/responses` | `responses` |
| 顺序交互和连接级增量上下文 | `GET /v1/responses` Upgrade | `responses-ws` |

两者都属于 `open_ai_responses`，要分别配置操作能力与模型路由。普通 HTTP 支持非流式请求；[Codex 订阅后端](codex-oauth-connect.md)要求 SSE。

## 最小请求

```bash
curl "$GATEWAY_BASE_URL/v1/responses" \
  -H "Authorization: Bearer $GATEWAY_API_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"model":"your-client-model","input":"Reply with OK."}'
```

`GATEWAY_BASE_URL` 不含 `/v1`。增加 `"stream":true` 可请求 SSE；请求会产生正常调用费用。
WebSocket 在握手中使用同一 Gateway Bearer Key，连接后发送：

```json
{"type":"response.create","model":"your-client-model","input":[{"role":"user","content":"Reply with OK."}]}
```

等本轮终态到达后再发送下一条 create。

## 外部接口关键语义

Responses 以 `model`、`input` 为核心，instructions、tools 和结构化输出依模型能力而定。
非流式返回 Response 对象；SSE 返回有类型的事件，不能把每个 `data:` 当文本 delta。
核对时成功流通常包含 `response.completed`，失败可能包含 `response.failed`。

官方 WebSocket 模式发送 `response.create` 并接收 Responses 事件对象，一次只能有一个
Response 在途。`previous_response_id` 的增量缓存属于具体连接，断线后不可假定仍存在；
核对时官方连接最长 60 分钟。其他上游须单独确认这些约束。

## ai-gateway 行为

### HTTP 与事件

- HTTP 请求与 WS `response.create` 分别检查顶层白名单；未知字段在入口静默删除，允许字段内部由上游或插件解释。`response.interrupt` 控制帧不接受额外字段。
- 不在 `input` 与 Chat `messages` 之间转换。模型别名只改顶层 `model`；无任何 body
  修改时保留原始字节。
- HTTP `Content-Encoding: zstd` 在大小限制内解码后才检查策略；其他 JSON 操作不因此获得 zstd 入口。
- Responses HTTP 操作能力可显式选择出站 zstd：只编码适配后的最终 HTTP Responses JSON，
  不用于 WS、search 或 Images。
- JSON/SSE 默认逐块转发。Responses SSE 规则按事件类型选择；WS 可使用相同事件选择器和
  JSON patch，但没有 SSE 文本 envelope。显式响应适配仍限定同格式。
- 内置 Responses usage 支持 `input_tokens`、`output_tokens`、缓存命中/写入和 reasoning
  细分；插件供应商解析见[usage 规范化](../development/usage-normalization.md)。
- `response.completed` / `response.failed` 可在 EOF 前确定日志终态。2xx SSE 在没有终态时结束，仍按上游响应体错误记日志，不向流中补写成功或失败事件。
- Codex 流的无状态码 `error` 按错误码区分：例如额度耗尽或过载可结束本轮，未知通知和 `response.cancelled` 仍转发，但不终结请求，也不提供计费用量。
  `response.incomplete` 的原因是 `interrupted` 时与 `response.completed` 一样检查完成对象：`id` 必须是字符串，若提供 usage、usage metadata 或 `end_turn`，其结构也必须有效。其他 incomplete 原因不记为成功。客户端的重试建议不授权网关重放请求。

### WebSocket 状态与复用

每条 `response.create` 独立鉴权、准入、选路、记录用量和计费。完整输入按路由选择上游；增量输入只发给同一会话、最近成功响应 ID 匹配且仍符合授权的上游连接。无法命中时返回 `404 previous_response_not_found`，客户端应清除增量状态，发送完整请求。

`websocket_connection_limit_reached` 和 `previous_response_not_found` 是状态恢复信号：原样转发错误，废弃连接，不禁用渠道。消息发出后不自动重试。客户端收到握手 `426` 或升级后的同码错误帧时，是否改用 HTTP 由客户端决定。

系统开关、Header、消息大小、连接池与超时见[使用 WebSocket](../user/websocket.md)。Codex 选择增量、处理中断和回退的条件见[客户端参考](codex-responses-websocket.md)。
