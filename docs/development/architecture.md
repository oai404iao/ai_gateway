# 当前架构

> 状态：当前。本文描述已实现的运行时架构；具体行为仍以代码、测试、migration 和 OpenAPI 契约为准。

## 系统定位

`ai-gateway` 是 Rust 2024 单二进制服务。生产运行时由 Axum/Tokio、reqwest、PostgreSQL/SQLx 和 `ArcSwap` 组成；Console Web UI 可在构建时嵌入二进制，生产环境不需要常驻 Node 服务。

系统支持三种数据面格式：

- `OpenAiChatCompletions`
- `OpenAiResponses`
- `OpenAiImages`

三种格式共享鉴权、选路、上游客户端和日志基础设施，但路由、变换、协议操作和 usage
解析保持隔离，禁止跨格式回退或转换。`ApiOperation` 进一步区分 Chat Completions、
Responses、standalone web search、Images generation 与 Images edit。Standalone web search
复用 `OpenAiResponses` 路由与授权维度，但拥有独立 capability、请求契约、目标路径和日志
operation；当前 Images 实现非流式 JSON generation 和非流式 multipart edit。

客户端 API 格式与上游接入方式是两个维度。Channel Group 另有
`ConnectorKind`：普通渠道使用 `openai_compatible`，Codex 订阅凭证使用
`codex_oauth`；后者可投影为 `OpenAiResponses` 与 `OpenAiImages` 渠道，但不会新增
provider-specific 客户端格式。

## 运行拓扑

```text
OpenAI-compatible client
  -> public listener
  -> /health or /v1/*
  -> API-key authentication and admission
  -> immutable control-plane snapshot
  -> channel selection and optional session affinity
  -> request transforms and in-process connector preparation
  -> reusable reqwest client or pinned Responses WebSocket
  -> streamed HTTP/SSE response or WebSocket events
  -> durable asynchronous request logging and settlement

Browser or Console client
  -> HTTPS reverse proxy
  -> dedicated Console listener
  -> embedded SPA and /console/v1/*
  -> JWT authorization
  -> PostgreSQL transaction and audit
  -> immediate snapshot publication
```

公共数据面和 Console 使用独立 listener。公共 listener 不挂载 Console API 或 UI；Console listener 上显式 API 路由优先于 SPA fallback。

## 数据面请求链路

1. 从请求头读取客户端 Bearer API Key。
2. 从一次性获取的不可变快照完成鉴权和格式权限判断。
3. 在读取 body 前执行进程内 RPM、并发和软额度准入。
4. 按操作和 Content-Type 读取请求体。Chat Completions、Responses、standalone web search 与
   Images generation 在 `proxy_body_bytes` 内读取 JSON；Images edit 在独立总大小/单文件限制内接收 multipart，
   超过内存阈值后写入匿名临时文件。两者都要求路由用 `model` 为非空、最多 300 字符；
   Images streaming fail closed。请求日志还会宽松提取客户端显式提供的
   `reasoning.effort`、`reasoning_effort` 和 `service_tier = "priority"`，但这些元数据不会
   增加转发校验。
5. 使用嵌入的 [`request-allowlists.json`](../reference/request-allowlists.json) 执行客户端入口
   白名单：未列出的 Header 被忽略，常见反向代理/CDN 转发元数据作为显式 Header `ignore`
   条目删除，未列出的顶层 JSON/multipart 字段返回 `400`；显式 body `ignore` 字段只在值满足
   契约时删除。当前只校验顶层字段，允许字段内部的嵌套结构仍由上游解释。随后按 API Key
   快照中的用户组策略执行可选 Fast 过滤：启用时删除顶层 `service_tier`，因此后续日志元数据、
   请求倍率、Session affinity、Transform 和 Connector 都只观察过滤后的请求。
