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

可选 `mcp-server` feature 不是第四种数据面格式。它在公共 listener 上提供
`/mcp/{slug}` transport；默认使用无状态 `2026-07-28` POST，可选兼容完整
`2025-11-25` Session/SSE，并接受 Codex 旧版模式使用的 `2025-06-18` 协商。当前
`web_search` kind 把 `web.run` 参数编译为
`ApiOperation::StandaloneWebSearch`，`image` kind 把 `image_gen.imagegen` 参数编译为
`ApiOperation::ImagesGeneration` 或 `ApiOperation::ImagesEdit`，随后进入同一认证后 Proxy
执行核心。

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

MCP client
  -> optional public /mcp/{slug}
  -> Host / Origin / MCP metadata validation
  -> API-key authentication and immutable MCP registry lookup
  -> built-in web.run or image_gen.imagegen adapter
  -> authenticated standalone-search or Images generation/edit Proxy execution
  -> bounded MCP CallToolResult

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
6. 从分 API 格式索引按 `(api_format, client_model)` 取得预编译模型路由。规则分别保存目标
   渠道位图、与 `channels.available_models` 求交后的模型兼容位图，以及由当前启用状态构成的
   优先级 tier。没有模型兼容渠道的规则仍保留为可发布的断开状态。
7. `accessible_routes` 通常按模型兼容渠道完成 O(1) 授权判断；只有规则全局没有任何模型兼容
   渠道时，才退回目标渠道位图，使原本已授权的断开规则仍可识别。随后使用渠道授权位图过滤
   实际模型兼容候选，并依次应用 operation capability、Session 粘性、最低优先级、权重策略和
   被动健康过滤。授权范围内没有可选候选时返回 `503 no_healthy_channel`。
   `/v1/models` 额外要求 API Key 范围与模型兼容位图相交，所以不公布断开规则。
   Standalone web search 只允许
   `supports_standalone_web_search = true` 的 Responses 渠道。
8. 必要时改写顶层模型别名，并按“模板默认值 → 渠道覆盖”应用受限变换。普通 JSON 沿用
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
12. 将终态事件写入本地 spool，并异步投影、提取 usage 和结算。

客户端和 Connector policy 均未删除/覆盖字段、且没有模型别名、body Transform、客户端请求
解码或渠道组请求压缩时，原始请求字节保持不变。`POST /v1/responses` 另外接受客户端
`Content-Encoding: zstd`；网关先在配置的 JSON 请求上限内解码，再执行解析、白名单与路由。
其他 JSON 接口只接受 identity。普通响应不会为了 usage 采集而整体缓冲。

### MCP adapter

`src/mcp/mod.rs` 使用 feature-gated `rmcp` Streamable HTTP transport。每个请求只获取一次
`ArcSwap` 快照并认证一次 Gateway API Key，然后把 `CompiledMcpServer`、`CompiledApiKey` 和同一
快照放入 request extension。`src/mcp/search.rs` 只负责静态 `web.run` schema、typed validation、
显式 `search_session_id`、域名策略和 Search body/result 映射；它不查询 PostgreSQL、不回环
HTTP，也不保存 MCP Session。`src/mcp/image.rs` 提供静态 `image_gen.imagegen`
generation/edit schema：无引用时编译 JSON generation；有引用时验证最多五个显式
PNG/JPEG/WebP base64 data URL，并逐块解码为既有 replayable multipart edit。两种操作都固定
实例的模型、background、quality、size 和单图 PNG/base64 输出，并返回带
`codex/imageDetail = original` 的 MCP `ImageContent`；它不抓取远程 URL，也不保存图片或服务端文件。

