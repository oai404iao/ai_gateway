# MCP 服务架构与实施提案

> 状态：提案。本文定义尚未实现的 MCP 服务方案；当前行为仍以代码、测试、migration 和
> OpenAPI 契约为准。

## 目标

在现有单二进制 Gateway 中增加可选的远程 MCP 数据面，使只支持 MCP 的客户也能使用已经实现的
standalone web search 和 Images generation/edit 能力。

首期提供两个逻辑 MCP 服务：

| MCP kind | 默认 endpoint 示例 | 工具 |
| --- | --- | --- |
| `web_search` | `POST /mcp/search` | `web.run` |
| `image` | `POST /mcp/image` | `image_gen.imagegen` |

方案必须支持以后增加更多 MCP kind，也允许管理员创建多个同 kind 实例，例如为不同模型、域名
策略或客户场景分别创建 `search-public`、`search-docs` 和 `image-brand`。

## 固化决策

1. **仍是一个 Rust 二进制。** MCP 是进程内可选模块，不增加 sidecar、第二个微服务、Unix
   Socket RPC、动态 `.so` 或 WASM。
2. **一个逻辑 MCP 一个 endpoint。** 使用 `/mcp/{slug}`，不把全部工具强制合并到一个全局
   `/mcp`。
3. **采用 MCP `2026-07-28` 无状态 Streamable HTTP。** 只使用
   `server/discover`、`tools/list` 和 `tools/call`，普通结果返回 JSON；不创建
   `Mcp-Session-Id`，不挂独立 SSE GET。
4. **复用现有 Gateway API Key。** HTTP 使用
   `Authorization: Bearer <gateway-api-key>`；Search 继续要求 Responses `proxy` 权限，
   Images 继续要求 Images `proxy` 权限。
5. **MCP 不是新的上游格式。** 不增加 `ApiFormat::Mcp`。MCP 工具只把参数编译为既有
   `ApiOperation::StandaloneWebSearch`、`ImagesGeneration` 或 `ImagesEdit`。
6. **内部调用转发核心，不回环 HTTP。** MCP handler 不请求本机 `/v1/*` URL；它调用抽取后的
   认证后 Proxy use case，复用准入、选路、Transforms、Connector、超时、健康、日志和计费。
7. **静态内置工具 registry。** 管理员只能创建和配置已编译的 MCP kind，不能上传任意工具代码
   或任意 JSON Schema。
8. **显式状态句柄。** Search 的连续 `open` / `click` / `find` 使用客户端回传的
   `search_session_id`；Images edit 必须显式回传图片，不从服务端会话历史查找。
9. **Images 仍不自动重试。** MCP adapter 不改变 generation/edit 的一次上游尝试边界。
10. **首期同步执行。** Search 与 Images 都用普通 `tools/call` 返回；MCP Tasks 扩展作为后续
    阶段，不在首期引入任务表或 worker。

MCP 外部语义与最近核对日期见
[Model Context Protocol 2026-07-28](../reference/mcp-2026-07-28.md)。

## 非目标

首期不实现：

- MCP stdio transport；
- prompts、resources、sampling、elicitation、subscriptions 或 Tasks；
- MCP OAuth authorization server；
- 用户上传自定义工具实现；
- MCP 到 Chat Completions/Responses 的任意通用反射代理；
- 服务端保存对话、Search Session 或最近图片；
- Images streaming；
- 把 MCP 图片大 body 限制并入全局 `proxy_body_bytes`。

## 参考实现边界

工具设计和执行语义参考 `openai/codex` 的以下代码，核对快照为
`757c151a0e920c6238801866a3d13e010dfeddb8`（2026-08-05）：

- `codex-rs/ext/web-search/src/tool.rs`
- `codex-rs/ext/web-search/src/schema.rs`
- `codex-rs/ext/web-search/src/history.rs`
- `codex-rs/ext/web-search/web_run_description.md`
- `codex-rs/ext/image-generation/src/tool.rs`
- `codex-rs/ext/image-generation/src/backend.rs`
- `codex-rs/ext/image-generation/imagegen_description.md`
- `codex-rs/codex-api/src/search.rs`
- `codex-rs/codex-api/src/images.rs`