6. 按公共操作与 `client_model` 取得预编译路由。`client_model`
   来自唯一绑定到顶层 routing profile 的计价 `models.source_model_id`；一个 profile 可包含
   多个操作唯一的规则。规则拥有按非负 `priority` 排序的 routing tier；数值越小越先
   尝试，每个 tier 独立选择 `weighted_random` 或 `weighted_round_robin`。停用的协议可以作为
   无 tier 的 `draft` 保存，启用协议必须至少有一个非空 tier。
   每个 tier 保存显式 `(capability_id, upstream_model, weight)` 候选；同一能力可在同层使用
   不同模型，也可跨层重复，只有同层完全相同的能力/模型组合不能重复。Console 逐行编辑
   能力、模型和权重，管理组仅提供选项上下文；以后加入或移出该组的渠道
   不会隐式改写既有规则。Console 只能从能力声明的 `available_models` 中选择模型，服务端
   在协议更新时再次验证。协议规则另存目标渠道位图和模型兼容位图；后续渠道能力变化可使已发布
   规则进入断开状态。
7. `accessible_routes` 通常按模型兼容渠道完成 O(1) 授权判断；只有规则全局没有任何模型兼容
   渠道时，才退回目标渠道位图，使原本已授权的断开规则仍可识别。随后使用渠道授权位图过滤
   实际模型兼容候选，并依次应用 operation capability、Session 粘性、规则中最低可用
   `priority` tier 和被动健康过滤。权重只比较该 tier 内仍然合格的渠道/模型候选，不跨 tier 比较；
   API Key 的 group/logical-channel 选择保存为固定能力授权；新增成员或能力不扩大已有授权，
   Console 路由编辑也不授予权限。HTTP 授权范围内没有可选候选时返回 `503 no_healthy_channel`；
   Responses WS 使用下文的 `426 websocket_unavailable` 回退提示。
   `/v1/models` 额外要求 API Key 范围与模型兼容位图相交，所以不公布断开规则。
   Standalone web search 使用独立 Search 能力与操作规则，不借用 Responses 规则。
   候选选定后，同步持久化客户端逻辑请求的日志 intent 并预留终态 slot；
   不可写或容量不足返回 `503 request_log_unavailable`，不进入 Connector 准备或上游 dispatch。
   重试复用同一预占；WS 每次 create 单独准入。见[日志耐久化](request-log-durability.md)。
8. 将客户端计价模型标识改写为最终候选携带的上游 wire model，并按“模板默认值 → 渠道覆盖”
   应用受限变换。普通 JSON 沿用
   JSON Patch；multipart edit 在无需别名时原样回放，需要别名时流式等价重建，只执行 Header
   变换而不执行请求 JSON Transform。Standalone web search 同样禁止 Request JSON Transform，
   但保留模型别名和 Header/响应 Header Transform。只要 body 被别名、JSON Transform 或 provider adapter
   改变，客户端完整性 Header 会先被移除，随后 Header Transform 才能设置与新 body 匹配的值。
9. 由进程内 Connector 的 `PreparedUpstreamAttempt` 完成 provider 特定 body、目标路径和最终
   Header/鉴权准备。Codex Connector 在普通 Transform 之后再次执行 provider body 白名单，
   只保留 wire type 声明的字段或显式兼容项；随后把
   `client_metadata["x-codex-installation-id"]` 和 turn metadata 的 `installation_id` 归一化为
   按逻辑凭证稳定的 opaque UUID，并把 `workspaces` 强制替换为系统设置中的单一合成 Git
   工作区。Responses HTTP/WebSocket 缺少 `client_metadata`、`prompt_cache_key` 或安全身份字段时
   会补齐；不伪造 request kind、sandbox、beta、subagent、attestation 或 turn-state，也不改变
   其他 metadata 和 W3C trace/baggage。普通 Connector 保持相同 API 路径和认证行为。
