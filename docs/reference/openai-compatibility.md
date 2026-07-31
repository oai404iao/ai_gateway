# OpenAI API 兼容性总览

> 类型：外部参考与项目兼容契约。
>
> 最近核对：2026-07-31。
>
> 权威来源：[OpenAI API Reference](https://developers.openai.com/api/reference/overview)。
>
> 相关字段来源：[OpenAI Reasoning](https://developers.openai.com/api/docs/guides/reasoning)、
> [OpenAI Priority processing](https://developers.openai.com/api/docs/guides/priority-processing)、
> [OpenAI Images API](https://developers.openai.com/api/reference/resources/images)、
> [DeepSeek Thinking Mode](https://api-docs.deepseek.com/guides/thinking_mode)。

## 支持范围

| 路径 | ai-gateway 行为 |
| --- | --- |
| `GET /health` | 网关自有存活检查，返回 `204`，不属于 OpenAI API。 |
| `GET /v1/models` | 返回当前 API Key 可达的客户端模型名，不返回完整上游目录。 |
| `POST /v1/chat/completions` | 仅按 Chat Completions 格式选路并转发。 |
| `POST /v1/responses` | 仅按 Responses 格式选路并转发。 |
| 带 WebSocket Upgrade 的 `GET /v1/responses` | 顺序转发 Responses WebSocket `response.create`；不做并发多路复用。 |
| `POST /v1/images/generations` | 仅按 Images 格式转发非流式 JSON generation 请求。 |

不支持 `/v1/images/edits`、multipart 图片请求、图片流式响应、embeddings、audio、files、
batches、assistants、fine-tuning 等其他 OpenAI 路径。

## 请求兼容策略

- 客户端使用 `Authorization: Bearer <gateway-api-key>`。
- 网关只做路由所需的最小 JSON 校验：body 必须是可解析的 JSON 对象，顶层
  `model` 必须是非空字符串且不超过 300 个字符；可选 `stream` 必须是布尔值。
- 除 `model`、`stream` 和已配置变换涉及的字段外，其余字段语义由上游决定。
- 仅为请求日志元数据，网关会宽松识别 Responses 的 `reasoning.effort`、兼容
  Chat Completions 的 `reasoning_effort`，以及 `service_tier = "priority"`；非字符串、
  过长或未知形状不会增加本地拒绝条件。
- 没有模型别名或 body 变换时，网关保留原始请求字节，不重新序列化。
- 模型别名只改写顶层 `model`。
- Images generation 的 `stream: true` 在本地返回
  `400 image_streaming_unsupported`；当前不会联系上游。
- 普通 `openai_compatible` Connector 会把查询字符串和原 API 路径拼接到渠道 `base_url`；
  provider Connector 可以按操作改写目标路径。
- 客户端 `Authorization`、`Host`、`Content-Length`、代理鉴权和 hop-by-hop headers 不会直接转发；上游鉴权最后注入。

因此，“网关接受某字段”不代表所有上游都支持该字段；同样，上游新增字段通常不需要网关先升级，只要该字段不触发本地变换限制。

## 响应兼容策略

- 收到上游响应头后，状态码和响应体默认透传。
- 普通 JSON 响应不会为了 usage 采集而整体缓冲。
- SSE 默认按字节流转发；只有启用 SSE 变换时才按事件边界解析和重写。
- 上游 HTTP 错误不会触发自动重试。
- Chat Completions 与 Responses 的自动故障转移仅发生在响应头前的连接失败、建连超时或
  响应头超时。Images generation 一旦开始上游尝试就不自动重试或切换渠道。
- 网关生成的本地错误使用 OpenAI 风格的 `{ "error": { ... } }` JSON 结构，但错误代码是本项目契约。

Responses WebSocket 的上游事件以 JSON 文本消息透传；配置的 Responses SSE 事件规则会应用到
同类型 WebSocket 事件。网关按顺序一次处理一个 `response.create`，并让同一下游 Session 优先取回
同一条上游连接，以保留 `previous_response_id` 的连接级缓存；只有成功、无残留的连接才会归还池中。
上游 WebSocket Upgrade 在首条消息之后发生，因此上游握手响应 Header 不会出现在已经完成的下游
Upgrade 响应中。

Codex OAuth managed channel 也走同一 Responses WebSocket 路径，但由 Connector 改写
`/responses` 目标、强制 `stream=true`/`store=false` 并注入订阅凭证与 Codex Header；
`previous_response_id` 仍只在同一条可复用上游连接上有效。

Codex OAuth Images projection 仍使用标准客户端 `/v1/images/generations`，Connector 将上游目标
改为 `/images/generations`，注入共享订阅凭证和 `x-codex-image-turn-id`，并按非流式 JSON
而不是 SSE 处理响应。它不会把 Responses 输入转换成 Images。

## 格式隔离

模型规则、渠道组和渠道都绑定一个 `api_format`。同一个客户端模型名若需要同时支持多个接口，
必须分别配置 Chat Completions、Responses 和 Images 路由。网关不会：

- 将 `/v1/chat/completions` 转换为 `/v1/responses`；
- 将 Responses 输入转换为 messages；
- 将 Responses 或 Chat Completions 请求转换为 Images generation；
- 在一个格式无可用路由时回退到另一格式。

## SDK 使用

兼容 OpenAI base URL 配置的 SDK 通常可以把 base URL 指向网关公共 listener，并使用网关 API Key。SDK 是否可用还取决于：

- SDK 是否调用本项目未实现的 OpenAI 路径；
- SDK 是否要求特定响应字段或事件；
- 目标上游是否支持 SDK 发送的请求字段；
- 是否配置了与调用路径格式一致的模型规则。

## 本地错误

常见本地错误包括：

| HTTP | `code` | 含义 |
| --- | --- | --- |
| `400` | `invalid_request` | body、`model` 或请求变换无效。 |
| `400` | `image_streaming_unsupported` | Images generation 请求设置了 `stream: true`。 |
| `401` | `invalid_api_key` | 缺少或无法认证 Gateway API Key。 |
| `403` | `permission_denied` | API Key 没有当前格式或模型列表权限。 |
| `404` | `model_not_found` | 模型不存在、未授权或当前格式没有路由。 |
| `413` | `request_too_large` | 超过配置的代理 body 限制。 |
| `429` | `rate_limit_exceeded` / `concurrent_limit_exceeded` / `insufficient_quota` | 进程内准入或软额度拒绝。 |
| `502` | `upstream_unavailable` / `response_transform_failed` | 上游连接或响应变换失败。 |
| `503` | `no_healthy_channel` | 没有可选择的健康渠道。 |
| `504` | `connect_timeout` / `response_header_timeout` | 响应头前超时。 |

上游已经返回的 HTTP 状态和 body 默认按上游内容传给客户端，不包装成本地错误。
