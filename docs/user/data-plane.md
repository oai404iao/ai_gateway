# 调用公共数据面

> 状态：当前。

公共数据面使用 Gateway API Key，为 SDK 和程序提供 OpenAI 兼容接口。本页先给出调用方式，再说明请求策略、图片和搜索的限制；首次创建 Key 与路由见[快速开始](quickstart.md)。

## 发起请求

```bash
export GATEWAY_API_KEY='替换为网关 API Key'
curl --fail-with-body http://127.0.0.1:3000/v1/models \
  -H "Authorization: Bearer $GATEWAY_API_KEY"
```

模型列表只反映当前 Key 的授权和可达性，不代表所有操作均可用；请为要调用的操作配置[独立路由](routing.md)。

## 选择接口

- `GET /health`：返回 `204`，无需认证。
- `GET /v1/models`：列出当前 API Key 可达的模型；需要 `proxy` 和 `models.read` 权限。
- `POST /v1/chat/completions`：仅匹配 Chat Completions 路由规则。
- `POST /v1/responses`：仅匹配 Responses 路由规则。
- `POST /v1/alpha/search`：Codex standalone web search 扩展；只匹配独立 `web_search` 能力与同操作规则，固定使用非流式 JSON。
- 带 WebSocket Upgrade 的 `GET /v1/responses`：接受顺序的 Responses
  `response.create` 文本消息，只匹配独立 `responses-ws` 能力与同操作规则，见[WebSocket](websocket.md)。
- `POST /v1/images/generations`：接受带顶层 `model` 的 JSON 请求，只匹配 `images_generation` 能力和同操作规则；当前只支持非流式 generation。
- `POST /v1/images/edits`：接受带 `model`、一个或多个 `image`/`image[]` 和可选
  `mask` 的 `multipart/form-data`，只匹配 `images_edit` 能力和同操作规则。

三个 OpenAI 格式绝不互相回退。客户端 `Authorization` 不会转发给上游；网关清理
hop-by-hop headers 后，按渠道配置最后注入上游认证。

## 请求字段与压缩

所有公开数据面请求先应用客户端入口白名单：未列出的 Header 被忽略，未列出的顶层 JSON 或
multipart 字段返回 `400 request_body_field_unsupported`。当前只检查顶层字段，允许字段内部的
嵌套结构仍由上游解释。完整字段和动作见
[`请求字段与 Header 白名单`](../reference/request-allowlists.md)。

`POST /v1/responses` 支持客户端以 `Content-Encoding: zstd` 发送 JSON body；网关会先解码，
再执行模型解析、入口白名单和路由。压缩后的请求和解压后的 JSON 都不能超过
`request_limits.proxy_body_bytes`。其他 JSON 数据面路由只接受 identity，Images edit 继续只
接受未编码 multipart body。

## 转发、响应与重试

数据面在认证后、读取请求体前执行 RPM、并发与已结算软额度预检查。客户端/Connector policy
均未删除或覆盖字段、且没有模型别名或 JSON 变换时才保留原始请求字节。客户端
`Accept-Encoding` 不直接转发；网关独立向上游声明
`gzip, deflate, br, zstd`，流式解码后执行 usage、错误诊断和 SSE 变换，再按客户端的
`Accept-Encoding` 对可压缩非 SSE 响应流式重编码。已知小于 `1 KiB` 的响应保持 identity；
长度未知的流不会为阈值判断而延迟或缓冲。SSE 下游保持 identity，Range 请求上游也使用
identity；整个过程不缓冲完整响应。未知上游 coding 返回
`502 upstream_content_encoding_unsupported`；读取中才能发现的损坏压缩流会终止响应 body 并
记录 `upstream_body_error`。连接失败、连接超时或等待响应头超时时，可以按系统设置在尚未尝试过
的其他健康渠道/模型候选上故障转移；管理员也可以显式列出允许在转发响应前重试的 4xx/5xx
上游状态码。未配置的 HTTP 状态直接转发；一旦向客户端发送响应头或任何响应字节，绝不重试或
切换候选。

## 生成或编辑图片