10. 清理客户端鉴权、hop-by-hop headers，并再次应用客户端 Header policy 中显式 `ignore` 的
   常见反向代理/CDN 转发元数据，防止 Header Transform 重新引入；Codex Connector 还会在该结果
   上执行 provider Header 白名单，并对 standalone web search 的合法
   `x-codex-turn-metadata` 应用相同安装/工作区归一化并在缺失时安全合成，再注入最终
   OAuth/account/protocol Header。该共享清理规则
   覆盖所有普通、Codex、HTTP/SSE、Images 与 Responses WebSocket 渠道共享的请求清理层。Connector
   鉴权和网关自有 coding Header 准备完成后，还会在交给 transport 前再次执行显式 `ignore`
   guard；自定义上游鉴权 Header 名若与该集合冲突则在控制面编译时直接拒绝。渠道模型发现和
   scheduled probe 等会应用 Header Transform 的内部请求同样执行最终 guard。HTTP
   `Accept-Encoding` 由网关拥有：下游值不会直接转发，普通请求向上游声明
   `gzip, deflate, br, zstd`，Range 请求使用 `identity`。随后使用按代理、TLS 和超时策略复用的
   reqwest client 直接转发，不经过 sidecar、Unix Socket RPC 或第二个 HTTP 服务。Responses
   渠道组可把 `request_compression` 从默认的 `default` 改为 `zstd`；此时 HTTP
   `POST /v1/responses` 的最终 JSON body 使用 Zstandard level 3 编码，并设置
   `Content-Encoding: zstd` 与 `Content-Type: application/json`。WebSocket、standalone
   search 与 Images 请求不使用该请求编码。
11. 上游响应按 `Content-Encoding` 流式解码；支持 gzip、RFC 1950 deflate、Brotli 和
    Zstandard，已知的多层 coding 按逆序解码。usage、错误诊断和 SSE Transform 只读取解码后的
    明文流，不缓冲完整响应。公共 listener 再按下游请求的 `Accept-Encoding` 独立选择 coding；
    已知小于 1KiB 的响应保持 identity，长度未知的可压缩非 SSE 流仍可立即流式重编码。SSE 保持
    identity 以避免事件延迟。未知或过深的上游 coding 在发送下游响应头前返回
    `502 upstream_content_encoding_unsupported`。
    表示被解码、变换或重编码时，失效的长度、range、ETag 和 digest 元数据会被移除。失败的文本
    响应仍旁路保留最长 16KiB 供请求日志诊断；只能在读取 body 时发现的损坏压缩流会终止当前
    body，并记录 `upstream_body_error`。
12. 同步保存已准入请求的终态 slot 与 spool，再释放预占，并异步投影和结算。
    完整 slot 可按 UUID 重放；终态缺失保留待核对，不伪造 usage 或零费用。

客户端和 Connector policy 均未删除/覆盖字段、且没有模型别名、body Transform、客户端请求
解码或渠道组请求压缩时，原始请求字节保持不变。`POST /v1/responses` 另外接受客户端
`Content-Encoding: zstd`；网关先在配置的 JSON 请求上限内解码，再执行解析、白名单与路由。
其他 JSON 接口只接受 identity。普通响应不会为了 usage 采集而整体缓冲。

### Images edit replayable body

`src/application/request_body.rs` 把 multipart edit 与普通 JSON 的内存生命周期隔离：

```text
downstream Body
  -> Memory(Bytes) until image_edit_memory_bytes
  -> anonymous TempFile after threshold
  -> multipart inspection for model/count/size
  -> exact replay or streamed adapter
  -> reqwest Body stream
  -> Drop closes and removes the anonymous file
```

Unix 目录与文件权限分别收紧为 `0700` 与 `0600`。实现不保留用户文件名对应的磁盘路径，不把
multipart 字段值或图片字节写入 tracing、请求日志、audit 或错误响应。普通
OpenAI-compatible edit 直接回放，或在模型别名存在时使用原 boundary 重建。Codex adapter
在第二次顺序读取中增量 base64 编码最多五张图片并写入另一个 replayable body；原始 multipart
和适配后的 JSON 都不需要完整驻留内存。

`GET /console/v1/system/load` 暴露当前活跃临时文件/字节、文件系统可用容量、累计落盘量和写入
失败。目录创建、写入或初始回放 seek 失败返回 `image_body_spool_unavailable`，并在任何上游
派发之前失败；发送期间的文件读取错误同样不会触发 Images 自动重试，并计入存储失败指标。

## 进程内 Upstream Connector

普通渠道通过 `credential_id` 引用独立上游身份，仓储在控制面事务内解析静态认证、
精确 Base URL 范围与启用状态，再编译完整快照；数据面不逐请求读取凭证数据库。
通用凭证轮换不改变渠道、模型或权限；Codex 身份仍走下述专属生命周期。
凭证 revision 与渠道 binding revision 共同参与 WebSocket 连接身份，池归还路径也验证
最新有效范围，避免在轮换或改绑期间借出的旧连接重新入池。

