# 无状态 MCP 服务

> 状态：部分实现。当前已实现可选的 Search MCP 与 Images generation MCP；Images edit
> 仍在后续阶段。

## 适用范围

启用 `mcp-server` Cargo feature 后，Gateway 可以在公共 listener 上提供无状态
Streamable HTTP MCP endpoint，使只适配 MCP 的客户端复用现有 API Key、Responses
与 Images 路由、Connector、准入、计费和请求日志。

当前实现：

| kind | endpoint | tool | Gateway 操作 |
| --- | --- | --- | --- |
| `web_search` | `POST /mcp/{slug}` | `web.run` | `StandaloneWebSearch` |
| `image` | `POST /mcp/{slug}` | `image_gen.imagegen` | `ImagesGeneration` |

当前不提供 Images edit MCP、MCP OAuth、stdio transport、服务端 Session、独立 SSE GET、
prompts、resources 或 Tasks。

## 构建与启用

MCP 代码默认不编入二进制。构建时启用：

```bash
cargo build --release --features mcp-server
```

然后在 TOML 中启用进程级 transport：

```toml
[mcp]
enabled = true
public_base_url = "https://api.example.com"
allowed_origins = []
allow_legacy_2025_11_25 = false
request_body_bytes = 4194304
search_result_bytes = 4194304
image_result_bytes = 33554432
```

- `public_base_url` 必须是无路径、无查询参数和无凭据的 HTTP(S) origin。它用于派生
  必须匹配的入站 `Host`，并用于展示完整 endpoint。
- 缺少 `Origin` 的非浏览器客户端可以访问。
- 请求带有 `Origin` 时，必须精确匹配非空的 `allowed_origins`；空列表会拒绝所有带
  `Origin` 的请求，而不是放开浏览器来源。
- `allow_legacy_2025_11_25` 默认关闭。关闭时要求 MCP `2026-07-28` 的每请求 metadata
  和标准 `Mcp-Method` / `Mcp-Name` Header。
- `image_result_bytes` 限制单次 Images generation 收集的上游 JSON/base64，默认 32 MiB，
  硬上限 64 MiB。
- 如果 TOML 启用了 MCP，但二进制未编译 `mcp-server` feature，启动会被拒绝。

MCP 路由只挂载到公共 listener；Console listener 不提供 MCP transport。

## 管理 MCP 实例

管理员通过 Console API 管理实例：

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

### Images generation 实例

```json
{
  "slug": "image",
  "kind": "image",
  "name": "Image generation",
  "description": "Generate one managed PNG image",
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

Images generation MCP 要求 API Key：

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
`arguments.search_session_id` 回传。服务端不保存 Session；Gateway 根据 API Key ID、MCP
server ID、当前模型/策略 scope 和该 UUID 确定性派生上游 Search ID，因此请求可以由任意
Gateway 实例处理，并且同一 API Key 在不同 MCP endpoint 复用相同 UUID 时也不会共享 Search
上下文。管理员更改绑定模型或 Search 策略后，旧 ref id 会自然失效，不能跨策略版本继续使用。

直接打开完整 `https://` URL 不要求已有 Search Session。`http://` URL、未知字段、空 command
集合、超限 command 数量和被实例域名策略完全排除的 query domains 会失败关闭。

## Images generation

Images endpoint 暴露：

```text
image_gen.imagegen
```

当前 generation 阶段只接受：

```json
{
  "prompt": "a moonlit lake in watercolor"
}
```

工具行为：

- `prompt` 必填，最多 32,000 个字符且最多 64 KiB，未知字段失败关闭；
- 模型、background、quality 和 size 由 MCP 实例固定；
- 固定 `n = 1`，省略 `stream` 以保持非流式，并使用与 Codex 内置 generation 相同的最小
  Images 字段集合；
- 继续经过现有 `ImagesGeneration` policy、模型别名、Transforms、Connector、Images 专用超时、
  被动健康、usage、计费和无自动重试边界；
- 只取第一张结果；要求上游直接返回 `b64_json`，并验证标准 base64 与 PNG signature；
- 返回一个 MCP `ImageContent`，`mimeType = image/png`，并设置
  `_meta["codex/imageDetail"] = "original"`；
- `structuredContent` 只包含完成状态和 MIME，不重复 base64；
- Gateway 不保存文件或最近图片。

该工具是非幂等付费操作。Gateway 不会自动重试 Images，但客户端若在网络结果不确定时重新发送
`tools/call`，仍可能生成第二张图片并再次计费。Images edit 尚未实现；当前提交
`referenced_image_urls` 或其他引用字段会作为未知字段拒绝。

## 日志与结果边界

- Search 与 Images generation 工具继续通过现有 Proxy use case，保留模型别名、Channel
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
- 当前 MCP Search 与 Images generation 调用都同步完成，不创建后台 Task。

## 相关文档

- [MCP 服务架构与实施记录](../development/mcp-services.md)
- [MCP 2026-07-28 外部语义](../reference/mcp-2026-07-28.md)
- [运行与接口说明](operations.md)
