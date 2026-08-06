# MCP 服务

> 状态：当前实现。已提供 Search 与 Images generation/edit MCP；默认使用无状态
> `2026-07-28`，也可启用完整的 `2025-11-25` Session/SSE 兼容以及 Codex
> 旧版模式使用的 `2025-06-18` 协商。

## 适用范围

启用 `mcp-server` Cargo feature 后，Gateway 可以在公共 listener 上提供 Streamable HTTP
MCP endpoint，使只适配 MCP 的客户端复用现有 API Key、Responses 与 Images 路由、Connector、
准入、计费和请求日志。

当前实现：

| kind | endpoint | tool | Gateway 操作 |
| --- | --- | --- | --- |
| `web_search` | `/mcp/{slug}` | `web.run` | `StandaloneWebSearch` |
| `image` | `/mcp/{slug}` | `image_gen.imagegen` | `ImagesGeneration` 或 `ImagesEdit` |

当前不提供 MCP OAuth、stdio transport、prompts、resources 或 Tasks。现代协议不创建 Session；
只有管理员显式开启旧协议兼容时才提供进程内 Session、GET SSE 和 DELETE。

## 构建与启用

MCP 代码默认不编入二进制。构建时启用：

```bash
cargo build --release --features mcp-server
```

`[mcp]` 是数据库系统设置首次缺失时的一次性引导值：

```toml
[mcp]
enabled = true
public_base_url = "https://api.example.com"
allowed_origins = []
allow_legacy_2025_11_25 = false
request_body_bytes = 4194304
image_request_body_bytes = 33554432
search_result_bytes = 4194304
image_result_bytes = 33554432
```

- `public_base_url` 必须是无路径、无查询参数和无凭据的 HTTP(S) origin。它用于派生
  必须匹配的入站 `Host`，并用于展示完整 endpoint。
- 缺少 `Origin` 的非浏览器客户端可以访问。
- 请求带有 `Origin` 时，必须精确匹配非空的 `allowed_origins`；空列表会拒绝所有带
  `Origin` 的请求，而不是放开浏览器来源。
- `allow_legacy_2025_11_25` 默认关闭。关闭时只接受 MCP `2026-07-28` 的每请求 metadata
  和标准 `Mcp-Method` / `Mcp-Name` Header。开启后完整支持 `2025-11-25` 的
  `initialize` / `notifications/initialized`、`Mcp-Session-Id`、请求级 SSE、独立 GET SSE
  和 DELETE Session，同时接受 Codex 旧版 MCP 客户端发送的 `2025-06-18` 初始化与后续
  Session Header。字段名保持不变，以兼容已部署的数据库和配置。
- `request_body_bytes` 限制 Search MCP 的 JSON-RPC envelope，默认 4 MiB。
- `image_request_body_bytes` 独立限制 Image MCP 的 JSON-RPC envelope，使 inline data URL
  edit 不会提高 Search 上限；默认 32 MiB、硬上限 64 MiB。
- `image_result_bytes` 限制单次 Images generation/edit 收集的上游 JSON/base64，默认 32 MiB，
  硬上限 64 MiB。
- 如果数据库中的 MCP transport 已启用，但二进制未编译 `mcp-server` feature，候选配置会被拒绝。

首次启动或升级完成后，到 Console 的“系统 → 系统设置”（`/admin/system`）修改这些字段；保存
后会发布新的运行时快照，无需重启。修改 transport 设置或关闭 MCP 会终止当前进程中的旧协议 Session。旧协议
Session 不写入 PostgreSQL，重启后失效；多实例部署启用旧协议时必须对 `/mcp/*` 使用粘性路由。

MCP 路由只挂载到公共 listener；Console listener 不提供 MCP transport。生产镜像仍必须编译
`mcp-server` feature，数据库开关不能动态加载未编入二进制的代码。

## 管理 MCP 实例

管理员可以在 Console 的“路由 → MCP 服务”中打开：

```text
/admin/mcp-servers
```

该页面提供：

- Search 与 Images MCP 实例列表，包括相对 endpoint、固定工具名、绑定模型规则、API 格式、
  kind-specific 设置和启用状态；
