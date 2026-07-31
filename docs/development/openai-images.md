# OpenAI Images 转发设计与分阶段实施

> 状态：部分实现。PR 1 的普通 OpenAI-compatible generation 与 PR 2 的 Codex OAuth
> generation 已实现；edit、大 body 和 streaming 阶段仍是提案，不能替代代码、migration
> 或 OpenAPI 契约。

## 目标

为公共数据面增加 OpenAI Images API，同时复用现有鉴权、格式隔离路由、渠道健康、Transform、
请求日志、计费和上游客户端基础设施。设计必须避免两个风险：

1. 把 Images 当成 Responses 的附属路径，导致权限、路由、超时、健康和日志语义混合；
2. 为 multipart/base64 图片请求直接提高全局内存 body limit。

## 固化决策

### 格式与操作分离

`ApiFormat` 是鉴权和路由维度，新增：

```text
OpenAiImages <-> open_ai_images
```

`ApiOperation` 是公共路径和协议操作维度：

```text
ChatCompletions
Responses
ImagesGeneration
ImagesEdit
```

generation 与 edit 共用 Images 路由格式，但保留独立操作，便于 Connector 路径选择、请求日志、
body 策略和未来的协议差异。请求日志同时保存 `api_format` 与 `api_operation`。

### 一个 Channel 只属于一个格式

模型规则、Channel Group 和 Channel 继续严格绑定单一 `api_format`。同一个 Channel 不同时承载
Responses 与 Images，原因包括：

- 支持模型和模型别名可能不同；
- Images generation/edit 的请求体、超时和重复生成风险不同；
- Responses 有 SSE、WebSocket、Session affinity 和 scheduled probe 语义；
- usage、TTFT/TPS 与健康观测不能假设一致。

需要共享的是凭证，不是 Channel。普通 OpenAI-compatible provider 可以为同一凭证创建独立
Responses 与 Images Channel；后续 Codex 接入也使用同样原则。

### Images 请求不做自动重试

Images generation 或 edit 一旦开始上游尝试，就不自动切换渠道或重试，包括发生在响应头前的
连接错误和超时。客户端可按自身幂等策略重试，但 Gateway 不隐式承担重复生成和重复扣费风险。

### 大 body 使用独立 replay 策略

PR 1 只支持小型 JSON generation，继续使用当前 `proxy_body_bytes`。edit/multipart 阶段必须先
引入类似以下抽象，再增加专用限制：

```text
ReplayableRequestBody
  -> Memory(Bytes)
  -> TempFile { path, length }
```

不能仅把全局代理 body limit 提高到可容纳多张图片。临时文件必须使用受限目录和权限，流式写入，
在取消、错误和正常结束时清理，并且敏感图片内容不得进入日志、错误摘要或 audit。

## PR 1：当前实现

### 公共协议

当前只挂载：

```text
POST /v1/images/generations
```

行为：

- JSON body 必须可解析，顶层 `model` 必须是非空字符串；
- `stream: true` 返回 `400 image_streaming_unsupported`，不联系上游；
- 未启用模型别名或请求 JSON 变换时保留原始请求字节；
- 使用 `open_ai_images` API Key 权限、模型规则、Channel Group 和 Channel；
- 普通 `openai_compatible` Connector 沿用原路径
  `/v1/images/generations`、查询字符串和 Header/鉴权顺序；
- 上游状态、Header 和响应 body 默认逐块透传；
- 若顶层 `usage` 存在，增量提取输入/输出 token，不为 base64 图片缓冲完整响应；
- 请求日志写入 `images_generation` 操作，journal schema 为 v4，并兼容读取 v2/v3 backlog；
- Images 请求禁用自动重试、Session affinity、SSE 变换、WebSocket 和 scheduled probe。

### 控制面与 Console

- PostgreSQL `api_format` enum 新增 `open_ai_images`。
- `request_logs.api_operation` 回填旧数据并用约束保证 format/operation 对应关系。
- migration 的 `BEFORE INSERT` 兼容触发器会为尚未升级的旧 Gateway 写入推导 operation，
  避免滚动升级期间旧进程因新列非空约束失败。