启用的 MCP 定义随其他控制面记录编译进按 slug 索引的不可变 registry。数据库不能上传任意工具
代码或 schema；`kind` 只选择二进制中已链接的实现。全局 MCP transport 设置同样存入
`system_settings` 并随快照热更新；TOML 只在该节首次缺失时引导。默认只支持
`2026-07-28` 每请求 metadata，不发 `Mcp-Session-Id`。开启
`allow_legacy_2025_11_25` 后，RMCP 同时提供 `initialize` / `notifications/initialized`、
`Mcp-Session-Id`、请求级/独立 GET SSE 和 DELETE；它协商 `2025-11-25`，也接受 Codex
旧版模式固定发送的 `2025-06-18`。现代请求仍无状态。旧 Session 使用进程内
`LocalSessionManager`，设置变更、关闭或重启会终止，集群部署必须在入口层保持粘性路由。

Search 使用普通 MCP envelope 上限；Image endpoint 使用独立 inline-edit envelope 上限。
Images 输入还限制单图/解码总字节，Search 与 Images 结果分别按独立上限有界收集。底层转发
日志使用 `request_source = "mcp"`，但不保存 tool arguments、prompt、图片字节或结果。无论
是否存在旧协议 Session，Search ref-id 和 Images edit 输入都保持显式无状态，不读取 Session
历史。

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

Codex token 与 quota 使用独立 `ArcSwap` 凭证快照；两个 projection channel ID 指向同一份
credential，避免每次 token 轮换都重编译整个控制面。
维护 worker 从 PostgreSQL 周期收敛多实例更新，并以有界并发处理各凭证；单凭证 token refresh
同时使用进程内 mutex、PostgreSQL row lock 和 `refresh_generation`，防止 rotating refresh token
并发重用。正式代理请求仍直接通过 reqwest streaming path，不经过 worker actor。

Codex 凭证可移植性仍沿用相同 provider 边界：服务端显式导出 API 从 repository 读取敏感 Token
及实际引用的代理，生成带版本的原生 Bundle；高级导入页在浏览器内把原生、CLIProxyAPI 和
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
握手身份、渠道网络配置、目标和最终 Header 精确隔离的有界空闲池；下一条消息优先取回同一连接。
上游客户端使用与 Codex 相同的 SHA 固定 OpenAI Tungstenite fork，并主动协商
`permessage-deflate`；未接受该扩展的上游仍使用未压缩消息。池只复用成功终态后的无残留连接。
系统设置动态配置是否启用、最大空闲连接数、空闲超时和连接
最长寿命；发布新快照时会立即清理失效 API Key、用户、渠道、网络身份和超出新容量的空闲连接。
连接池维护进程级空闲/借出数及命中、未命中、丢弃累计计数，并与下游活跃 Session 一同出现在
管理员系统负载快照中。
关闭流程单独跟踪 Axum Upgrade 后的任务：停止新 Upgrade 并清空空闲池，允许当前逻辑请求在全局
grace period 内完成，截止时强制取消，避免 Upgrade 脱离 Hyper connection tracker 后绕过进程排空。

## 重试与 Streaming 边界

- 自动故障转移只覆盖收到响应头前的连接失败、建连超时和响应头超时。
- Images generation/edit 不使用自动故障转移；上游尝试一旦开始即只返回该尝试结果。
- Images generation/edit 在渠道没有显式响应头超时时使用独立的系统 Images 响应头超时；
  建连和流空闲超时仍与其他格式共享。
- Standalone web search 在渠道没有显式响应头超时时使用独立的系统 Search 响应头超时；普通
  Connector 仍可在响应头前故障转移，Codex Connector 发送后不重试。
- Responses WebSocket 只在上游 Upgrade/建连完成前故障转移；`response.create`
  一旦发送就不再切换连接或渠道。
- 每次后续尝试排除已经尝试过的渠道，并重新遵守授权、优先级、健康和权重规则。
- 上游返回任意 HTTP 响应头后，不再重试 HTTP 错误。
- 向客户端发送响应头或任何响应字节后，不得切换渠道。
- SSE 变换按解码后的事件边界处理，不按压缩或网络 chunk 处理，也不缓冲完整流。
- 客户端断开会释放上游响应体；流空闲超时只终止当前流，不再发起新尝试。

## 控制面与一致性