`src/application/connector.rs` 是静态链接的 Connector registry。代理主循环只调用统一的
prepare、body adaptation、URL、Header injection、pre-header retry capability 和 response
observation 接口，不包含 provider 的 OAuth claim、路径或 Header 细节。

- `OpenAiCompatible` attempt 是无状态路径：保留现有请求字节、API 路径和
  `UpstreamAuth` 注入。
- `CodexOauth` attempt 的实现位于 `src/application/codex/attempt.rs`：读取独立凭证快照，
  按操作分派 Responses HTTP SSE、Responses WebSocket、standalone web search、Images generation 或 Images edit
  约束，改写目标并注入 OAuth/account 与协议专用 Header。
- `ConnectorKind` 编译进 group/channel 快照。新增 provider 时扩展 registry 和独立 provider
  模块，不能在标准 Chat Completions/Responses/Images 逻辑中再建一套路由器。

Codex 的每个逻辑凭证属于一个 `connector_pools` 记录，并通过
`codex_oauth_credential_channels` 投影为独立的 Responses 与 Images provider-managed Channel；
同一 pool 有两个格式隔离的 Channel Group。普通 Channel CRUD 和批量修改在 repository 层拒绝
managed channel；provider API 在 serializable 控制面事务中同时创建/修改共享凭证和对应
projections，再编译并发布统一路由快照。
凭证有 workspace account ID 时由 account/member 共同确定；个人 Token 缺少 account ID 时按
user ID 确定。因此同一 Business workspace 可以包含多个独立凭证，Free/Plus/Pro 等个人凭证也
不需要伪造 workspace ID；单条/批量删除会清除 Token 并保留不含敏感信息的两个历史 channel
tombstone。
managed channels 保留为统一路由中的稳定壳，credential 的 enable/quota/重新授权状态由独立
Connector 快照判定；这样 Responses 新 Session 和 Images 请求可在发送前排除不可用账户，
Responses affinity hit 会持续命中原 channel 并 fail closed，不会因一次失败静默改绑账户。
路由顺序和权重不属于 managed channel 或 credential。Responses 与 Images 模型规则分别维护自己
的 routing tiers 和 group/channel assignments；一个格式的修改不会投影或同步到另一个格式。

Codex token 与 quota 使用独立 `ArcSwap` 凭证快照；两个 projection channel ID 指向同一份
credential，避免每次 token 轮换都重编译整个控制面。
维护 worker 从 PostgreSQL 周期收敛多实例更新，并以有界并发处理各凭证；单凭证 token refresh
同时使用进程内 mutex、PostgreSQL row lock 和 `refresh_generation`，防止 rotating refresh token
并发重用。正式代理请求仍直接通过 reqwest streaming path，不经过 worker actor。

Codex 凭证可移植性仍沿用相同 provider 边界：服务端显式导出 API 从 repository 读取敏感 Token
及实际引用的代理，生成不含路由权重的 version 2 原生 Bundle；高级导入页在浏览器内把原生、
CLIProxyAPI 和
Sub2API JSON 标准化成可编辑草稿，完成代理 CRUD/映射后再逐条调用既有服务端验证导入事务。导入
格式解析不是数据面职责，也不会绕过“account/user 至少存在一个”、models、代理 enable 或
managed channel 的现有不变量。代理删除使用 optimistic concurrency，并在 repository 层拒绝仍被渠道或待完成 OAuth
授权流引用的记录。