参考的是工具名称、命令形状、最多五张 edit 输入、Images 默认值和结果处理；Codex 中依赖本地线程
历史或本地文件系统的部分必须改成显式无状态参数，不能原样复制。

## 运行拓扑

```text
MCP client
  -> public listener
  -> POST /mcp/{slug}
  -> Origin / Target-URI / MCP metadata validation
  -> Gateway API-key authentication
  -> CompiledMcpRegistry lookup
  -> server/discover | tools/list | tools/call
  -> built-in MCP tool adapter
  -> authenticated Proxy execution core
  -> existing admission / route / transform / connector / upstream
  -> bounded result collector
  -> MCP CallToolResult
  -> existing durable request logging and settlement
```

MCP 路由属于公共数据面，不进入 Console listener。Console listener 只提供 MCP 配置管理 API 和
管理页面。

## Endpoint 与协议

### URL

每条启用的 MCP 定义使用：

```text
POST /mcp/{slug}
```

`slug` 创建后不可修改，建议限制为：

```text
[a-z0-9][a-z0-9-]{0,62}
```

`GET`、`PUT`、`PATCH`、`DELETE` 返回 `405`。禁用或已删除的 slug 返回 `404`，避免对未授权调用方
泄露配置状态。

### 协议模式

默认只接受 `2026-07-28` 现代无状态请求：

- 要求现代 MCP metadata Header；
- 支持 `server/discover`；
- 不接受 `Mcp-Session-Id`；
- 不提供独立 SSE GET；
- 每个请求创建轻量 handler，请求结束即释放。

可以预留默认关闭的 `allow_legacy_2025_11_25` 兼容开关。即使开启，也必须保持
`legacy_session_mode = false`，不能恢复服务端 Session。

### SDK

建议增加可选依赖：

```toml
[features]
mcp-server = ["dep:rmcp"]

[dependencies]
rmcp = { version = "3.1", default-features = false, optional = true, features = [
  "server",
  "transport-streamable-http-server",
] }
```

`rmcp` 负责协议模型、发现、Header 校验和 Axum transport。业务鉴权、MCP registry、工具策略、
内部 Proxy 调用和日志仍由本项目实现，不把这些职责交给 SDK。

### 公开 URL 校验

MCP `MCP-Target-URI` 必须与实际 endpoint 一致。由于生产通常位于 TLS 反向代理之后，配置中需要
显式公开 URL：

```toml
[mcp]
enabled = false
public_base_url = "https://api.example.com"
allowed_origins = []
allow_legacy_2025_11_25 = false
```

有效 target 为 `${public_base_url}/mcp/{slug}`。默认不信任 `X-Forwarded-Host`、
`X-Forwarded-Proto` 或其他客户端可伪造的 forwarding Header。没有 `Origin` 的非浏览器客户端
可以访问；存在 `Origin` 时必须匹配 `allowed_origins`。

## “无状态”的具体含义

首期不保存 MCP Session，也不把下列内容放入进程内跨请求 Map：

- client initialization state；
- Search ref-id Session；
- 最近图片；
- tool call continuation；
- 订阅或 SSE resume cursor。

跨请求需要的值由客户端显式回传：

```text
Search:
  tools/call -> structuredContent.search_session_id
  next tools/call -> arguments.search_session_id

Images:
  tools/call -> ImageContent
  edit tools/call -> arguments.referenced_image_urls[]
```

任一请求都可以被负载均衡到另一实例。`search_session_id` 不查数据库；Gateway 使用 API Key ID
和该值确定性派生 provider `id`，使不同 API Key 即使提交相同值也不会共享 Search 上下文。

## 鉴权与授权

### API Key

所有 MCP 方法，包括 `server/discover` 和 `tools/list`，都要求：

```http
Authorization: Bearer <gateway-api-key>
```

API Key 只在 MCP HTTP 边界解析一次，随后把 `Arc<CompiledApiKey>` 作为 principal 传给内部
Proxy use case；不得把原始 secret 保存到 handler、日志或内部请求 Header。

