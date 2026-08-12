# 请求字段与 Header 白名单

> 类型：外部兼容契约与网关安全边界。
>
> 状态：当前。
>
> 最近核对：2026-08-12。
>
> 机器可读权威契约：
> [`request-allowlists.json`](request-allowlists.json)。
>
> 权威来源：
> [`openai/openai-node@854892a`](https://github.com/openai/openai-node/tree/854892a0580980449ce1ed04aa5e3831d3330383) 的
> [Chat Completions](https://github.com/openai/openai-node/blob/854892a0580980449ce1ed04aa5e3831d3330383/src/resources/chat/completions/completions.ts)、
> [Responses](https://github.com/openai/openai-node/blob/854892a0580980449ce1ed04aa5e3831d3330383/src/resources/responses/responses.ts) 与
> [Images](https://github.com/openai/openai-node/blob/854892a0580980449ce1ed04aa5e3831d3330383/src/resources/images.ts) 请求类型；
> [`openai/codex@7a0e974`](https://github.com/openai/codex/tree/7a0e974e08c798d1e8d59d407aeb6e24db1313af) 的
> [Responses wire type](https://github.com/openai/codex/blob/7a0e974e08c798d1e8d59d407aeb6e24db1313af/codex-rs/codex-api/src/common.rs)、
> [Responses metadata](https://github.com/openai/codex/blob/7a0e974e08c798d1e8d59d407aeb6e24db1313af/codex-rs/core/src/responses_metadata.rs)、
> [compression selection](https://github.com/openai/codex/blob/7a0e974e08c798d1e8d59d407aeb6e24db1313af/codex-rs/core/src/client.rs)、
> [Responses endpoint](https://github.com/openai/codex/blob/7a0e974e08c798d1e8d59d407aeb6e24db1313af/codex-rs/codex-api/src/endpoint/responses.rs)、
> [Zstandard encoder](https://github.com/openai/codex/blob/7a0e974e08c798d1e8d59d407aeb6e24db1313af/codex-rs/http-client/src/request.rs)、
> [Images wire type](https://github.com/openai/codex/blob/7a0e974e08c798d1e8d59d407aeb6e24db1313af/codex-rs/codex-api/src/images.rs)、
> [Search wire type](https://github.com/openai/codex/blob/7a0e974e08c798d1e8d59d407aeb6e24db1313af/codex-rs/codex-api/src/search.rs) 与
> [Search tool Header](https://github.com/openai/codex/blob/7a0e974e08c798d1e8d59d407aeb6e24db1313af/codex-rs/ext/web-search/src/tool.rs)；
> [DeepSeek Thinking Mode](https://api-docs.deepseek.com/guides/thinking_mode) 与
> [阿里云百炼深度思考](https://help.aliyun.com/zh/model-studio/deep-thinking) 的
> Chat Completions 兼容扩展。

## 目标

数据面请求使用两层独立白名单和一层 Codex 隐私归一化/安全补全：

1. **客户端入口策略**：在路由和 Transform 前约束客户端提供的请求 Header 与顶层 body 字段；
2. **Codex 出口策略**：选中 `connector_kind = codex_oauth` 后，在普通 Transform 之后再次约束
   发往 Codex 后端的 Header 与顶层 body 字段；
3. **Codex 隐私归一化/安全补全**：改写已知的安装与工作区指纹；当 Codex Connect 请求缺少
   可安全推导的身份元数据时补齐，不伪造 beta、subagent、attestation、turn-state、residency、
   sandbox 或 request kind。

普通 `openai_compatible` Connector 不执行第二层 provider 白名单，但仍受客户端入口策略、
hop-by-hop 清理和上游鉴权覆盖约束。

当前白名单只校验 JSON 对象或 multipart 表单的**顶层字段名**。`messages`、`input`、`tools`、
`metadata` 等已允许字段内部的嵌套结构仍由目标上游解释；网关不会递归实现完整 OpenAI schema。
唯一的嵌套例外是机器契约中的 `codex_fingerprint_normalization`：它定位
`client_metadata` 与 `x-codex-turn-metadata` 中的安装 ID、请求身份和 `workspaces`。

## 动作语义

机器契约为每个字段指定以下动作之一：

| 动作 | 行为 |
| --- | --- |
| `allow` | 保留字段；若没有其他 Transform，JSON 原始字节保持不变。 |
| `ignore` | 删除字段后继续；body 的 `accepted_values` 存在时，仅这些等价值可以被删除。 |
| `reject` | 返回客户端 `400`，不联系上游。 |
| 隐私归一化/安全补全 | 替换已知 Codex 安装/工作区值，并补齐契约声明的缺失字段；不改变字段的 allow/ignore/reject 分类。 |
| 未列出字段 | 客户端或 Codex body 默认 `reject`；Header 默认 `ignore`。 |

`ignore` 只能用于以下情况：

- 纯客户端遥测字段，对 provider 生成语义没有影响；
- 空值、默认值或明确的 no-op；
- 已记录的兼容行为，例如 Codex Responses 忽略 `max_output_tokens`。

会改变生成内容、状态管理、输出格式或成本语义，而 Codex wire type 无法表达的非默认值必须
`reject`，不能静默丢弃。

## 客户端入口策略

### Header

所有公开数据面操作共享 `client_headers` 白名单。精确允许/忽略的 Header 名与允许的前缀完整记录在
[`request-allowlists.json`](request-allowlists.json)：

- OpenAI 与 HTTP 表示相关 Header，例如 `authorization`、`content-type`、
  `openai-organization`、`openai-project`、`idempotency-key`；
- Gateway/Codex Session Header，例如 `session-id`、`thread-id`、
  `x-client-request-id`、`x-codex-window-id`、`x-session-id`；
- Codex 请求归因和 Search 上下文 Header：`originator` 与
  `x-codex-turn-metadata`；
- 为兼容 0.9.4 示例配置而保留的 `session_id`、`thread_id`；新配置应使用上面的连字符形式；
- W3C trace Header；
- 官方 SDK 使用的 `x-stainless-*` 前缀。

`client_headers.ignore` 显式列出 `Forwarded`、`Via`、常用 `X-Forwarded-*`、真实客户端 IP
Header 与 Cloudflare 转发 Header。这些名称既在客户端入口删除，也在 Header Transform 后由
共享清理层删除，并在 Connector 鉴权及网关自有 Header 准备完成、交给 transport 前再次检查，
因此配置、内部 Connector 或自定义上游鉴权都不能重新引入。渠道模型发现和 scheduled probe
等会应用 Header Transform 的内部请求使用同一个最终 guard。未列出的客户端 Header 仍默认仅在
入口层忽略；普通 Connector 的 Header Transform 可以添加其他未受保护的自定义 Header。

`Connection` 声明的动态 hop-by-hop 名称会暂时保留到安全清理阶段，以防 Header Transform 绕过
动态保护，但绝不会转发。Session affinity 的
`request_header` 来源必须同时位于该客户端 Header 白名单，否则控制面编译失败。

### Body

`interfaces` 下分别维护：

| 契约键 | 公共接口 |
| --- | --- |
| `chat_completions` | `POST /v1/chat/completions` |
| `responses_http` | HTTP/SSE `POST /v1/responses` |
| `responses_websocket` | WebSocket `response.create` |
| `standalone_web_search` | 非流式 `POST /v1/alpha/search` |
| `images_generation` | `POST /v1/images/generations` |
| `images_edit` | multipart `POST /v1/images/edits` |

各接口的 `client_body.allow` 是当前支持的完整顶层字段白名单。未知顶层字段返回
`request_body_field_unsupported`。字段已知但值不能按契约忽略时返回
`request_body_field_value_unsupported`。

Chat Completions 额外允许第三方 OpenAI-compatible 上游常用的顶层扩展字段 `thinking` 和
`enable_thinking`。网关不解释或校验这两个字段的值，只按普通允许字段保留并转发；具体结构、
开关语义和模型支持范围由选中的上游决定。

Images edit 额外兼容部分通用表单会提交、但当前公开 edit 类型未声明的
`moderation=auto`：该默认值在客户端入口层被删除；`moderation=low` 或其他值返回错误。
`output_format` 是公开 edit 字段，因此入口层保留，并由选中的 provider 决定后续动作。

## Codex 出口策略

每个支持 Codex 的接口在 `codex_oauth` 下维护：

- `headers`：允许从客户端或 Header Transform 进入 Codex 请求的 Header；
- `headers.generated`：Connector、HTTP/WebSocket transport 最终生成的 Header，供审计和维护；
- `body`：发往 Codex 的顶层 body 字段动作；
- `body_overrides`：Connector 强制写入的字段；
- `generated_body_fields`：adapter 生成而不是直接来自客户端的字段。

根级 `codex_fingerprint_normalization` 另行维护以下固定行为：

- `client_metadata["x-codex-installation-id"]` 和 turn metadata 中的
  `installation_id` 被替换为按 Codex 凭证稳定派生的 opaque UUID。同一逻辑凭证的 Responses 与
  Images projection 使用同一值，不同凭证使用不同值；客户端原始 installation ID 不发往上游。
- turn metadata 的 `workspaces` 始终替换为数据库系统设置 `forwarding_policy.codex` 定义的
  单一合成工作区。默认值为
  `workspaces["/workspace"].associated_remote_urls.origin =
  "https://github.com/oai404iao/ai_gateway"`；不会发送客户端路径、workspace 数量、commit 或
  dirty 状态。
- Responses HTTP/WebSocket 在缺少时创建 `client_metadata`，补齐 installation、session、
  thread、turn、window、JSON 字符串形式的 turn metadata，以及顶层 `prompt_cache_key`。已有
  非空身份值保留；installation 与 workspaces 始终使用平台值。
- Responses HTTP/WebSocket 在缺少时补 `x-codex-window-id` 和
  `x-codex-turn-metadata` Header；WebSocket 的合成握手 metadata 不新增 turn ID，使同一
  Session 的上游连接池 key 保持稳定。Standalone web search 缺少 turn metadata Header 时也会
  合成一个。
- 无法解析为 JSON 对象的 `x-codex-turn-metadata` 不会作为 opaque 值继续转发，而是用安全合成
  metadata 替换。其他已有字段与 W3C `traceparent`、`tracestate`、`baggage` 保留。

### Responses HTTP

- 只允许 Codex `ResponsesApiRequest` 声明的字段；
- 保留 Codex 客户端生成的 `client_metadata`；缺失时由 Connector 创建并补齐安全身份字段，
  安装 ID 与工作区按上面的隐私规则强制归一化；
- `metadata`、`user`、`safety_identifier` 与 `max_output_tokens` 被忽略；
- `previous_response_id` 仅允许 `null` 或空字符串后删除，非空值返回错误；
- provider 未支持的状态、采样、prompt template、moderation 和缓存选项仅接受契约列出的
  空值/no-op，其他值返回错误；
- 最终强制 `stream=true`、`store=false`。
- 最终 JSON 使用 Zstandard level 3 编码，并生成
  `Content-Encoding: zstd` 与 `Content-Type: application/json`；该请求编码只用于
  Codex Responses HTTP，不用于 WebSocket、standalone search 或 Images。

### Responses WebSocket

- 只允许 Codex `ResponseCreateWsRequest` 声明的字段，以及客户端事件 `type`；
- Gateway 扩展字段 `generate`、`client_metadata` 明确列入客户端与 Codex 白名单；缺失的
  `client_metadata`、`prompt_cache_key` 与安全身份字段会补齐，安装 ID 和工作区强制归一化；
- 保留 `previous_response_id`；
- `max_output_tokens`、纯遥测字段按契约忽略，其他 provider 不支持的非默认值返回错误；
- 最终强制 `type=response.create`、`stream=true`、`store=false`。

### Standalone web search

- 允许 `id`、`model`、`reasoning`、`input`、`commands`、`settings` 和
  `max_output_tokens`；
- Gateway 只检查顶层字段；Search command、settings 和 result DTO 的嵌套结构由 Codex
  上游解释；
- 客户端 `originator` 和 `User-Agent` 不作为上游身份保留，发送前统一覆盖为 Gateway 的
  `codex_cli_rs` Connector 身份；合法的 `x-codex-turn-metadata` 保留，缺失时安全合成，
  安装 ID 与工作区信息在发送前归一化；
- 固定使用非流式 JSON，不添加 body override；
- 当前不支持 Request JSON Transform；支持该操作的渠道若组合出非空 Request JSON
  Transform，控制面编译会失败。

### Images generation

- 发往 Codex 的字段只保留 `prompt`、`background`、`model`、`n`、`quality`、`size`；
- `output_format=png`、`moderation=auto`、`response_format=b64_json`、`stream=false` 和
  `partial_images=0` 可作为等价值删除；
- JPEG/WebP、降低 moderation、输出压缩、非空 style 等无法表达的语义返回错误。

### Images edit

- multipart 输入图片由 adapter 转为生成字段 `images[].image_url`；
- 文本字段只保留 `prompt`、`background`、`model`、`n`、`quality`、`size`；
- `output_format=png`、`response_format=b64_json`、`stream=false`、
  `partial_images=0` 和遥测 `user` 可删除；
- `mask`、`input_fidelity`、`output_compression` 以及其他不等价值返回错误；
- `moderation=auto` 已在客户端入口层删除，但 Codex 契约仍显式记录其兼容动作，防止调用路径绕过
  第一层时产生不一致。

Codex 出口 Header 从普通 Header Transform 结果中再次过滤。未知 Header 被删除；随后 Connector
才注入 Bearer、可选 account/FedRAMP、Codex 版本、Session 或 image-turn Header。客户端不能通过
同名 Header 覆盖这些最终值。

## 执行顺序

```text
raw request
  -> API Key / framing / body size checks
  -> client Header allowlist
  -> client top-level body allowlist
  -> model routing and alias
  -> configured JSON/Header Transform
  -> Codex top-level body allowlist (Codex only)
  -> Codex body privacy normalization and safe enrichment (Codex only)
  -> common hop-by-hop and explicit client Header ignore cleanup
  -> Codex Header allowlist (Codex only)
  -> Codex Header privacy normalization and safe enrichment (Codex only)
  -> connector auth/protocol headers
  -> final explicit client Header ignore guard
  -> transport framing headers
```

客户端策略删除字段、模型别名、JSON Transform 或 Connector policy 改变 body 时，网关会移除原始
`Content-MD5`、digest、ETag 等表示元数据。multipart edit 通过 replayable 重建删除被忽略的
part，不把图片整体读回内存。

## 维护流程

新增或修改请求字段时：

1. 先核对 OpenAI 官方请求类型、涉及的第三方官方兼容文档与本机
   `/home/u/dev/research/codex` 的当前 wire type；
2. 首先编辑 `request-allowlists.json`，更新 `verified_at` 和 source commit；
3. 对每个字段明确选择 `allow`、`ignore` 或 `reject`，并在涉及已知安装/工作区指纹时同步
   `codex_fingerprint_normalization`；不得依赖未列出字段的默认动作表达已知 provider 差异；
4. 常见反向代理/CDN 转发 Header 必须放入 `client_headers.ignore`，不得在代理转发模块另建
   一份运行时名单；
5. 更新本说明及相关接口文档；
6. 添加客户端入口、普通 upstream、Codex HTTP/WebSocket/Images 的确定性测试；
7. 运行 Rust、文档和真实上游验证。

Rust 单元测试会拒绝以下契约漂移：

- 缺少六个公共接口之一；
- 未知客户端/Codex body 不再默认拒绝；
- 未知 Header 不再默认忽略；
- Header 的精确 `allow`/`ignore` 动作重叠；
- Codex 生成 Header 与客户端显式 `ignore` 动作冲突；
- 一个字段同时出现在多个动作集合；
- 客户端已知字段在对应 Codex policy 中没有显式动作；
- Codex 安装 ID 不再按凭证作用域归一化，`workspaces` 不再由系统设置提供单一合成投影，或
  Responses/Search 缺失字段不再按契约补齐；
- Header 名、排序、重复项、source commit 或日期格式无效。