Images generation/edit 是例外：请求一旦开始尝试上游，就不会自动切换渠道或重试，即使失败
发生在响应头之前，以避免重复生成和重复计费。`stream: true` 返回
`400 image_streaming_unsupported`，且不会联系上游。generation JSON 与其他数据面请求共享
`request_limits.proxy_body_bytes`；edit 使用独立的总 body、单文件、内存阈值和 spool 目录，
不会提高全局 JSON 内存上限。未配置渠道级响应头超时时，generation/edit 使用系统设置中的
Images 专用响应头超时，而不是 Chat Completions/Responses 的普通响应头超时。

Images 的 `x-codex-image-turn-id` 可由调用方传入：普通渠道透传；Codex 渠道保留有效的
客户端/Transform 值，仅在缺失或不可用时生成随机 UUID。该 Header 只关联图片调用，不用于
鉴权，不会在 Codex Responses/Search 出口透传。鉴权与 Connector 身份仍由网关最终覆盖。
Codex Connect 的 Images / Search 同时像 Responses 一样保留 `session-id`、`thread-id`、
`x-client-request-id`、`x-codex-window-id` 和 turn metadata，仅缺失时补全。安装 ID / 工作区
继续隐私归一化；会话头传递不会启用 Images affinity，也不会改变独立请求的 body 格式。

multipart edit 最多接受 64 个 part、16 张输入图片和一个 mask；普通文本字段最多
单项 `64 KiB`、合计 `1 MiB`；boundary 最多 70 bytes，preamble、单个 part Header block 和
boundary padding 分别最多 `8 KiB`、`16 KiB` 与 `1 KiB`，防止畸形 framing 放大 parser
内存。不需要模型别名时，普通 OpenAI-compatible 渠道收到原始 multipart 字节；需要别名时，
网关流式等价重建并只替换 `model` part。客户端 policy 删除兼容字段时也会通过同一 replayable
路径重建 multipart。edit 不应用请求 JSON Transform；若选中渠道配置了该类规则，返回
`400 image_edit_json_transform_unsupported`。Header 和响应 Header 变换仍照常执行。当前不接受
JSON/data URL 形式的公开客户端 edit 请求。

配置 Images 路由时，使用 `images_generation` 或 `images_edit` 能力及同操作路由规则；
Key 必须获得所属逻辑渠道授权，授权不按 API 格式划分。Images 能力不支持 `test_model`，不会进入定时付费探测；Session
粘性、SSE 变换和 WebSocket 也不适用于该格式。普通 Header 变换、generation 的请求 JSON 变换、目标模型改写、
被动健康、准入、请求日志和结算仍沿用统一数据面基础设施。

## 使用独立搜索

Standalone web search 允许顶层 `id`、`model`、`reasoning`、`input`、`commands`、
`settings` 和 `max_output_tokens`；结果 JSON 中的 `output`、`encrypted_output` 和
`results` 原样流式转发，不解释 `results` DTO。该操作允许模型别名和 Header/响应 Header
Transform，但不应用 Request JSON Transform。开启 Search 能力的渠道若组合出非空 Request JSON
Transform，控制面编译失败。

Search 的 `id`、`input`、commands 和 settings 不借用 Responses body 规则。无需模型别名或
策略删除时保留原始 JSON；网关不会额外加入 `stream`、`store`、`client_metadata` 或伪造
`input`。

## 已知边界

- 支持 Chat Completions、Responses、非流式 JSON Images generation 与非流式 multipart
  Images edit；公开 `/v1/images/edits` 不提供 JSON/data URL edit，也不提供图片流式响应、
  embeddings、audio、files、batches、assistants 或 fine-tuning API。
- 所有余额、额度、模型价格和请求费用统一使用 USD；没有跨实例限流、健康状态或 Session
  粘性协调。Chat Completions 与 Responses 的自动重试覆盖收到响应头前的连接失败、连接超时和
  响应头超时；只有显式列入 `retryable_status_codes` 的 HTTP 错误可在下游响应前重试，SSE
  流中断或流空闲超时不会重试；Images generation/edit 不自动重试。
  系统保留独立计量事实和结算回执，但不提供充值/退款或货币兑换产品。
- 服务本身不终止 TLS；Console 必须部署在正确配置的 HTTPS 反向代理后。

详见[计费与额度](billing.md)、[Codex 接入](codex.md)和[兼容性参考](../reference/openai-compatibility.md)。