首期 API Key 模式是 Gateway 专用 MCP 鉴权 profile，不宣称实现完整 MCP OAuth 自动发现。
未来可以在相同 endpoint 前增加标准 OAuth resource server，并把 OAuth principal 映射为现有
Gateway 用户/授权范围，而不改变工具 Schema。

### 权限映射

| 工具操作 | 必需 Gateway 权限 |
| --- | --- |
| `web.run` | `open_ai_responses` + `proxy` |
| `image_gen.imagegen` generation | `open_ai_images` + `proxy` |
| `image_gen.imagegen` edit | `open_ai_images` + `proxy` |

不新增 `ApiFormat::Mcp`。首期也不要求新增 `mcp.call` 权限，因为 MCP 只是现有数据面能力的另一种
协议表达；API Key 仍只能访问它已经允许的格式、模型、Channel Group 和 Channel。

`tools/list` 应按当前 API Key 过滤：如果固定模型对该 Key 不可达，则不返回该工具；直接调用隐藏
工具时返回不含路由细节的授权错误。

## 多 MCP registry

### 持久化模型

建议新增 `mcp_server_kind`：

```text
web_search
image
```

以及 `mcp_servers`：

| 字段 | 说明 |
| --- | --- |
| `id` | UUID 主键 |
| `slug` | 唯一、不可修改的 endpoint 段 |
| `kind` | 静态实现 kind，创建后不可修改 |
| `name` | Console 展示名称 |
| `description` | `server/discover` 与 Console 描述 |
| `enabled` | 单服务开关 |
| `model_rule_id` | 固定客户端模型规则；首期两个 kind 都必填 |
| `settings_version` | kind-specific JSON schema 版本 |
| `settings` | 经后端 typed validation 的 JSONB |
| `created_at` / `updated_at` | 乐观并发版本 |
| `deleted_at` | 软删除 tombstone，保留日志引用 |

不要允许管理员直接编辑工具名称、任意 Schema 或执行入口。`kind` 在代码中的 registry 决定：

- server metadata；
- 工具列表；
- input/output Schema；
- settings 的 typed schema；
- 支持的 `ApiOperation`；
- 参数到 Gateway 请求的编译逻辑；
- 结果到 MCP content 的映射。

允许同一个 kind 存在多条记录。这样新增第三个 MCP 时只需要增加新的静态
`McpServerKind`、typed config、migration/OpenAPI/UI 和测试，不需要改变 transport。

### 运行时快照

控制面编译结果增加：

```rust
CompiledMcpRegistry {
    by_slug: HashMap<Arc<str>, Arc<CompiledMcpServer>>,
}

enum CompiledMcpServer {
    WebSearch(CompiledWebSearchMcp),
    Image(CompiledImageMcp),
}
```

它与模型规则、Channel 和 API Key 一起进入不可变运行时快照。MCP 数据面每个请求只查一次
`ArcSwap` 快照，不查询 PostgreSQL。

编译时必须验证：

- slug 唯一且合法；
- `model_rule_id` 存在并启用；
- kind 与模型路由格式一致；
- `web_search` 模型存在 `OpenAiResponses` rule；
- `image` 模型存在 `OpenAiImages` rule；
- settings version 和字段合法；
- Search domain policy 不冲突；
- Images 大小限制不超过文件级运行配置；
- 启用的 MCP 至少能编译出一个全局候选路由。

API Key 的实际可达性仍在请求时按其 authorization bitmap 判断。

## Search MCP

### 工具

```text
web.run
```

工具总体沿用 Codex `web.run`：

- `search_query`
- `image_query`
- `open`
- `click`
- `find`
- `screenshot`
- `finance`
- `weather`
- `sports`
- `time`
- `response_length`

另外增加一个 MCP 专用可选字段：

```text
search_session_id
```

Codex 内部可直接使用线程 `session_id`；无状态 MCP 没有隐式线程，因此必须把该值显式化。

### 输入约束

- `additionalProperties = false`；
- 至少出现一种 command；
- `search_query` 最多 4 项；
- `image_query` 最多 2 项；
- query、domain、URL、pattern、location 和 ticker 都有长度上限；
- `response_length` 只允许 `short`、`medium`、`long`；
- 使用 Search ref id 的 `open`、`click`、`find` 或 `screenshot` 必须带前一次返回的
  `search_session_id`；
