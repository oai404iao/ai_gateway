# OpenAI API 兼容性总览

> 类型：外部参考与网关兼容边界。
>
> 最近核对：2026-08-05。
>
> 权威来源：[OpenAI API Reference](https://developers.openai.com/api/reference/overview)。
> 相关来源：[Reasoning](https://developers.openai.com/api/docs/guides/reasoning)、
> [Priority processing](https://developers.openai.com/api/docs/guides/priority-processing)、
> [Images](https://developers.openai.com/api/reference/resources/images)、
> [DeepSeek Thinking Mode](https://api-docs.deepseek.com/guides/thinking_mode)、
> [百炼深度思考](https://help.aliyun.com/zh/model-studio/deep-thinking)。
> Search 来源为固定 [`openai/codex@5af85998`](https://github.com/openai/codex/tree/5af85998c24fb3353ddd8164c3ed472057b03cb3) 的
> [endpoint](https://github.com/openai/codex/blob/5af85998c24fb3353ddd8164c3ed472057b03cb3/codex-rs/codex-api/src/endpoint/search.rs) 与
> [wire types](https://github.com/openai/codex/blob/5af85998c24fb3353ddd8164c3ed472057b03cb3/codex-rs/codex-api/src/search.rs)。

OpenAI 兼容性描述的是客户端能发送的 HTTP 请求、WebSocket 消息及收到的响应。每种操作按自己的模型路由转发；管理 API 的资源与字段另见 [Console OpenAPI](../openapi/console-v1.yaml)。

## 支持范围

| 公共路径 | 操作名 | 请求 / 响应 | 详情 |
| --- | --- | --- | --- |
| `POST /v1/chat/completions` | `chat_completion` | JSON / JSON 或 SSE | [Chat](chat-completions.md) |
| `POST /v1/responses` | `responses` | JSON / JSON 或 SSE | [Responses](responses.md) |
| `GET /v1/responses` Upgrade | `responses-ws` | `response.create` / 顺序 WS 事件 | [WS 状态](responses.md#websocket-状态与复用) |
| `POST /v1/alpha/search` | `web_search` | JSON / 非流式 JSON | [Codex search](codex-oauth-connect.md) |
| `POST /v1/images/generations` | `images_generation` | JSON / 非流式 JSON | [Images](openai-images.md) |
| `POST /v1/images/edits` | `images_edit` | multipart / 非流式 JSON | [Images](openai-images.md) |
| `GET /v1/models` | — | 当前 Key 可达的客户端模型名 | 不是完整上游 catalog |
| `GET /health` | — | `204` 存活检查 | 网关自有接口，不检查所有依赖健康 |

独立搜索是 Codex 扩展，客户端 provider 需声明 `supports_standalone_web_search = true`。Images 编辑仅支持 multipart。embeddings、audio、files、batches、assistants 和 fine-tuning 等不在网关公共接口内。

## SDK 配置

```text
base URL: https://gateway.example.test/v1
API key:  <gateway-api-key>
model:    <configured-client-model>
```

使用已获授权的客户端模型名，从 [Chat](chat-completions.md#最小请求)或 [Responses](responses.md#最小请求)的最小请求开始。模型列表中出现某个名字，表示该 Key 能使用至少一条路由；Chat、Responses HTTP、Responses WebSocket、搜索和两种 Images 操作要分别配置。网关不在这些格式间转换请求。

## 请求兼容策略

JSON 顶层须为对象；`model` 须为非空字符串且最多 300 字符，提供 `stream` 时须为布尔值。网关按[公共字段与 Header 策略](request-allowlists.md)过滤请求，只在需要删字段、改模型或应用变换时改写请求体。目标上游决定工具、reasoning 等嵌套参数是否可用。Codex 的[插件出站策略](codex-oauth-connect.md)可进一步限制字段。

## 响应兼容策略

网关默认保留上游状态码与业务响应。上游 HTTP content coding 最多解码四层已知编码；下游 SSE 不压缩，其他响应按客户端能力重新协商，见[转发与重试](../user/data-plane.md#转发响应与重试)。配置事件变换时按完整事件处理；连接器可提供[同格式响应适配](../development/connector-response-adapters.md)。

发送下游响应头或内容后不再切换候选。显式允许的状态码重试可能重复执行并计费；图片、已派发的 Codex 请求、已发出的 WS 消息不自动重放。

## 本地错误

本地错误使用 JSON `error` 对象。成功 Upgrade 后，逻辑错误通过 WS 帧而非新的 HTTP 响应传达。

| HTTP | `code` | 排查方向 |
| --- | --- | --- |
| `400` | `invalid_request` | body、model 或变换无效 |
| `400` | `request_body_field_unsupported` | 公共策略显式标为 `reject` 的顶层字段 |
| `400` | `request_body_field_value_unsupported` | ignore 字段不是可删除的等价值 |
| `400` | `codex_request_body_field_unsupported` / `codex_request_body_field_value_unsupported` | 插件无法表达字段或值 |
| `400` | `image_streaming_unsupported` | Images 请求流式 |
| `400` | `image_edit_json_transform_unsupported` / `standalone_web_search_json_transform_unsupported` | 操作不支持请求 JSON 变换 |
| `400` | `codex_image_edit_*` | 插件图片数量、mask、文本或 MIME 限制 |
| `401` | `invalid_api_key` | Gateway Key 无效 |
| `403` | `permission_denied` | 未授权 |
| `404` | `model_not_found` | 模型不可见或本操作无路由 |
| `404` | `previous_response_not_found` | WS 增量状态丢失，重发完整请求 |
| `413` | `request_too_large` | body 超限 |
| `415` | `image_edit_content_type_unsupported` | 非合法 multipart |
| `426` | `websocket_unavailable` | WS transport 不可用；客户端决定是否 fallback |
| `429` | `rate_limit_exceeded` / `concurrent_limit_exceeded` / `insufficient_quota` | 准入或软额度 |
| `502` | `upstream_unavailable` / `response_transform_failed` | 连接或响应变换失败 |
| `502` | `upstream_content_encoding_unsupported` | 未知或过深 coding |
| `503` | `no_healthy_channel` | 无健康可用候选 |
| `503` | `image_body_spool_unavailable` | 图片 edit 临时存储失败 |
| `504` | `connect_timeout` / `response_header_timeout` | 响应头前超时 |

未命中显式重试策略的上游 HTTP 状态/body 默认保留，不重新包装成本地错误。
配置和诊断步骤见[操作说明](../user/data-plane.md)。
