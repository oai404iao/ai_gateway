# OpenAI API 兼容性总览

> 类型：外部参考与项目兼容契约。
>
> 最近核对：2026-08-05。
>
> 权威来源：[OpenAI API Reference](https://developers.openai.com/api/reference/overview)。
>
> 相关字段来源：[OpenAI Reasoning](https://developers.openai.com/api/docs/guides/reasoning)、
> [OpenAI Priority processing](https://developers.openai.com/api/docs/guides/priority-processing)、
> [OpenAI Images API](https://developers.openai.com/api/reference/resources/images)、
> [`openai/codex@5af85998c24fb3353ddd8164c3ed472057b03cb3` Search endpoint](https://github.com/openai/codex/blob/5af85998c24fb3353ddd8164c3ed472057b03cb3/codex-rs/codex-api/src/endpoint/search.rs)、
> [Search wire types](https://github.com/openai/codex/blob/5af85998c24fb3353ddd8164c3ed472057b03cb3/codex-rs/codex-api/src/search.rs)、
> [DeepSeek Thinking Mode](https://api-docs.deepseek.com/guides/thinking_mode)、
> [阿里云百炼深度思考](https://help.aliyun.com/zh/model-studio/deep-thinking)。

## 支持范围

| 路径 | ai-gateway 行为 |
| --- | --- |
| `GET /health` | 网关自有存活检查，返回 `204`，不属于 OpenAI API。 |
| `GET /v1/models` | 返回当前 API Key 可达的客户端模型名，不返回完整上游目录。 |
| `POST /v1/chat/completions` | 仅按 Chat Completions 格式选路并转发。 |
| `POST /v1/responses` | 仅按 Responses 格式选路并转发。 |
| `POST /v1/alpha/search` | Codex standalone web search 扩展；复用 Responses 路由，但只选择显式支持该操作的渠道。 |
| 带 WebSocket Upgrade 的 `GET /v1/responses` | 顺序转发 Responses WebSocket `response.create`；不做并发多路复用。 |
| `POST /v1/images/generations` | 仅按 Images 格式转发非流式 JSON generation 请求。 |
| `POST /v1/images/edits` | 仅按 Images 格式转发非流式 multipart edit 请求。 |

不支持 Images JSON edit 请求、图片流式响应、embeddings、audio、files、batches、
assistants、fine-tuning 等其他 OpenAI 路径。

`/v1/alpha/search` 不是稳定的 OpenAI 公共 API；其外部契约来自固定 Codex source commit。Codex
自定义 provider 的 base URL 指向 Gateway `/v1` 且声明
`supports_standalone_web_search = true` 时，Codex 会调用该路径。

## 请求兼容策略

- 客户端使用 `Authorization: Bearer <gateway-api-key>`。
- 网关要求 body 是 JSON 对象或 Images edit multipart，顶层 `model` 必须是非空字符串且不超过
  300 个字符；可选 `stream` 必须是布尔值。
- 所有公开接口使用顶层客户端 body 白名单。未列出字段返回
  `request_body_field_unsupported`；字段已知但不能按契约忽略的值返回
  `request_body_field_value_unsupported`。允许字段内部的嵌套结构不递归校验。
- Chat Completions 额外允许第三方兼容扩展 `thinking` 和 `enable_thinking`；网关不解释其值，
  由选中的上游决定是否支持。
- 客户端 Header 使用共享白名单；未知 Header 被删除。官方 SDK 的 `x-stainless-*`、OpenAI
  组织/项目 Header、Gateway Session Header 和 W3C trace Header 显式列入契约。
- 仅为请求日志元数据，网关会宽松识别 Responses 的 `reasoning.effort`、兼容
  Chat Completions 的 `reasoning_effort`，以及 `service_tier = "priority"`；非字符串、
  过长或未知形状不会增加本地拒绝条件。
- 客户端 policy 未删除字段、且没有模型别名或 body 变换时，网关保留原始请求字节。
- 模型别名只改写顶层 `model`。
- Standalone web search 固定为非流式 JSON，允许顶层 `id`、`model`、`reasoning`、`input`、
  `commands`、`settings` 和 `max_output_tokens`。它不应用 Request JSON Transform；渠道必须
  显式声明 `supports_standalone_web_search`。
- Images generation/edit 的 `stream: true` 在本地返回
  `400 image_streaming_unsupported`；当前不会联系上游。
- Images edit 要求带合法 boundary 的 `multipart/form-data`，最多接受 16 张
  `image`/`image[]`、一个 `mask` 和 64 个 part；文本字段单项最多 `64 KiB`、合计最多
  `1 MiB`。总 body、单文件和内存阈值由独立 `image_edit_*` 配置控制；超过内存阈值后使用
  匿名临时文件。为保持 parser 内存有界，boundary 最多 70 bytes，preamble、单个 part Header
  block 和 boundary padding 分别最多 `8 KiB`、`16 KiB` 与 `1 KiB`。
- multipart edit 不应用请求 JSON Transform。模型别名需要变更时，网关流式等价重建 multipart；
  否则原始字节保持不变。
- 普通 `openai_compatible` Connector 会把查询字符串和原 API 路径拼接到渠道 `base_url`；
  provider Connector 可以按操作改写目标路径。
- 客户端 `Authorization`、`Host`、`Content-Length`、`Accept-Encoding`、代理鉴权和
  hop-by-hop headers 不会直接转发；上游鉴权最后注入，HTTP content coding 由网关独立协商。
- 模型别名、请求 JSON Transform 或 provider adapter 改变 body 时，客户端提供的
  `Content-MD5`、`Digest`、`Content-Digest`、`Repr-Digest`、`ETag` 和 `Last-Modified`
  会在 Header Transform 之前移除，避免把失效的完整性/表示元数据带到上游；Header
  Transform 仍可显式设置新的值。

机器可读权威契约为 [`request-allowlists.json`](request-allowlists.json)，动作语义和维护流程见
[`请求字段与 Header 白名单`](request-allowlists.md)。上游新增顶层字段需要先更新该契约；
嵌套字段仍通常不要求网关升级。

## 响应兼容策略

- 网关向上游声明 gzip、deflate、Brotli 和 Zstandard，支持流式解码单层或最多四层已知
  `Content-Encoding`；未知 coding 在发送下游响应头前失败，读取中发现的损坏压缩流以
  `upstream_body_error` 终止。
- 普通 JSON 响应不会为了解压、usage 采集或下游重编码而整体缓冲。
- SSE 在解码后默认按事件字节流转发；只有启用 SSE 变换时才按事件边界解析和重写。下游 SSE
  不启用 HTTP 压缩，以避免事件延迟。
- 已知长度至少 1KiB 的可压缩非 SSE 响应按客户端 `Accept-Encoding` 独立选择 gzip、deflate、
  Brotli、Zstandard 或 identity；长度未知的流保持立即转发并允许压缩。表示变化时移除失效的
  长度、range、ETag 和 digest 元数据。
- 上游 HTTP 错误不会触发自动重试。
- Chat Completions 与 Responses 的自动故障转移仅发生在响应头前的连接失败、建连超时或
  响应头超时。普通 standalone web search 使用相同的 pre-header 故障转移边界；Codex OAuth
  Connector 发送后不重试。Images generation/edit 一旦开始上游尝试就不自动重试或切换渠道。
- 网关生成的本地错误使用 OpenAI 风格的 `{ "error": { ... } }` JSON 结构，但错误代码是本项目契约。

Responses WebSocket 的上游事件以 JSON 文本消息透传；配置的 Responses SSE 事件规则会应用到
同类型 WebSocket 事件。网关按顺序一次处理一个 `response.create`，并让同一下游 Session 优先取回
同一条上游连接，以保留 `previous_response_id` 的连接级缓存；只有成功、无残留的连接才会归还池中。
上游 WebSocket Upgrade 在首条消息之后发生，因此上游握手响应 Header 不会出现在已经完成的下游
Upgrade 响应中。

Codex OAuth managed channel 也走同一 Responses WebSocket 路径，但由 Connector 改写
`/responses` 目标、强制 `stream=true`/`store=false` 并注入订阅凭证与 Codex Header；
`previous_response_id` 仍只在同一条可复用上游连接上有效。Responses HTTP/WebSocket 的
`client_metadata` 会把客户端 installation ID 替换为按凭证稳定的 opaque UUID，并把 turn
metadata 的 `workspaces` 强制替换为系统设置中的单一合成 Git 工作区。缺失的安全身份
metadata 与 `prompt_cache_key` 会补齐；其余已有 metadata 保留。

Codex OAuth standalone web search 使用同一 Responses managed channel、模型规则、API Key 权限
和凭证。Connector 把公共 `/v1/alpha/search` 改写为 managed base URL 下的 `/alpha/search`，
保留 `originator` 与 `x-codex-turn-metadata`，缺失时安全补齐，删除 Responses Session Header，
并按普通 JSON 处理响应。Turn metadata 使用与 Responses 相同的 installation/workspace 归一化；
`results` 中未知 DTO 和字段透明转发；没有 usage 时不估算 token 或费用。

Codex OAuth Images projection 仍使用标准客户端 `/v1/images/generations`，Connector 将上游目标
改为 `/images/generations`，注入共享订阅凭证和 `x-codex-image-turn-id`，并按非流式 JSON
而不是 SSE 处理响应。它不会把 Responses 输入转换成 Images。

同一 projection 也接受客户端 `/v1/images/edits` multipart。Connector 流式读取 replayable
body，把最多五张输入图片编码为 Codex JSON `images[].image_url` data URL，再将目标改为
`/images/edits`。Codex adapter 不接受 mask 或无法等价表达的字段；`moderation=auto` 在客户端
入口 policy 删除，`output_format=png` 在 Codex policy 删除。这些限制不改变普通
OpenAI-compatible edit 的最多 16 张输入边界。

Codex Responses HTTP、Responses WebSocket、standalone web search、Images generation/edit
都在普通 Transform 后执行
独立的 provider body/Header 白名单。已知字段必须显式归类为转发、忽略或报错；未知 Codex body
字段报错，未知 Codex Header 被删除。最终 OAuth/account/Session/image-turn Header 在白名单之后
注入，不能由客户端或 Transform 覆盖。该归一化只属于 `codex_oauth` Connector；普通
OpenAI-compatible channel 不改写嵌套 metadata。

## 格式隔离

模型规则、渠道组和渠道都绑定一个 `api_format`。同一个客户端模型名若需要同时支持多个接口，
必须分别配置 Chat Completions、Responses 和 Images 路由。网关不会：

- 将 `/v1/chat/completions` 转换为 `/v1/responses`；
- 将 Responses 输入转换为 messages；
- 将 Responses 或 Chat Completions 请求转换为 Images generation；
- 在一个格式无可用路由时回退到另一格式。

Standalone web search 是 `ApiOperation`，不是第四种 `ApiFormat`：它复用
`open_ai_responses` 模型规则与授权，但 operation capability、请求白名单、目标路径、协议和日志
保持独立。

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
| `400` | `request_body_field_unsupported` | 客户端提交了当前接口未列入白名单的顶层字段。 |
| `400` | `request_body_field_value_unsupported` | 字段只能按特定默认/no-op 值忽略，但请求使用了其他值。 |
| `400` | `codex_request_body_field_unsupported` | Codex wire type 未声明该字段。 |
| `400` | `codex_request_body_field_value_unsupported` | Codex 无法等价表达该字段的非默认值。 |
| `400` | `image_streaming_unsupported` | Images generation/edit 请求设置了 `stream: true`。 |
| `400` | `image_edit_json_transform_unsupported` | multipart edit 选中了请求 JSON Transform。 |
| `400` | `standalone_web_search_json_transform_unsupported` | standalone web search 选中了请求 JSON Transform。 |
| `400` | `codex_image_edit_*` | Codex edit 的图片数量、必填/重复/无效文本字段或 MIME 不符合 adapter 契约。 |
| `401` | `invalid_api_key` | 缺少或无法认证 Gateway API Key。 |
| `403` | `permission_denied` | API Key 没有当前格式或模型列表权限。 |
| `404` | `model_not_found` | 模型不存在、未授权或当前格式没有路由。 |
| `413` | `request_too_large` | 超过配置的代理 body 限制。 |
| `415` | `image_edit_content_type_unsupported` | edit 不是合法 multipart/form-data。 |
| `429` | `rate_limit_exceeded` / `concurrent_limit_exceeded` / `insufficient_quota` | 进程内准入或软额度拒绝。 |
| `502` | `upstream_unavailable` / `response_transform_failed` | 上游连接或响应变换失败。 |
| `502` | `upstream_content_encoding_unsupported` | 上游返回未知或过深的 HTTP content coding。 |
| `503` | `no_healthy_channel` | 没有可选择的健康渠道。 |
| `503` | `image_body_spool_unavailable` | edit 临时文件系统无法创建、写入或准备回放。 |
| `504` | `connect_timeout` / `response_header_timeout` | 响应头前超时。 |

上游已经返回的 HTTP 状态和 body 默认按上游内容传给客户端，不包装成本地错误。