Responses WebSocket 使用同一个 `/v1/responses` 路径的 `GET` Upgrade。握手先验证 API Key
认证与 Responses `proxy` 权限，再要求数据库系统设置、API Key 所属用户和最终候选渠道三层均显式
允许 WebSocket；系统、用户和普通 channel 默认关闭。每条顺序的 `response.create` 重新读取当前快照并
独立执行鉴权、准入、选路、变换、Connector 凭证准备、usage 和日志。普通 Responses channel
由管理员显式声明能力；Codex OAuth Responses projection 在创建和 migration 时自动声明该能力，
Images projection 永不声明，并且 Responses 仍受系统与用户开关限制。由于
`previous_response_id` 的增量缓存属于具体上游连接，下游连接会固定到一个仍可用的上游渠道和
WebSocket 身份，不做请求多路复用。每个成功请求结束后，上游连接立即回到按 API Key、Session
握手身份、渠道/上游模型候选、网络配置、目标和最终 Header 精确隔离的有界空闲池；下一条消息
优先取回同一连接及候选。
上游客户端使用与 Codex 相同的 SHA 固定 OpenAI Tungstenite fork，并主动协商
`permessage-deflate`；未接受该扩展的上游仍使用未压缩消息。池只复用成功终态后的无残留连接。
系统设置动态配置是否启用、最大空闲连接数、空闲超时和连接
最长寿命；发布新快照时会立即清理失效 API Key、用户、渠道、网络身份和超出新容量的空闲连接。
连接池维护进程级空闲/借出数及命中、未命中、丢弃累计计数，并与下游活跃 Session 一同出现在
管理员系统负载快照中。
关闭流程单独跟踪 Axum Upgrade 后的任务：停止新 Upgrade 并清空空闲池，允许当前逻辑请求在全局
grace period 内完成，截止时强制取消，避免 Upgrade 脱离 Hyper connection tracker 后绕过进程排空。

握手还执行只读 WS 可用性预检：在同一不可变快照内检查 API Key 可访问的 Responses 路由及其候选、
显式 WS 能力、Connector 运行时可用性和实时被动健康，不占用 lease/半开探针、不推进轮转/粘性状态、
不进行数据库或上游调用。系统/用户关闭 WS 或全部无可选 WS 路由时均拒绝 Upgrade，并返回不暴露内部
原因的 HTTP `426 websocket_unavailable`。模型直到
`response.create` 才确定；混合能力场景或连接后可用性变化造成该模型没有 WS 候选时，返回带
`status: 426` 的错误帧并记录同码请求日志。HTTP 的 `503 no_healthy_channel`、
鉴权/准入错误和消息阶段的 `404 model_not_found` 保持不变。该兼容提示由 Codex 在客户端执行
HTTP fallback，不能成为 Gateway 对已派发消息自动重放的依据。

连接池中的具体上游 socket 是非空 `previous_response_id` 的状态载体。增量请求只允许使用精确命中的
池连接；池 miss 时不新建连接或联系上游，而是返回规范的
`404 previous_response_not_found`，由保有完整上下文的客户端重试。上游
`websocket_connection_limit_reached` 与 `previous_response_not_found` 同样被归类为可恢复的
连接状态丢失：控制事件不应用可配置 SSE/WebSocket JSON patch，当前 socket 被废弃，但渠道自动禁用和
Session affinity 不受影响。

## 重试与 Streaming 边界

- 自动故障转移覆盖收到响应头前的连接失败、建连超时和响应头超时；管理员还可显式配置
  `request_retry.retryable_status_codes`，在尚未向客户端发送响应时丢弃匹配的 4xx/5xx 响应并
  选择下一候选。状态码列表默认为空，因为重放可能产生重复工作或费用。
- Images generation/edit 不使用自动故障转移；上游尝试一旦开始即只返回该尝试结果。
- Images generation/edit 在渠道没有显式响应头超时时使用独立的系统 Images 响应头超时；
  建连和流空闲超时仍与其他格式共享。
- Standalone web search 在渠道没有显式响应头超时时使用独立的系统 Search 响应头超时；普通
  Connector 仍可按传输失败和显式状态码策略故障转移，Codex Connector 发送后不重试。
- Responses WebSocket 只在上游 Upgrade/建连完成前故障转移；`response.create`
  一旦发送就不再切换连接或渠道。
- 每次后续尝试排除已经尝试过的精确渠道/模型候选，并重新遵守授权、规则 tier、健康和 tier
  内权重；同一物理渠道上的其他模型仍可被选择。同一精确候选即使跨 tier 重复也不会再次尝试。
- 未列入 `retryable_status_codes` 的 HTTP 响应直接转发，不触发故障转移。
- 向客户端发送响应头或任何响应字节后，不得切换渠道。
- SSE 变换按解码后的事件边界处理，不按压缩或网络 chunk 处理，也不缓冲完整流。
- 客户端断开会释放上游响应体；流空闲超时只终止当前流，不再发起新尝试。