- 创建表单，只显示与 kind 兼容的模型规则；
- Search 的外部网络访问、上下文大小、允许/屏蔽域名和短/中/长输出 Token 上限；
- Images 的 background、quality 和规范 `WIDTHxHEIGHT` 默认值；
- 使用详情响应 `ETag` 的更新与删除；遇到并发修改时重新加载，而不是静默覆盖；
- 删除前明确提示软删除会立即移出运行时 registry，但 `slug` 永久保留、不能复用。

`slug` 与 `kind` 创建后不可修改。页面只显示相对路径 `/mcp/{slug}`；客户端使用的完整公开
origin 由 Console“系统设置”中的 `mcp.public_base_url` 决定。MCP 实例页面不会替代构建时的
`mcp-server` feature 或数据库中的全局 MCP transport 开关。

同一资源也可以通过管理员 Console API 管理：

```text
GET    /console/v1/mcp-servers
POST   /console/v1/mcp-servers
GET    /console/v1/mcp-servers/{id}
PUT    /console/v1/mcp-servers/{id}
DELETE /console/v1/mcp-servers/{id}
```

请求/响应形状以
[`docs/openapi/console-v1.yaml`](../openapi/console-v1.yaml) 为准。

### Search 实例

创建示例：

```json
{
  "slug": "search",
  "kind": "web_search",
  "name": "Web search",
  "description": "Search the public web",
  "model_rule_id": "00000000-0000-0000-0000-000000000000",
  "settings": {
    "external_web_access": "live",
    "search_context_size": "medium",
    "allowed_domains": [],
    "blocked_domains": [],
    "max_output_tokens": {
      "short": 1000,
      "medium": 3000,
      "long": 6000
    }
  },
  "enabled": true
}
```

`model_rule_id` 必须指向启用的 `open_ai_responses` 模型规则，并且候选 Channel 必须支持
standalone web search。`slug` 与 `kind` 创建后不可修改；删除采用软删除并立即从运行时
registry 移除。

最终 MCP URL 为：

```text
https://api.example.com/mcp/search
```

### Images 实例

```json
{
  "slug": "image",
  "kind": "image",
  "name": "Image generation and editing",
  "description": "Generate or edit one managed PNG image",
  "model_rule_id": "00000000-0000-0000-0000-000000000000",
  "settings": {
    "background": "auto",
    "quality": "auto",
    "size": "auto"
  },
  "enabled": true
}
```

`model_rule_id` 必须指向启用的 `open_ai_images` 模型规则。`background` 允许 `auto`、
`opaque`、`transparent`；`quality` 允许 `auto`、`low`、`medium`、`high`；`size` 可以为
`auto` 或每个维度在 64 到 8192 之间的规范 `WIDTHxHEIGHT`。实际值仍须由绑定的上游模型支持。

## 鉴权与授权

每一个 MCP 请求，包括 `server/discover` 和 `tools/list`，都必须携带：

```http
Authorization: Bearer <gateway-api-key>
```

Search MCP 要求 API Key：

- 允许 `open_ai_responses`；
- 具有 `proxy` 权限；
- 能访问 MCP 实例绑定模型规则的至少一个候选路由。

Images MCP 要求 API Key：

- 允许 `open_ai_images`；
- 具有 `proxy` 权限；
- 能访问 MCP 实例绑定模型规则的至少一个候选 Images 路由。

`tools/list` 会按 API Key 路由权限过滤。原始 API Key 不会进入 MCP handler 的业务参数、
Search/Images 请求 body、上游 Header 或请求日志。

## 无状态 Search continuation

`web.run` 的命令形状参考 Codex，包括：

- `search_query`、`image_query`
- `open`、`click`、`find`、`screenshot`
- `finance`、`weather`、`sports`、`time`
- `response_length`

首次调用会在 `structuredContent.search_session_id` 返回一个 UUID。后续使用 Search ref id
执行 `open`、`click`、`find` 或 `screenshot` 时，客户端必须把它作为
`arguments.search_session_id` 回传。服务端不保存 Search 历史或 provider Search Session；
Gateway 根据 API Key ID、MCP
server ID、当前模型/策略 scope 和该 UUID 确定性派生上游 Search ID，因此请求可以由任意
Gateway 实例处理，并且同一 API Key 在不同 MCP endpoint 复用相同 UUID 时也不会共享 Search
上下文。管理员更改绑定模型或 Search 策略后，旧 ref id 会自然失效，不能跨策略版本继续使用。