- 直接打开完整 `https://` URL 可以不带 Search Session；
- 管理员可按 MCP 实例关闭 command family；关闭后该 property 不出现在 `inputSchema`。

### 请求编译

工具参数编译为现有 standalone search body：

```json
{
  "id": "<derived-provider-search-id>",
  "model": "<configured-client-model>",
  "input": "<deterministic command summary>",
  "commands": {},
  "settings": {},
  "max_output_tokens": 3000
}
```

规则：

1. 缺少 `search_session_id` 时生成随机 UUID，并在结果中返回；
2. provider `id` 由 domain separation、API Key ID 和 `search_session_id` 确定性派生；
3. `response_length` 映射为管理员配置的 token 上限，且不允许调用方突破实例上限；
4. `settings.allowed_callers = ["direct"]`；
5. `external_web_access`、location、context size、允许/阻止域名由实例 settings 生成；
6. 调用方 query domains 与管理员 allowlist 取交集，并继续应用 blocklist；
7. 不接受调用方直接提交上游 `model`、`id`、`settings` 或 `max_output_tokens`；
8. 最终请求继续经过现有 standalone search client policy、模型别名、Header Transform、
   Connector body/Header policy 和 operation capability 选路。

### 结果

上游 JSON 解析为：

```json
{
  "content": [
    { "type": "text", "text": "<search output>" }
  ],
  "structuredContent": {
    "output": "<search output>",
    "results": [],
    "search_session_id": "<echo-or-generated-id>"
  }
}
```

`results` 保持 opaque JSON，不在 MCP adapter 解释或重写。`encrypted_output` 不返回给客户端；
Codex 当前工具实现也不把它交给模型。

Search 工具声明 read-only/open-world 提示，但这些 annotation 只用于客户端 UX，不能替代服务端
权限和域名策略。

## Images MCP

### 工具

```text
image_gen.imagegen
```

保持 Codex 的单工具语义：

- 未提供参考图片时执行 generation；
- 提供参考图片时执行 edit；
- 最多五张 edit 图片；
- 只返回第一张生成结果；
- 固定 `n = 1`；
- `background`、`quality`、`size` 首期由实例设置决定，默认均为 `auto`；
- 输出格式固定为 PNG/base64；
- 模型由 MCP 实例固定，调用方不能任意指定。

### 无状态输入

Codex 的 `referenced_image_paths` 和 `num_last_images_to_include` 依赖本地文件系统与对话历史，不能
直接用于远程无状态 MCP。首期工具参数改为：

```json
{
  "prompt": "add a red hat",
  "referenced_image_urls": [
    "data:image/png;base64,..."
  ]
}
```

规则：

- `prompt` 必填并限制长度；
- `referenced_image_urls` 可省略，最多五项；
- 首期只接受 `data:image/png`、`data:image/jpeg` 和 `data:image/webp`；
- 不接受 `file:`、本机路径、任意远程 URL 或 SVG，避免把 SSRF 和文件系统访问引入首期；
- 每张图片、解码后总大小和整个 MCP JSON envelope 都有独立硬上限；
- 不提供 `num_last_images_to_include`；客户端若要编辑上一张结果，必须把返回图片显式传回。

首期 inline edit 应采用保守 MCP body 上限。它不能提高普通
`request_limits.proxy_body_bytes`，也不能改变公共 multipart edit 的
`image_edit_*` 限制。若客户需要超过 inline 上限的大图片，后续应增加受控 artifact upload/URI，
而不是无限提高 JSON 内存上限。

### 请求编译

generation 生成规范化 JSON：

```json
{
  "model": "<configured-client-model>",
  "prompt": "...",
  "n": 1,
  "background": "auto",
  "quality": "auto",
  "size": "auto",
  "output_format": "png",
  "response_format": "b64_json",
  "stream": false
}
```

edit 对每个 data URL 做 MIME、base64 和大小校验，构造
`PreparedRequestBody::ImageEdit` 可消费的 multipart 等价输入，再进入既有 Images edit policy
和 Connector adapter。不得绕过：

