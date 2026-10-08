# 请求字段与 Header 白名单

> 类型：外部兼容契约与网关安全边界。
>
> 状态：当前。
>
> 最近核对：2026-09-08。
>
> 机器可读权威契约：
> [`request-allowlists.json`](request-allowlists.json)。
>
> 权威来源：
> [`openai/openai-node@854892a`](https://github.com/openai/openai-node/tree/854892a0580980449ce1ed04aa5e3831d3330383) 的
> [Chat Completions](https://github.com/openai/openai-node/blob/854892a0580980449ce1ed04aa5e3831d3330383/src/resources/chat/completions/completions.ts)、
> [Responses](https://github.com/openai/openai-node/blob/854892a0580980449ce1ed04aa5e3831d3330383/src/resources/responses/responses.ts) 与
> [Images](https://github.com/openai/openai-node/blob/854892a0580980449ce1ed04aa5e3831d3330383/src/resources/images.ts) 请求类型；
> [`openai/codex@fde2156`](https://github.com/openai/codex/tree/fde2156057c38c0227ce94c8514d04c7498df60d) 的
> [Responses client and Header construction](https://github.com/openai/codex/blob/fde2156057c38c0227ce94c8514d04c7498df60d/codex-rs/core/src/client.rs)、
> [default HTTP client identity](https://github.com/openai/codex/blob/fde2156057c38c0227ce94c8514d04c7498df60d/codex-rs/login/src/auth/default_client.rs)、
> [Responses SSE turn state](https://github.com/openai/codex/blob/fde2156057c38c0227ce94c8514d04c7498df60d/codex-rs/codex-api/src/sse/responses.rs)、
> [Responses WebSocket endpoint](https://github.com/openai/codex/blob/fde2156057c38c0227ce94c8514d04c7498df60d/codex-rs/codex-api/src/endpoint/responses_websocket.rs)、
> [Images wire type](https://github.com/openai/codex/blob/fde2156057c38c0227ce94c8514d04c7498df60d/codex-rs/codex-api/src/images.rs) 与
> [Search wire type](https://github.com/openai/codex/blob/fde2156057c38c0227ce94c8514d04c7498df60d/codex-rs/codex-api/src/search.rs)；
> 请求压缩实现还与本地研究 checkout
> `openai/codex@eb9dceba1a2e658142a456c5898836774835616b` 的
> `codex-rs/http-client/src/request.rs` 交叉核对；
> [DeepSeek Thinking Mode](https://api-docs.deepseek.com/guides/thinking_mode) 与
> [阿里云百炼深度思考](https://help.aliyun.com/zh/model-studio/deep-thinking) 的
> Chat Completions 兼容扩展。

独立 Images/Search 另以
[`openai/codex@d648947`](https://github.com/openai/codex/tree/d6489472f3c15e87d2d7763a5fde033545c530f8)
的 [Images Header 构造](https://github.com/openai/codex/blob/d6489472f3c15e87d2d7763a5fde033545c530f8/codex-rs/ext/image-generation/src/backend.rs)、
[Search 请求构造](https://github.com/openai/codex/blob/d6489472f3c15e87d2d7763a5fde033545c530f8/codex-rs/ext/web-search/src/tool.rs)
与 [Search 可选上下文](https://github.com/openai/codex/blob/d6489472f3c15e87d2d7763a5fde033545c530f8/codex-rs/ext/web-search/src/history.rs)
核对，记录为 `sources.codex_standalone_commit`；Responses 的核对版本不因此更新。

## 目标

网关拥有公共客户端 Header、顶层 body 白名单和与服务商无关的出站 Header 安全约束。
选中外部连接器后，渠道特有的出站白名单、协议补全、安装标识和隐私处理全部由插件执行。
Codex 实现及其独立机器契约位于
[ai-gateway-connectors](https://github.com/oai404iao/ai-gateway-connectors) 的
`codex/src/request-allowlists.json`，不再嵌入网关契约。

网关只校验 JSON 对象或 multipart 表单的**顶层字段名**。
`messages`、`input`、`tools`、`metadata` 等允许字段内部的嵌套结构由插件或目标上游解释；
普通 `general` Connector 不执行 Codex 隐私处理。

## 动作语义

机器契约为每个字段指定以下动作之一：

| 动作 | 行为 |
| --- | --- |
| `allow` | 保留字段；若没有其他 Transform，JSON 原始字节保持不变。 |
| `ignore` | 删除字段后继续；body 的 `accepted_values` 存在时，仅这些等价值可以被删除。 |
| `reject` | 返回客户端 `400`，不联系上游。 |
| 未列出字段 | 公共客户端 body 默认 `reject`；Header 默认 `ignore`。 |

`ignore` 只能用于以下情况：

- 纯客户端遥测字段，对 provider 生成语义没有影响；
- 空值、默认值或明确的 no-op；
- 已记录的兼容行为，例如 Codex Responses 忽略 `max_output_tokens`。

插件必须自行拒绝无法等价表达的服务商字段；网关不会替插件维护另一份出站字段规则。

## 客户端入口策略

### Header

所有公开数据面操作共享 `client_headers` 白名单。精确允许/忽略的 Header 名与允许的前缀完整记录在
[`request-allowlists.json`](request-allowlists.json)：

- OpenAI 与 HTTP 表示相关 Header，例如 `authorization`、`content-type`、
  `openai-organization`、`openai-project`、`idempotency-key`；
- Gateway/Codex Session Header，例如 `session-id`、`thread-id`、
  `x-client-request-id`、`x-codex-window-id`、`x-session-id`；
- 独立 Images 调用关联 Header `x-codex-image-turn-id`；
- Codex 请求归因、Search 上下文与 Responses 控制 Header：`originator`、
  `x-codex-turn-metadata`、`x-codex-beta-features`、`x-codex-routing-hint`、
  `x-codex-turn-state`、`x-openai-internal-codex-responses-lite` 与
  `x-responsesapi-include-timing-metrics`；
- 为兼容 0.9.4 示例配置而保留的 `session_id`、`thread_id`；新配置应使用上面的连字符形式；
- W3C trace Header；
- 官方 SDK 使用的 `x-stainless-*` 前缀。

`Content-Encoding` 只在 `POST /v1/responses` 接受 `identity` 或 `zstd`；其他 JSON
接口只接受 identity，multipart Images edit 继续拒绝编码 body。Responses 的 zstd body 会在
顶层字段策略前解码，压缩后和解压后的大小都受 `request_limits.proxy_body_bytes` 约束。

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

客户端 body 契约仍把 `service_tier` 分类为 `allow`。在该契约通过之后，运行时可以按用户组
`filter_fast_mode` 策略执行额外的产品级静默过滤：删除顶层 `service_tier` 后继续请求，不改变
机器契约的字段分类。这样 Connector、请求日志和请求计费倍率都只观察过滤后的请求。

Images edit 额外兼容部分通用表单会提交、但当前公开 edit 类型未声明的
`moderation=auto`：该默认值在客户端入口层被删除；`moderation=low` 或其他值返回错误。
`output_format` 是公开 edit 字段，因此入口层保留，并由选中的 provider 决定后续动作。

## 执行顺序

```text
raw request
  -> API Key / framing / body size checks
  -> client top-level body allowlist
  -> user-group Fast filtering
  -> client Header allowlist
  -> model routing and alias
  -> configured JSON/Header Transform
  -> connector-owned protocol/body/Header policy and privacy processing
  -> host common Header safety and credential-binding checks
  -> transport framing/compression/handshake
```

客户端策略删除字段、模型别名、JSON Transform 或 Connector policy 改变 body 时，网关会移除原始
`Content-MD5`、digest、ETag 等表示元数据。multipart edit 通过 replayable 重建删除被忽略的
part，不把图片整体读回内存。

## 维护流程

新增或修改公共请求字段时：

1. 核对官方请求类型及相关兼容文档；
2. 编辑 `request-allowlists.json`，更新核对日期与来源提交；
3. 明确选择 `allow`、`ignore` 或 `reject`；
4. 反向代理/CDN 转发 Header 必须放入 `client_headers.ignore`，不能另建运行时名单；
5. 更新本说明、公共接口测试并运行 Rust、文档和真实上游验证。

服务商出站字段或隐私规则变更应修改插件仓库的契约与测试，不修改网关公共契约。
插件适配在可配置 Transform 之后执行；网关最后仍拒绝受保护 Header、错误凭证绑定、
被改变的选定模型和重新引入的 Fast 计费字段。

Rust 单元测试校验公共契约的六个接口、未知字段默认拒绝、未知 Header 默认忽略、
字段分类互斥、Header 名合法性、排序、重复项、来源提交及核对日期格式。
