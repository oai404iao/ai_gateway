# 请求字段与 Header 白名单

> 类型：公共请求策略与外部协议参考。
>
> 最近核对：2026-11-20。
>
> 权威来源：[openai-node 请求类型](https://github.com/openai/openai-node/tree/854892a0580980449ce1ed04aa5e3831d3330383)、[Codex 请求构造](https://github.com/openai/codex/blob/a0a30c40e8dbcc44af52e2df64dcce64368d4422/codex-rs/core/src/client.rs)、[DeepSeek Thinking Mode](https://api-docs.deepseek.com/guides/thinking_mode)、[百炼深度思考](https://help.aliyun.com/zh/model-studio/deep-thinking)。

要判断某个字段是否会到达上游，先在[机器契约](request-allowlists.json)查 `client_headers` 或操作的 `client_body`，再看用户组策略、管理员变换和所选连接器。`allow` 表示网关接收这个字段，其嵌套结构和模型支持范围仍取决于插件或上游。

## 查找操作

| 机器契约键 | 公共入口 | 路由操作 |
| --- | --- | --- |
| `chat_completions` | `POST /v1/chat/completions` | `chat_completion` |
| `responses_http` | `POST /v1/responses` | `responses` |
| `responses_websocket` | WebSocket `response.create` | `responses-ws` |
| `standalone_web_search` | `POST /v1/alpha/search` | `web_search` |
| `images_generation` | `POST /v1/images/generations` | `images_generation` |
| `images_edit` | multipart `POST /v1/images/edits` | `images_edit` |

## Body 字段

| 分类 | 行为 |
| --- | --- |
| `allow` | 保留顶层字段，不递归过滤。例如 Chat 的 `thinking` 和 `enable_thinking` 保留原值。 |
| `ignore` | 删除字段或 multipart part；若指定 `accepted_values`，仅在值等价时删除。edit 的 `moderation=auto` 会删除，`moderation=low` 返回 `400 request_body_field_value_unsupported`。 |
| `reject` | 直接返回 `400 request_body_field_unsupported`。 |
| 未列出 | 删除后继续。WebSocket 的字段表只适用于 create；`response.interrupt` 另有严格帧格式。 |

Responses HTTP 和 WS 忽略客户端的 `request_country` 与 `access_programs`。`max_output_tokens` 可通过公共入口，但 Codex Responses 的出站策略会删除它。独立 Search 的 `literature_search_query` 属于 `commands` 内部，不是顶层字段。数据类型见 [Search](https://github.com/openai/codex/blob/a0a30c40e8dbcc44af52e2df64dcce64368d4422/codex-rs/codex-api/src/search.rs) 和 [Images](https://github.com/openai/codex/blob/a0a30c40e8dbcc44af52e2df64dcce64368d4422/codex-rs/codex-api/src/images.rs)。

用户组启用 `filter_fast_mode` 后，网关在白名单处理后删除顶层 `service_tier`，路由、变换、插件、日志及计费均使用过滤后的请求。模型别名只改顶层 `model`。各阶段都无需修改时，保留原始请求字节；改写 body 时会清理对应的摘要、ETag 等失效元数据。

## Header 与出站鉴权

六种操作共用 `client_headers` 的名称和前缀。未知 Header 在入口忽略；显式忽略的转发元数据（如 `Forwarded`、`X-Forwarded-For`）在变换和鉴权后也不能出站。`Authorization` 用于 Gateway 鉴权；`Host`、`Content-Length` 和 `Accept-Encoding` 等由传输层处理。Session affinity 只能从白名单允许的 Header 提取会话标识，例如 `session-id`。

选定渠道后，网关依次应用模板默认值、能力覆盖、插件的协议与隐私处理，清理 hop-by-hop 和显式忽略的 Header，然后准备上游鉴权并做最终检查。模型发现和定时探测也受出站检查约束。[Codex 插件](https://github.com/oai404iao/ai-gateway-connectors)单独维护服务商字段规则；公共字段兼容性不等于向每个上游透传。

只有 HTTP Responses 接受客户端 `Content-Encoding: zstd`。网关先在大小限制内解码，再处理字段；该能力的出站压缩另行配置。multipart 编辑用专用限制并按需流式重建，见[图片请求](openai-images.md)。