- multipart 字段 allowlist；
- 单文件、总 body 和图片数量限制；
- Codex 最多五图且无 mask 的 provider 约束；
- request JSON Transform fail-closed；
- Images 无自动重试规则。

### 结果

成功结果返回一个 MCP `ImageContent`：

```json
{
  "content": [
    {
      "type": "image",
      "data": "<base64>",
      "mimeType": "image/png",
      "_meta": {
        "codex/imageDetail": "original"
      }
    }
  ],
  "structuredContent": {
    "status": "completed",
    "mime_type": "image/png"
  }
}
```

`structuredContent` 不重复 base64。Gateway 不保存文件，也不返回服务端本地路径。

MCP adapter 必须设置独立的结果大小上限。因为 MCP `ImageContent` 最终需要一个完整 base64
字符串，首期允许在严格上限内做有界聚合；不得把该行为扩散到普通 `/v1/images/*` streaming
转发路径。

## 内部 Proxy 重构

当前 `ProxyService::proxy` 同时负责：

- 从 HTTP Header 认证；
- 读取/捕获 body；
- 请求 policy；
- 准入；
- 路由与转发；
- 结果映射和日志。

MCP 不应构造本机 HTTP 请求再经过 TCP 回环，也不应复制一套选路和 Connector 逻辑。实现前先把
核心拆成：

```text
HTTP OpenAI adapter
  -> authenticate bearer
  -> capture public request body
  -> execute_authenticated(...)

MCP adapter
  -> authenticate bearer
  -> compile tool arguments
  -> execute_authenticated(...)

execute_authenticated
  -> admission
  -> request policy
  -> route selection
  -> transforms
  -> connector preparation
  -> upstream transport
  -> logging / usage / billing
```

建议引入：

```rust
struct AuthenticatedClientRequest {
    principal: Arc<CompiledApiKey>,
    operation: ApiOperation,
    entrypoint: RequestEntrypoint,
    headers: HeaderMap,
    body: PreparedRequestBody,
    metadata: RequestEntrypointMetadata,
}

enum RequestEntrypoint {
    OpenAiHttp,
    Mcp,
    ScheduledTest,
}
```

MCP Header、`Origin`、`MCP-*` 和客户端 API Key 不进入内部上游 Header 集合。MCP tool call 只执行
一次 admission、一次路由和一条终态请求日志。

MCP 结果 adapter 可以收集 `execute_authenticated` 返回的已解码、已变换下游 body，但必须使用
operation-specific 上限：

- Search：小型 JSON 上限；
- Images：单图 base64 上限；
- 超限时终止 body、记录失败并返回工具错误；
- body 读取失败后不重试 Images。

## 错误语义

分层处理错误：

| 错误 | 返回方式 |
| --- | --- |
| 缺失/无效 API Key | HTTP `401` + `WWW-Authenticate: Bearer` |
| Origin / Target URI / MCP metadata 无效 | HTTP `400` 或 `403` |
| endpoint slug 不存在、禁用或删除 | HTTP `404` |
| 不支持的 MCP 方法或协议结构 | JSON-RPC error |
| 未知工具 | JSON-RPC method/invalid params error |
| 工具参数、工具授权、模型不可达、无健康渠道、上游错误 | `CallToolResult { isError: true }` |
| RPM、并发或额度拒绝 | `CallToolResult { isError: true }`，在安全 `_meta` 中返回 retry hint |

工具错误只返回安全的 Gateway error code、简短消息和可重试提示，不返回上游凭据、Header、图片
数据、完整请求参数或未清理上游正文。

Images 无 exactly-once 保证。Gateway 本身不自动重试，但客户端在网络不确定时重发
`tools/call` 仍可能产生第二次生成和费用；文档与工具描述必须明确这一点。

## 配置

文件级配置只保存安装/网络/大小等进程参数：

```toml
[mcp]
enabled = false
public_base_url = "https://api.example.com"
allowed_origins = []
allow_legacy_2025_11_25 = false
request_body_bytes = 4_194_304
image_request_body_bytes = 33_554_432
search_result_bytes = 4_194_304
image_result_bytes = 33_554_432
```

