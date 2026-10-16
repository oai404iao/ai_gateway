# Responses API 参考

> 类型：外部参考与网关兼容边界。
>
> 最近核对：2026-08-04。外部连接时限等观察保留该日期，不代表重新核验。
>
> 权威来源：[Create a model response](https://developers.openai.com/api/reference/resources/responses/methods/create)、
> [Streaming API responses](https://developers.openai.com/api/docs/guides/streaming-responses)、
> [WebSocket mode](https://developers.openai.com/api/docs/guides/websocket-mode)。

## 选择传输

| 需要 | 客户端入口 | 网关操作 |
| --- | --- | --- |
| 一次 JSON 或 SSE 调用 | `POST /v1/responses` | `responses` |
| 顺序交互和连接级增量上下文 | `GET /v1/responses` Upgrade | `responses-ws` |

两者都属于 `open_ai_responses`，但能力和模型操作路由分别配置，不能借用对方路由。
订阅凭证后端另有[Codex 限制](codex-oauth-connect.md)，不是所有普通 Responses 都必须使用 SSE。

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

等待本条终态后才能发下一条。不要把 HTTP `stream` 请求方式与 WS 事件封装混为一谈。

## 外部接口关键语义

Responses 以 `model`、`input` 为核心，instructions、tools 和结构化输出依模型能力而定。
非流式返回 Response 对象；SSE 返回有类型的事件，不能把每个 `data:` 当文本 delta。
核对时成功流通常包含 `response.completed`，失败可能包含 `response.failed`。

官方 WebSocket 模式发送 `response.create` 并接收 Responses 事件对象，一次只能有一个
Response 在途。`previous_response_id` 的增量缓存属于具体连接，断线后不可假定仍存在；
核对时官方连接最长 60 分钟。其他上游须单独确认这些约束。

## ai-gateway 行为

### HTTP 与事件

- HTTP 和 WS 分别检查顶层白名单；未知字段本地 `400`，允许字段内部由上游或插件解释。
- 不在 `input` 与 Chat `messages` 之间转换。模型别名只改顶层 `model`；无任何 body
  修改时保留原始字节。
- HTTP `Content-Encoding: zstd` 在大小限制内解码后才检查策略；其他 JSON 操作不因此获得 zstd 入口。
- Responses HTTP 操作能力可显式选择出站 zstd：只编码适配后的最终 HTTP Responses JSON，
  不用于 WS、search 或 Images。
- JSON/SSE 默认逐块转发。Responses SSE 规则按事件类型选择；WS 可使用相同事件选择器和
  JSON patch，但没有 SSE 文本 envelope。显式响应适配仍限定同格式。
- 内置 Responses usage 支持 `input_tokens`、`output_tokens`、缓存命中/写入和 reasoning
  细分；插件供应商解析见[usage 规范化](../development/usage-normalization.md)。
- `response.completed` / `response.failed` 可在 EOF 前确定日志终态。

### WebSocket 状态与复用

Upgrade 验证 API Key，每个 `response.create` 再独立执行鉴权、准入、选路、日志和计费。
系统、用户和选中操作能力的 WS 条件均须满足；不可用时用通用 `426 websocket_unavailable`，
不泄露内部渠道或插件状态。

成功终态且无残留事件的上游连接才返回有界池，同 Session 优先复用。池身份包括 API Key、
下游握手身份、渠道及凭证/接入版本、上游模型、插件 generation、网络策略、目标和最终 Header。
轮换或改绑不会因为密钥文本相同而复用旧身份。

池参数由运行时设置控制：默认最多 128 条空闲连接、空闲 300 秒、总龄 3300 秒；不是固定协议限制。
同一精确身份只保留最近一条。非空 `previous_response_id` 必须精确命中连接，否则在新派发前返回
`404 previous_response_not_found`，由客户端重发完整请求。

`websocket_connection_limit_reached`、`previous_response_not_found` 保留原形状，不做事件 patch，
并废弃该连接；不因此自动禁用渠道或清掉 affinity。仅上游 Upgrade 前连接失败可故障转移；
消息发出后不重试，不复制生成。

下游 Upgrade 先于上游握手，所以上游握手响应 Header 和相应变换不能回填下游握手。
请求 Header/JSON 和事件变换仍可生效。缺少时默认注入
`OpenAI-Beta: responses_websockets=2026-02-06`，普通渠道 Header 变换可覆盖；Codex 由插件约束。

## 接入检查

1. 分别配置并测试 `responses`、`responses-ws`，确认路径、鉴权和模型目录。
2. 核对 JSON usage，SSE `event:` 与 JSON `type`，以及成功、失败和断连终态。
3. 核对 tools、文件/图片、持久化和 cache 字段，不把入口放行当能力证明。
4. WS 测试连续请求、精确连接复用、断线后的完整重发、代理及自定义 Beta Header。
5. 区分客户端 fallback 与服务端重试；网关消息发送后不重放。

更多：[请求白名单](request-allowlists.md)、[Codex WS 研究](codex-responses-websocket.md)、
[上游凭证轮换](../user/upstream-credentials.md)、[真实上游 smoke](../development/real-upstream-smoke.md)。
