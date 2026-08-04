# 请求字段与 Header 白名单

> 类型：外部兼容契约与网关安全边界。
>
> 状态：当前。
>
> 最近核对：2026-08-04。
>
> 机器可读权威契约：
> [`request-allowlists.json`](request-allowlists.json)。
>
> 权威来源：
> [`openai/openai-node@854892a`](https://github.com/openai/openai-node/tree/854892a0580980449ce1ed04aa5e3831d3330383) 的
> [Chat Completions](https://github.com/openai/openai-node/blob/854892a0580980449ce1ed04aa5e3831d3330383/src/resources/chat/completions/completions.ts)、
> [Responses](https://github.com/openai/openai-node/blob/854892a0580980449ce1ed04aa5e3831d3330383/src/resources/responses/responses.ts) 与
> [Images](https://github.com/openai/openai-node/blob/854892a0580980449ce1ed04aa5e3831d3330383/src/resources/images.ts) 请求类型；
> [`openai/codex@5af8599`](https://github.com/openai/codex/tree/5af85998c24fb3353ddd8164c3ed472057b03cb3) 的
> [Responses wire type](https://github.com/openai/codex/blob/5af85998c24fb3353ddd8164c3ed472057b03cb3/codex-rs/codex-api/src/common.rs) 与
> [Images wire type](https://github.com/openai/codex/blob/5af85998c24fb3353ddd8164c3ed472057b03cb3/codex-rs/codex-api/src/images.rs)。

## 目标

数据面请求使用两层独立白名单：

1. **客户端入口策略**：在路由和 Transform 前约束客户端提供的请求 Header 与顶层 body 字段；
2. **Codex 出口策略**：选中 `connector_kind = codex_oauth` 后，在普通 Transform 之后再次约束
   发往 Codex 后端的 Header 与顶层 body 字段。

普通 `openai_compatible` Connector 不执行第二层 provider 白名单，但仍受客户端入口策略、
hop-by-hop 清理和上游鉴权覆盖约束。

当前只校验 JSON 对象或 multipart 表单的**顶层字段名**。`messages`、`input`、`tools`、
`metadata` 等已允许字段内部的嵌套结构仍由目标上游解释；网关不会递归实现完整 OpenAI schema。

## 动作语义

机器契约为每个字段指定以下动作之一：

| 动作 | 行为 |
| --- | --- |
| `allow` | 保留字段；若没有其他 Transform，JSON 原始字节保持不变。 |
| `ignore` | 删除字段后继续；`accepted_values` 存在时，仅这些等价值可以被删除。 |
| `reject` | 返回客户端 `400`，不联系上游。 |
| 未列出字段 | 客户端或 Codex body 默认 `reject`；Header 默认 `ignore`。 |

`ignore` 只能用于以下情况：

- 纯客户端遥测字段，对 provider 生成语义没有影响；
- 空值、默认值或明确的 no-op；
- 已记录的兼容行为，例如 Codex Responses 忽略 `max_output_tokens`。

会改变生成内容、状态管理、输出格式或成本语义，而 Codex wire type 无法表达的非默认值必须
`reject`，不能静默丢弃。

## 客户端入口策略

### Header

所有公开数据面操作共享 `client_headers` 白名单。精确 Header 名与允许的前缀完整记录在
[`request-allowlists.json`](request-allowlists.json)：

- OpenAI 与 HTTP 表示相关 Header，例如 `authorization`、`content-type`、
  `openai-organization`、`openai-project`、`idempotency-key`；
- Gateway/Codex Session Header，例如 `session-id`、`thread-id`、
  `x-client-request-id`、`x-session-id`；
- W3C trace Header；
- 官方 SDK 使用的 `x-stainless-*` 前缀。

未列出的客户端 Header 被忽略。`Connection` 声明的动态 hop-by-hop 名称会暂时保留到安全清理
阶段，以防 Header Transform 绕过动态保护，但绝不会转发。Session affinity 的
`request_header` 来源必须同时位于该客户端 Header 白名单，否则控制面编译失败。

### Body

`interfaces` 下分别维护：

| 契约键 | 公共接口 |
| --- | --- |
| `chat_completions` | `POST /v1/chat/completions` |
| `responses_http` | HTTP/SSE `POST /v1/responses` |
| `responses_websocket` | WebSocket `response.create` |
| `images_generation` | `POST /v1/images/generations` |
| `images_edit` | multipart `POST /v1/images/edits` |

各接口的 `client_body.allow` 是当前支持的完整顶层字段白名单。未知顶层字段返回
`request_body_field_unsupported`。字段已知但值不能按契约忽略时返回
`request_body_field_value_unsupported`。

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

### Responses HTTP

- 只允许 Codex `ResponsesApiRequest` 声明的字段；
- `metadata`、`user`、`safety_identifier` 与 `max_output_tokens` 被忽略；
- `previous_response_id` 仅允许 `null` 或空字符串后删除，非空值返回错误；
- provider 未支持的状态、采样、prompt template、moderation 和缓存选项仅接受契约列出的
  空值/no-op，其他值返回错误；
- 最终强制 `stream=true`、`store=false`。

### Responses WebSocket

- 只允许 Codex `ResponseCreateWsRequest` 声明的字段，以及客户端事件 `type`；
- Gateway 扩展字段 `generate`、`client_metadata` 明确列入客户端与 Codex 白名单；
- 保留 `previous_response_id`；
- `max_output_tokens`、纯遥测字段按契约忽略，其他 provider 不支持的非默认值返回错误；
- 最终强制 `type=response.create`、`stream=true`、`store=false`。

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
  -> common hop-by-hop cleanup
  -> Codex Header allowlist (Codex only)
  -> connector auth/protocol headers
  -> transport headers
```

客户端策略删除字段、模型别名、JSON Transform 或 Connector policy 改变 body 时，网关会移除原始
`Content-MD5`、digest、ETag 等表示元数据。multipart edit 通过 replayable 重建删除被忽略的
part，不把图片整体读回内存。

## 维护流程

新增或修改请求字段时：

1. 先核对 OpenAI 官方请求类型与本机 `/home/u/dev/research/codex` 的当前 wire type；
2. 首先编辑 `request-allowlists.json`，更新 `verified_at` 和 source commit；
3. 对每个字段明确选择 `allow`、`ignore` 或 `reject`，不得依赖未列出字段的默认动作表达已知
   provider 差异；
4. 更新本说明及相关接口文档；
5. 添加客户端入口、普通 upstream、Codex HTTP/WebSocket/Images 的确定性测试；
6. 运行 Rust、文档和真实上游验证。

Rust 单元测试会拒绝以下契约漂移：

- 缺少五个公共接口之一；
- 未知客户端/Codex body 不再默认拒绝；
- 未知 Header 不再默认忽略；
- 一个字段同时出现在多个动作集合；
- 客户端已知字段在对应 Codex policy 中没有显式动作；
- Header 名、排序、重复项、source commit 或日期格式无效。
