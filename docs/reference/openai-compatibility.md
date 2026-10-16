# OpenAI API 兼容性总览

> 类型：外部参考与网关兼容边界。
>
> 最近核对：2026-08-05。外部来源日期保留；当前网关边界以代码、测试和机器契约为准。
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

网关兼容的是一组明确入口，不是完整 OpenAI 产品面。先检查路径，再检查操作授权、路由和
目标上游能力；仅替换 SDK base URL 不能补齐未实现接口。

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

不支持公共 Images JSON/data URL edit、图片流式、embeddings、audio、files、batches、
assistants、fine-tuning 等其他 OpenAI 路径。

`/v1/alpha/search` 是固定 Codex 源码中的 standalone search 扩展，不是稳定 OpenAI 公共 API。
Codex 自定义 provider 使用 Gateway `/v1` 并声明 `supports_standalone_web_search = true`
时可调用它；网关仍要求独立 `web_search` 能力及操作路由。

## SDK 使用

将 SDK 的 base URL 指向公共 listener 的 `/v1`，使用 Gateway API Key 和管理员配置的
客户端模型名。例如环境变量约定由具体 SDK 决定，不是网关自动加载 `.env`：

```text
base URL: https://gateway.example.test/v1
API key:  <gateway-api-key>
model:    <configured-client-model>
```