具体 MCP 实例、模型和工具策略保存在 PostgreSQL，并通过控制面快照热更新。

配置同步要求：

- `src/runtime_config/mod.rs`
- `config.example.toml`
- `deploy/compose/config.example.toml`
- `docs/user/`
- Docker feature/build 参数

设置 `[mcp].enabled = true` 但二进制没有编译 `mcp-server` feature 时，启动必须返回明确
`ConfigError`，与 embedded Console UI 的 feature gate 一致。

## Console 管理

权威契约仍从 `docs/openapi/console-v1.yaml` 开始。建议新增管理员接口：

```text
GET    /console/v1/mcp-servers
POST   /console/v1/mcp-servers
GET    /console/v1/mcp-servers/{id}
PUT    /console/v1/mcp-servers/{id}
DELETE /console/v1/mcp-servers/{id}
POST   /console/v1/mcp-servers/{id}/validate
```

要求：

- GET detail 返回 `ETag`；
- PUT/DELETE 使用 `If-Match`；
- create/update/delete 在 serializable transaction 中校验完整候选快照、写 audit、提交后发布；
- `validate` 只做编译和路由可用性检查，不发起付费上游请求；
- 删除使用 tombstone，slug 不复用；
- Console 不接收或展示额外 MCP secret，因为客户端复用现有 API Key；
- 页面位于 `/admin/mcp-servers`，显示 endpoint、kind、模型、启用状态和配置错误；
- 用户 API Key 页面可显示可复制 endpoint，但不改变“一次性显示 Key secret”的规则。

## 日志、计费与负载

底层请求仍按真实 operation 记录：

- Search：`standalone_web_search`
- generation：`images_generation`
- edit：`images_edit`

建议扩展请求日志：

```text
request_source = mcp
mcp_server_id
mcp_server_slug
mcp_tool_name
```

这样既保留现有 format/operation 计费，又能按 MCP endpoint 和工具筛选。日志不得保存：

- MCP arguments、Search query、URL 或 pattern；
- data URL/base64；
- MCP API Key；
- upstream response image。

`GET /console/v1/system/load` 可增加当前实例 MCP 指标：

- active tool calls；
- active Search calls；
- active Images calls；
- bounded result collector bytes；
- rejected body/result limit count；
- protocol/auth/origin rejection totals。

这些是实例级运行指标，不是集群聚合。

## 安全边界

1. **同源/目标校验。** 验证 `Origin` 和完整 `MCP-Target-URI`，不默认信任 forwarded Header。
2. **最小权限。** MCP 继续使用 API Key format、route、group/channel 位图和现有额度。
3. **无 Header 穿透。** `MCP-*`、Origin、Host 和客户端 Authorization 永不进入上游请求。
4. **无动态代码。** MCP kind 和工具实现静态链接；数据库只保存 typed settings。
5. **图片保密。** base64、解码字节和 multipart 值不得进入 tracing、请求日志、audit 或错误。
6. **独立限制。** Search JSON、MCP envelope、inline edit 和 MCP image result 分别限流/限大小。
7. **无 SSRF 首期范围。** Images edit 只接受受限 data URL，不抓取远程 URL。
8. **Search 域策略。** 调用方 domain filter 不能扩大管理员 allowlist。
9. **取消传播。** MCP HTTP 客户端断开必须丢弃上游 body 并取消当前调用。
10. **关闭排空。** 停止接受新 MCP 请求；现有普通调用与 HTTP forwarding 一起受全局 grace
    period 约束。

## 代码组织建议

```text
src/
  mcp/
    mod.rs                 # feature gate、registry、通用类型
    transport.rs           # rmcp/Axum、Header/Origin/target 校验
    auth.rs                # Gateway API-key principal
    error.rs               # HTTP、JSON-RPC、CallToolResult 映射
    search.rs              # web.run schema、编译、结果映射
    image.rs               # imagegen schema、data URL 校验、结果映射
  application/
    proxy.rs               # 抽取 execute_authenticated
    request_body.rs        # MCP edit 到 replayable multipart 的安全 builder
  domain/
    mcp.rs                 # persisted/compiled kind 与 typed settings
  persistence/
    mcp.rs                 # Console CRUD 与 tombstone
  http/
    mod.rs                 # feature-gated /mcp/{slug}
    console.rs             # MCP 管理 API
```