- Console OpenAPI 的 `ApiFormat` 和请求日志视图已扩展；前端生成类型由 spec 重新生成。
- Channel Group、Channel、Model Rule、API Key Policy 与 Transform 编辑器可以选择 Images。
- Images Channel 的 `test_model` 被后端和 UI 拒绝。
- 格式中性的空 Config Template 会分别生成 Chat Completions、Responses 和 Images no-op
  plan；显式 Images 文档仅允许 Header 与请求 JSON 规则，SSE 规则在编译阶段拒绝。

### 明确未实现

- `POST /v1/images/edits`
- multipart/form-data
- Images streaming
- PR 1 本身不含 Codex OAuth Images；当前该能力由下述 PR 2 实现
- Images Session affinity、WebSocket、scheduled probe
- Images 专用 body limit 或临时文件 spool
- 付费真实上游 Images smoke；当前由 deterministic mock integration test 覆盖

## PR 2：当前实现的 Codex OAuth 非流式 generation

Codex OAuth 仍使用 `ConnectorKind::CodexOauth`，没有新增
`codex_images_oauth`。Connector 根据 `ApiOperation` 选择 Responses 或 Images 目标、请求约束、
Header 和成功响应协议；客户端路由格式仍由选中 Channel 的 `ApiFormat` 隔离。

### 凭证池与 Channel 投影

一条 Codex 逻辑凭证现在属于共享 Connector pool，并投影为格式隔离的 managed channels：

```text
connector_pools
  -> open_ai_responses channel_group
  -> open_ai_images channel_group

codex_oauth_credentials
  -> codex_oauth_credential_channels
       (credential_id, api_format, channel_id)
```

为兼容既有 Console URL、日志引用和 Responses affinity，旧
`codex_oauth_credentials.channel_id` 保留为稳定的凭证 ID 和 Responses Channel ID；
`connector_pool_id` 与 projection 表承担新的共享关系。每个 OAuth 账户投影为：

- 一个 `open_ai_responses` managed channel；
- 一个 `open_ai_images` managed channel。

两个 Channel 共享 Token、account/member 身份、refresh generation、quota 与 outbound proxy
来源；label、weight、proxy 和超时初始同步。它们拥有独立的 ID、模型列表、格式能力、被动健康
状态和路由授权。Responses projection 继续使用 models endpoint 返回的 slug 并声明 WebSocket；
Images projection 当前固定声明经核对的 `gpt-image-2`，不声明 WebSocket、scheduled probe 或
状态统计。

### 安全迁移

- `migrations/0036_codex_images_projection.sql` 保留现有 Responses Channel Group、Channel 和
  凭证 ID，避免破坏模型规则、日志引用和 Session affinity。
- migration 为既有 Codex group 建立 Connector pool，回填 projection 关系，并新建停用的
  Images Channel Group 与 Images managed channel。
- 该 schema 会使旧二进制无法编译新的 Codex Images group，因此多实例升级使用协调停机切换，
  不能在 migration 应用后继续把 Console 或数据面流量发往旧版本。
- 新建 Codex Responses group 时，数据库约束和 trigger 会在同一事务创建停用的 Images group；
  新凭证也会在同一事务创建两个 projection。
- 不自动把 `open_ai_images` 添加到任何现有 API Key、Policy 或模型规则。
- 不自动创建可访问的客户端 Images 路由。
- 只有管理员显式启用 Images group，并配置本地 `gpt-image-2` 模型、Images model rule、
  API Key format 和 group/channel 权限后才产生新流量。
- credential 删除会清除共享 Token，并把 Responses 与 Images Channel 都保留为不含敏感信息的
  tombstone。

### Codex attempt

Images generation attempt 需要：

- 将目标改写为 Codex subscription backend 的 `/images/generations`；
- 使用与 Responses projection 相同的 credential snapshot 和预发送 token refresh 边界；
- 最后注入 Bearer、`ChatGPT-Account-ID`、FedRAMP、`originator`、`version`、
  `User-Agent` 和 Gateway 生成的 `x-codex-image-turn-id`；