先按[Chat](chat-completions.md#最小请求)或[Responses](responses.md#最小请求)发最小调用，
再增加工具、流式和扩展字段。示例调用会使用实际上游并产生费用。
出现问题时按以下顺序定位：

1. SDK 是否调用支持的路径和方法？
2. 客户端字段是否通过[入口白名单](request-allowlists.md)？
3. Key 是否授权目标逻辑渠道，模型是否配置本次操作的可用路由？
4. 所选操作能力、wire model、插件是否支持具体嵌套字段和响应事件？

## 格式隔离

格式用于区分 `open_ai_chat_completions`、`open_ai_responses`、`open_ai_images`；
操作进一步区分 HTTP/WS、search、generation/edit。一个逻辑渠道可显式拥有多个操作能力，
但各操作模型目录和路由独立。渠道组是组织和选择上下文，不替代显式路由候选或授权。

同一客户端模型可配置多个操作，网关不会：

- 将 Chat `messages` 与 Responses `input` 互转；
- 把 Responses 请求转换成 Images；
- 某操作无路由时借用兄弟操作或另一格式；
- 因导入 Codex 凭证自动创建渠道、能力、模型路由或授权。

Search 属于 Responses 格式，但有独立请求 policy、非流式协议、目标路径和日志操作。
Images generation 与 edit 同属 Images，也必须分别配置。

## 请求兼容策略

### 入口验证与保真

客户端通过 `Authorization: Bearer <gateway-api-key>` 鉴权。JSON 必须为对象，
顶层 `model` 为非空且不超过 300 字符的字符串，可选 `stream` 为布尔值。
Images edit 改用合法 multipart；大小和 framing 限制见[Images](openai-images.md)。

所有操作检查顶层字段，未知字段拒绝；允许字段内部不递归验证。字段全集只维护在
[`request-allowlists.json`](request-allowlists.json)，不要从本页摘要反向生成名单。
Chat 的 `thinking`、`enable_thinking` 是入口兼容扩展，不意味着网关解释其值。

用户组 Fast 过滤紧跟 body 白名单，静默删除顶层 `service_tier`。变换、插件、日志和请求
计费倍率只观察过滤后 body，不能重新引入该字段。reasoning/Fast 元数据的宽松提取
不增加嵌套字段拒绝规则。

没有 policy 删除、Fast 过滤、别名、变换或适配时保留原始 JSON 字节；模型别名只改顶层
`model`。body 变化会移除失效的 `Content-MD5`、digest、ETag、Last-Modified 等原始表示
元数据，管理员 Header 变换可显式设置新的有效值。

### Header、编码与特殊操作

- Header 只允许机器契约声明的名称/前缀。未知 Header 在入口忽略；显式忽略的转发/IP/CDN
  元数据还在变换后和最终派发前清理，插件和自定义鉴权不能重新引入。
- 客户端 Authorization、Host、Content-Length、Accept-Encoding、代理鉴权和 hop-by-hop
  Header 不直接转发；上游鉴权与 HTTP 编码由网关准备。
- 仅 HTTP Responses 接受入口 zstd JSON，解码前后均受 JSON 大小限制。
  出站 zstd 仅用于显式配置后的最终 HTTP Responses body，不用于 WS/search/Images。
- 普通 `general` 连接器拼接渠道 base URL、原 API 路径和 query；插件可按操作改目标。
- Search 固定非流式，不执行请求 JSON Transform；Images edit 也不执行请求 JSON Transform。
  选到该类变换返回本地错误，不能静默跳过。
- Images `stream:true` 本地拒绝。multipart edit 可用内存/匿名临时文件回放，不能通过提高
  全局 JSON 上限绕过专用限制。

## 响应兼容策略

### 流式传输与重试

默认保留业务 body 和未被重试策略消费的上游状态，不为 usage、解压或重编码整体缓冲 JSON。
网关向上游独立声明 gzip、deflate、Brotli、Zstandard，流式解码最多四层已知 coding。
未知或过深 coding 在下游响应头前失败，损坏压缩流以 `upstream_body_error` 终止。

SSE 在解码后转发，不启用下游 HTTP 压缩。有 SSE 变换时按事件处理；无变换时不重写事件。
非 SSE 可压缩响应按下游 `Accept-Encoding` 独立重编码；已知小于 1 KiB 保持 identity，
未知长度也不为判断阈值缓冲整个 body。表示改变会清理长度、range、ETag 和 digest。
连接器可显式提供同格式 JSON/SSE 归一化，范围见[响应适配](../development/connector-response-adapters.md)。

普通 Chat、Responses HTTP、search 可在响应头前连接失败/超时后换未尝试的渠道/模型候选；
管理员还可显式允许部分 `400..=599` 状态重试，可能重复工作或费用。默认不重试 HTTP 错误。
Codex 一旦派发、Images 一旦开始上游尝试、WS 一旦发出消息，均不自动复制请求。
发送下游响应头或任何 body 后不再切换候选。

### WebSocket 与 Codex

WS 一次只处理一个 `response.create`。成功且干净的连接才可复用；
`previous_response_id` 无法命中精确池连接时返回 `404 previous_response_not_found`。
连接限制和状态丢失控制错误保留原形状、不执行事件 patch，并废弃连接，不因此禁用渠道。
上游握手晚于下游 Upgrade，不能回填上游握手响应 Header。

Codex 依赖管理员安装并启用的外部插件，没有内置 fallback。宿主负责鉴权、凭证快照、
准入、路由、transport 和财务边界；插件负责 OAuth/后端协议、专有 body/Header、installation
和 workspace 隐私归一化、供应商 usage 语义。插件设置不属于宿主 system settings。

Codex HTTP Responses 要求 SSE，WS 保留连接级增量；search 和 Images 返回非流式 JSON。
公开 multipart edit 通过回放流适配为插件所需形状，不代表提供公共 JSON edit。
详细限制和版本来源见[Codex 接入](codex-oauth-connect.md)，不要从公共白名单推断 Codex 出站字段。

## 本地错误

本地错误使用 `{ "error": { ... } }` JSON envelope，`code` 是网关契约，不是 OpenAI 完整错误表。
WS 升级后的逻辑错误通过错误事件表达；下面 HTTP 状态不能一概当作新的握手响应。

| HTTP | `code` | 排查方向 |
| --- | --- | --- |
| `400` | `invalid_request` | body、model 或变换无效 |
| `400` | `request_body_field_unsupported` | 未知公共顶层字段 |
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
配置和诊断步骤见[操作说明](../user/operations.md)。