动态配置保存在 PostgreSQL。Console 写操作在事务中完成授权、候选配置校验、审计和提交；提交成功后立即编译并发布新的不可变快照。周期 worker 负责从数据库重新加载，以覆盖进程间或外部变更。
全局 MCP transport 的 enable、公开 origin、浏览器 origin、协议兼容和大小限制也属于该
`system_settings` 快照；`mcp-server` Cargo feature 仍是构建时边界，不能通过数据库动态加载。

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

路由快照为渠道和模型路由分配进程内 dense slot。模型 tier 保存连续的
`CompiledCandidate(slot, channel, weight)` 数组；相同授权范围的 API Key 共享
`AuthorizationProfile`，其中包含允许渠道和预计算的可达路由位图。可达路由通常按模型兼容
渠道授权；规则全局断开时改用目标渠道，以便原本已授权的请求得到明确的运行时不可用结果。
模型规则另存模型兼容渠道位图，用于 `/v1/models` 可见性；因此删除最后一个渠道模型不会阻止
发布，但会把规则标记为 `disconnected` 并从模型列表移除。模型查找、模型可达性判断和候选授权
均不创建请求级集合，也不按候选 UUID 回查快照；临时禁用和被动健康冷却不影响模型列表。

渠道健康、in-flight 和 half-open claim 使用渠道级原子状态；渠道状态注册表和平滑
加权轮询游标分别按 64 个 shard 隔离。加权随机使用无分配两遍扫描，重试使用固定
dense channel slot 数组，因此正常选择路径没有全局渠道状态锁，也不创建候选
`Vec`、`HashSet` 或 Session affinity `Box`。

## 请求日志耐久链路

```text
terminal request event
  -> process-unique local append-only spool
  -> request_log_ingest via COPY
  -> request_logs projection
  -> idempotent balance/quota settlement
```

spool 和 ingress 分别形成两层可恢复 backlog。不得在数据库 COPY 提交前 checkpoint，也不得在最终投影成功前删除 ingress 记录。
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
| `src/persistence.rs`、`src/persistence/` | 公共持久化 facade、后端中立 records/auth/error、数据库不透明边界、SQLx repository、事务和查询；PostgreSQL row mapping 与完整运行时实现位于 `postgres/`；SQLite feature 当前提供 schema/type、runtime-snapshot 只读仓储，以及 Console login/session/profile/password/recovery/bootstrap、注册邀请码、自助注册和 invite/reinvite/accept 仓储，但运行时数据库选择仍仅支持 PostgreSQL |
| `src/runtime_config/` | TOML bootstrap 配置和 `ArcSwap` 快照 |
| `src/workers/` | 重载、Connector 凭证维护、日志 ingest/投影/结算、渠道自动化、花费排行榜快照 |
| `web/console/` | React Console SPA；仅构建/开发阶段使用 Node |

## 权威来源

| 主题 | 来源 |
| --- | --- |
| 支持的 API 格式 | `src/domain/api_format.rs` |
| 公共路由 | `src/http/mod.rs` |
| 可选 MCP transport 与 Search/Images adapters | `src/mcp/mod.rs`、`src/mcp/search.rs`、`src/mcp/image.rs` |
| Images multipart/replay | `src/application/request_body.rs` |
| Responses WebSocket 转发与连接池 | `src/application/proxy/websocket.rs`、`src/upstream/websocket.rs` |
| Upstream Connector registry | `src/application/connector.rs` |
| Codex OAuth Connector | `src/application/codex/`、`src/persistence/postgres/codex.rs` |
| Console 路由 | `src/http/console.rs` |
| Console 契约 | `docs/openapi/console-v1.yaml` |
| 配置 schema | `src/runtime_config/mod.rs` |
| 数据库 schema | PostgreSQL `migrations/*.sql`、SQLite 基线 `migrations/sqlite/*.sql` |
| 当前用户行为 | `docs/user/operations.md` |
| OpenAI 兼容边界 | `docs/reference/openai-compatibility.md` |

早期产品方向和已经失效的架构假设保存在
[产品与架构蓝图归档](../archive/product-blueprint.md)，不能用它覆盖当前代码行为。