直接打开完整 `https://` URL 不要求已有 Search Session。`http://` URL、未知字段、空 command
集合、超限 command 数量和被实例域名策略完全排除的 query domains 会失败关闭。

## Images generation 与 edit

Images endpoint 暴露：

```text
image_gen.imagegen
```

省略图片引用时执行 generation：

```json
{
  "prompt": "a moonlit lake in watercolor"
}
```

提供显式图片引用时执行 edit：

```json
{
  "prompt": "add a red hat",
  "referenced_image_urls": [
    "data:image/png;base64,...",
    "data:image/jpeg;base64,..."
  ]
}
```

输入规则：

- `prompt` 必填，最多 32,000 个字符且最多 64 KiB，未知字段失败关闭；
- `referenced_image_urls` 可省略或为空，最多五项；
- edit 只接受标准 base64 的 `data:image/png`、`data:image/jpeg` 和
  `data:image/webp`，并核对声明 MIME 与 PNG/JPEG/WebP signature；
- 不接受 HTTP(S)、`file:`、本机路径、SVG、额外 data URL 参数或 URL-safe/带空白 base64；
- 单张图片解码后最多 16 MiB，全部引用解码后合计最多 24 MiB；整个 Image MCP JSON
  envelope 还受 `image_request_body_bytes` 限制；
- 模型、background、quality 和 size 由 MCP 实例固定；
- 固定 `n = 1`，省略 `stream` 以保持非流式，并使用与 Codex 内置 generation 相同的最小
  Images 字段集合；
- generation 进入现有 `ImagesGeneration` policy；edit 把验证后的图片逐块解码为 replayable
  multipart，再进入现有 `ImagesEdit` policy、模型别名、普通/Codex Connector、Images 专用
  超时、被动健康、usage、计费和无自动重试边界；
- edit 输入超过公共 `request_limits.image_edit_*` 限制时仍会失败；MCP 不提高公共 Images
  edit 的 body/file 上限；
- 只取第一张结果；要求上游直接返回 `b64_json`，并验证标准 base64 与 PNG signature；
- 返回一个 MCP `ImageContent`，`mimeType = image/png`，并设置
  `_meta["codex/imageDetail"] = "original"`；
- `structuredContent` 只包含完成状态和 MIME，不重复 base64；
- Gateway 不保存文件或最近图片；客户端若要继续编辑，必须显式回传前一次结果。

该工具是非幂等付费操作。Gateway 不会自动重试 Images，但客户端若在网络结果不确定时重新发送
`tools/call`，仍可能重复生成/编辑并再次计费。

## 日志与结果边界

- Search 与 Images generation/edit 工具继续通过现有 Proxy use case，保留模型别名、Channel
  选择、被动健康、超时、准入、额度、计费和 Connector policy。
- 请求日志使用 `request_source = "mcp"`，不保存 tool 参数、搜索内容、prompt、图片、
  API Key 或完整结果。
- MCP 上游失败只记录 Gateway 的通用错误 code/summary，不把 provider error message/code 写入
  durable request log 或返回给 MCP caller。
- 即使全局 tracing filter 配置为 `debug`/`trace`，Gateway 也会把 RMCP 中会格式化完整请求或
  结果的内部 target 强制限制到 `info`，避免 Search 参数和结果进入进程日志。
- 上游 `encrypted_output` 不返回给 MCP 客户端。
- 结果 body 按 `search_result_bytes` 有界收集；超过限制会返回 caller-visible tool error。
- Images JSON/base64 按 `image_result_bytes` 有界收集；超过限制、base64 无效或结果不是 PNG
  都返回 caller-visible tool error，不回传 provider body。
- 当前 MCP Search 与 Images generation/edit 调用都同步完成，不创建后台 Task。

## 相关文档

- [MCP 服务架构与实施记录](../development/mcp-services.md)
- [MCP 2026-07-28 外部语义](../reference/mcp-2026-07-28.md)
- [运行与接口说明](operations.md)