## 控制面与一致性

可选 [Codex 拼车](codex-sharing.md) 在授权快照中保护拼车/专用池投影，保留成员对普通渠道
的原有访问。仅拼车请求在 dispatch 前
通过独立单写线程完成金额预占的 WAL fsync。HTTP/SSE、Images 和每条 WebSocket 请求复用
同一完成回调进行幂等结算；失败或取消请求按零费用释放预占，成功请求缺失 usage 时保留
pending 并 fail closed。配置/窗口及对账由后台访问数据库，数据面不逐请求查库。
成员由车队席位中的用户 UUID 直接确定，不依赖下方的单用户组授权模型。本人 API Key
可以从席位获得拼车凭证目标，并与可选 API Key Policy 的普通目标组合；没有 Policy 时仍可
创建仅含拼车投影的 Key。它不分享会话上下文，也不提供多实例全局配额。

动态配置保存在 PostgreSQL。Console 写操作在事务中完成授权、候选配置校验、审计和提交；提交成功后立即编译并发布新的不可变快照。周期 worker 负责从数据库重新加载，以覆盖进程间或外部变更。

数据面不会为每个请求查询 PostgreSQL。用户 WebSocket 偏好和渠道 WebSocket 能力随完整控制面快照
编译并原子发布；Connector 动态凭证从独立不可变快照读取。进程内限流、被动健康、in-flight、
Session 粘性和 WebSocket 连接池不跨实例共享。

Console 用户采用单用户组模型。内置默认用户组和默认管理员组负责按用户邀请时的角色默认归属；用户
没有单独 API Key Policy 覆盖时，动态继承所在组的默认策略。除管理员按用户签发一次性邀请外，匿名
用户还可以使用管理员维护的可复用注册邀请码自助注册。邀请码明文不入库；注册事务锁定哈希匹配的
邀请码，原子检查启用状态、过期时间和剩余次数，再创建 active user、分配邀请码当前用户组与初始
余额并递增使用次数。注册成功后直接签发 Console session，不经过邮箱确认。

用户组还通过独立关联表授予 canonical Codex Responses Channel Group 的额度可见性。普通用户查询
始终按 JWT 用户当前所属组在 PostgreSQL 中限定 credential pool，只投影凭证 UUID、订阅等级和额度
窗口/周期字段；管理员 label、账户身份、Token、代理、运行状态和 reset-credit 等字段不进入 DTO。
该能力只挂载 owner-scoped `GET` 路由，不进入数据面快照，也不提供 refresh、reset 或其他 mutation。

用户批量修改在同一 serializable 事务中验证所有 `updated_at` 版本并统一审计，任一失败会回滚整批。
删除用户采用不可恢复的匿名化：撤销会话、邀请和 API Key，但保留用户主键以维持请求日志与审计记录
的引用完整性。Console session 保存 refresh token 哈希和浏览器 `User-Agent`；本人会话查询在响应中
派生当前、活跃、过期和已撤销状态，撤销操作始终按 JWT 主体限定 user ID。

管理员可在重新验证自己的当前密码后，为其他 active、已设置密码的用户生成 24 小时临时密码。
签发事务会替换原密码哈希、递增 `auth_version`、撤销全部 Console Session，并把账户标记为必须改密；
API Key 不受影响。临时密码登录只创建 `purpose = password_change` 的受限 Session，HTTP middleware
除刷新、退出和完成密码重置外拒绝所有 Console 路由。用户提交不同于临时密码的新密码后，事务原子
清除临时状态并撤销全部受限 Session，随后签发新的普通 Session。