- 移除客户端 `session-id`、`thread-id` 与 `x-client-request-id`，Images 不借用 Responses
  Session identity；
- 在模型别名和受限 JSON/Header 变换后保留 generation JSON 字节，不强制加入 Responses 的
  `stream`/`store` 字段；
- 把成功响应按非流式 JSON 而不是 SSE 处理，继续使用增量 Images usage collector；
- 发送后不跨 credential 或 Channel 自动重试；
- 对 `401` 继续触发现有 refresh-generation 去重的后台 refresh，但不重放已发送的图片请求。

## PR 3：Images edit 与大 body

增加 `POST /v1/images/edits` 和 `ApiOperation::ImagesEdit` 时需要先完成：

1. 路由级 Content-Type 分派和专用 Images body limits；
2. multipart 流式接收、受限字段/文件数量和总大小；
3. `ReplayableRequestBody::{Memory, TempFile}`；
4. 普通 OpenAI-compatible multipart 原样或等价重建；
5. Codex edit adapter，将受控数量的输入图片转换为其要求的 JSON/data URL 形状；
6. 取消安全、临时文件清理和磁盘容量观测；
7. 无自动重试和无敏感 body 日志的测试。

Codex 当前研究结果中的最多五张编辑输入是 provider-specific 约束，不应成为通用
`OpenAiImages` 格式限制；它只在 Codex attempt 中执行。

## 后续：Streaming

Images streaming 只有在以下问题单独设计并验证后才可启用：

- OpenAI 事件类型、终态和错误语义；
- partial image 大小上限与背压；
- SSE Transform 是否永久禁用或增加专用 typed selector；
- usage、取消、客户端断开和计费终态；
- streamed generation 的重复请求风险。

在此之前，`stream: true` 必须 fail closed。

## 测试与验收

PR 1 的最低覆盖：

- generation 路由、API Key/模型格式隔离和原始 JSON 字节透传；
- 上游认证与响应透传；
- 顶层 Images usage 增量提取，不缓冲 base64 输出；
- `stream: true` 在联系上游前拒绝；
- 至少两个可选 Images Channel 时仍只尝试一次；
- Images SSE Transform 和 scheduled test model 编译/写入拒绝；
- request-log journal v2/v3 兼容、v4 operation、数据库批量投影和 Console API；
- OpenAPI 生成类型、Console 表单和文档门禁。

PR 2 另外覆盖：

- 已应用到 0035 的数据库中，既有 Responses group/channel/credential 原 ID 在 migration 后
  保持不变；
- 新旧凭证都产生 Responses 与 Images projection，Images group 默认停用；
- credential 更新同步 projection 的 label、weight、proxy 和 timeout，重新授权不会把 Responses
  model catalog 写入 Images projection；
- credential 删除清理共享 Token 并 tombstone 两个 projection；
- Codex generation 的路径、认证、image turn Header、原始 JSON、JSON 响应、usage 和请求日志；
- 同一凭证可通过 Responses SSE/WebSocket 与 Images generation 使用。

任何后续 Images 转发改动仍须运行普通 Rust/Console 检查和现有付费真实上游 smoke；在引入专用
Images smoke 前，必须保留 deterministic Codex Images mock integration test。

## 权威实现位置

| 主题 | 来源 |
| --- | --- |
| 格式与操作 | `src/domain/api_format.rs`、`src/domain/api_operation.rs` |
| 公共路径 | `src/http/mod.rs` |
| 代理与无重试边界 | `src/application/proxy.rs` |
| usage | `src/application/usage.rs` |
| Transform | `src/transforms/mod.rs` |
| 运行时编译 | `src/runtime_config/mod.rs` |
| 数据库 | `migrations/0034_open_ai_images_api_format.sql`、`migrations/0035_request_log_api_operation.sql`、`migrations/0036_codex_images_projection.sql` |
| Console 契约 | `docs/openapi/console-v1.yaml` |
| 用户可观察行为 | `docs/user/operations.md` |
| 外部 API 边界 | `docs/reference/openai-images.md` |