如果 `rmcp` transport 与现有 Axum/Hyper 版本出现依赖冲突，应先在隔离 PR 中解决并跑完整 Rust
workspace gate；不要为绕过冲突复制一套不完整 JSON-RPC/MCP parser。

## 分阶段实施

### PR 1：Transport、registry 与 Search

- Cargo/runtime feature gate；
- `rmcp` Streamable HTTP；
- `/mcp/{slug}`；
- `mcp_servers` migration、Console OpenAPI/CRUD 和 snapshot；
- API Key auth；
- `web.run`；
- request log MCP metadata；
- protocol、auth、Search 和多实例无状态测试。

### PR 2：Images generation

- `image_gen.imagegen` generation；
- 固定单图 PNG/base64；
- MCP `ImageContent`；
- result size limit；
- 无自动重试和日志脱敏测试。

### PR 3：Images edit

- 最多五个受限 data URL；
- 独立 MCP envelope/body limits；
- 安全 decode 和 replayable multipart builder；
- 普通/Codex edit deterministic integration tests；
- 取消、Drop、临时文件和无泄漏测试。

### 后续：Tasks、artifact 与标准 OAuth

只有出现明确客户需求后再分别设计：

- MCP Tasks + PostgreSQL durable task state；
- 大图片 artifact upload / signed resource URI；
- 标准 OAuth 2.1 Protected Resource Metadata；
- 多工具 MCP kind；
- subscriptions 或其他 server capability。

Tasks 或 artifact 可以使用外部持久化，但 transport 仍不得恢复 MCP Session。

## 测试与验收

### 协议

- `server/discover`、`tools/list`、`tools/call`；
- `2026-07-28` 必需 Header；
- target URI、Origin 和 Host 反向代理场景；
- 无 `Mcp-Session-Id`、无 GET SSE；
- modern-only 与可选 legacy stateless 模式；
- 官方 MCP conformance tests；
- 两个 Gateway 实例之间无状态接续。

### 鉴权与控制面

- API Key 缺失、撤销、过期、格式不允许、路由不允许；
- tools/list 按 Key 过滤；
- ETag/`If-Match`、admin-only、audit、tombstone；
- 多个同 kind slug；
- snapshot 热更新不查询数据面数据库。

### Search

- Codex command schema 与批量限制；
- 生成/回传 `search_session_id`；
- ref id 跨请求接续；
- per-key 派生隔离；
- operation capability；
- domain policy 交集；
- opaque `results` 与 text output；
- Search Request JSON Transform 继续 fail closed。

### Images

- generation 与 edit 自动分派；
- 最多五图、MIME/base64/大小限制；
- 普通与 Codex Connector；
- 单图 MCP `ImageContent`；
- 不重复 base64 到 structured content；
- 输出超限、客户端取消、上游 body 失败；
- 无自动重试；
- 日志、tracing、audit 和错误不包含图片。

### 回归门禁

任何实现 PR 至少需要：

```bash
cargo fmt --check
cargo clippy --locked --workspace --all-targets
cargo test --locked --workspace
```

涉及 Console 契约和 UI 时还需 OpenAPI 生成检查、Console typecheck/lint/test/build。涉及 Search 或
Images forwarding path 时，除 deterministic integration tests 外，完成前还必须按项目规则获得
明确授权并执行付费 real-upstream smoke；未获授权时只能报告为未完成或保持 draft。

## 需要在实现前锁定的产品参数

下列参数应在 PR 1 开始前由维护者确认，但不改变总体架构：

1. production 镜像是否默认编译 `mcp-server` feature；
2. modern-only 是否允许管理员开启旧协议无 Session 兼容；
3. MCP inline image envelope 和结果的默认大小；
4. Search 各 `response_length` 对应的 `max_output_tokens`；
5. 哪些 `web.run` command family 默认开启；
6. 普通用户 Console 是否显示 MCP endpoint，或只由管理员分发。