路由快照为渠道和模型路由分配进程内 dense slot。每个协议规则 tier 保存自己的 priority、
selection strategy 和连续的
`CompiledCandidate(slot, channel, upstream_model, route_weight)` 数组；这里的
模型与 weight 都来自协议规则的显式候选，不是 Channel 的路由配置字段。计费快照始终来自顶层
profile 绑定的价格模型，不随重试选中的上游 wire model 改变。相同授权范围
的 API Key 共享
`AuthorizationProfile`，其中包含允许渠道和预计算的可达路由位图。可达路由通常按模型兼容
渠道授权；规则全局断开时改用目标渠道，以便原本已授权的请求得到明确的运行时不可用结果。
协议规则另存模型兼容渠道位图，用于 `/v1/models` 可见性；因此后续从 Channel 删除最后一个匹配
模型不会阻止发布，但会把协议规则标记为 `disconnected` 并从模型列表移除。模型查找、模型可达性判断和候选授权
均不创建请求级集合，也不按候选 UUID 回查快照；临时禁用和被动健康冷却不影响模型列表。

渠道健康、in-flight 和 half-open claim 使用渠道级原子状态；渠道状态注册表和平滑
加权轮询游标分别按 64 个 shard 隔离。加权随机使用无分配两遍扫描，重试使用固定
dense candidate slot 数组，因此正常选择路径没有全局渠道状态锁，也不创建候选
`Vec`、`HashSet` 或 Session affinity `Box`。

Channel Group 不再保存 priority 或 selection strategy，Channel 和 Codex credential 也不再保存
routing weight。Group 继续承担格式、Connector、启用、请求压缩和状态统计等资源配置；Channel
继续承担端点、鉴权、模型能力、变换、网络、计费和健康状态。路由层级与权重只由引用这些资源的
模型 profile 下的协议规则拥有；Group 在路由编辑器中只用于一次性批量加入当前 Channel，不进入
持久化路由或编译结果。

## 请求日志耐久链路

```text
terminal request event
  -> process-unique local append-only spool
  -> request_log_ingest via COPY
  -> immutable financial facts + pending work
     -> idempotent receipt and balance/quota settlement
     -> request_logs projection
```

spool、ingress 和 pending work 是独立的恢复来源。不得在 COPY 提交前 checkpoint，
也不得在事实和日志投影均成功前删除 ingress。结算不等待查询投影；
迁移与恢复保证见[独立计量事实](independent-metering.md)。
日志同时保存路由维度 `api_format` 和公共操作维度 `api_operation`，使同一 Images 格式中的
generation/edit 可以保持独立观测和迁移兼容性。

## 代码边界

| 模块 | 职责 |
| --- | --- |
| `src/http/` | Axum 路由、中间件、传输层错误映射 |
| `src/application/` | 代理、replayable request body、进程内 Connector registry、Console、控制面发布、日志编排 |
| `src/domain/` | API 格式、编译路由、凭据和值对象 |
| `src/routing/` | 渠道选择、被动健康、Session 粘性 |
| `src/transforms/` | 受限 JSON/Header/SSE DSL |
| `src/upstream/` | reqwest client 复用、Responses WebSocket 连接池、代理和超时策略 |
| `src/persistence/` | SQLx repository、事务和查询 |
| `src/runtime_config/` | TOML bootstrap 配置和 `ArcSwap` 快照 |
| `src/workers/` | 重载、Connector 凭证维护、日志 ingest/投影/结算、渠道自动化、花费排行榜快照 |
| `web/console/` | React Console SPA；仅构建/开发阶段使用 Node |

## 权威来源

| 主题 | 来源 |
| --- | --- |
| 支持的 API 格式 | `src/domain/api_format.rs` |
| 公共路由 | `src/http/mod.rs` |
| Images multipart/replay | `src/application/request_body.rs` |
| Responses WebSocket 转发与连接池 | `src/application/proxy/websocket.rs`、`src/upstream/websocket.rs` |
| Upstream Connector registry | `src/application/connector.rs` |
| Codex OAuth Connector | `src/application/codex/`、`src/persistence/codex.rs` |
| Console 路由 | `src/http/console.rs` |
| Console 契约 | `docs/openapi/console-v1.yaml` |
| 配置 schema | `src/runtime_config/mod.rs` |
| 数据库 schema | `migrations/` |
| 当前用户行为 | `docs/user/operations.md` |
| OpenAI 兼容边界 | `docs/reference/openai-compatibility.md` |

早期产品方向和已经失效的架构假设保存在
[产品与架构蓝图归档](../archive/product-blueprint.md)，不能用它覆盖当前代码行为。
