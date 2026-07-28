# 当前架构

> 状态：当前。本文描述已实现的运行时架构；具体行为仍以代码、测试、migration 和 OpenAPI 契约为准。

## 系统定位

`ai-gateway` 是 Rust 2024 单二进制服务。生产运行时由 Axum/Tokio、reqwest、PostgreSQL/SQLx 和 `ArcSwap` 组成；Console Web UI 可在构建时嵌入二进制，生产环境不需要常驻 Node 服务。

系统只支持两种数据面格式：

- `OpenAiChatCompletions`
- `OpenAiResponses`

两种格式共享鉴权、选路、上游客户端和日志基础设施，但路由、变换、SSE 识别和 usage 解析保持隔离，禁止跨格式回退或转换。

## 运行拓扑

```text
OpenAI-compatible client
  -> public listener
  -> /health or /v1/*
  -> API-key authentication and admission
  -> immutable control-plane snapshot
  -> channel selection and optional session affinity
  -> request transforms and upstream authentication
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
4. 在大小限制内读取 JSON，要求顶层 `model` 为非空字符串；可选 `stream` 必须
   为布尔值。
5. 从分 API 格式索引按 `(api_format, client_model)` 取得预编译模型路由；渠道组
   成员已经按规则 `upstream_model` 与 `channels.available_models` 求交并划分
   优先级 tier。
6. 先用 API Key 的 `accessible_routes` 位图完成 O(1) 模型可达性判断，再使用渠道
   授权位图过滤候选，并依次应用 Session 粘性、最低优先级、权重策略和被动健康过滤。
7. 必要时改写顶层模型别名，并按“模板默认值 → 渠道覆盖”应用受限变换。
8. 清理客户端鉴权与 hop-by-hop headers，最后注入上游鉴权。
9. 使用按代理、TLS 和超时策略复用的 reqwest client 转发到相同 API 路径。
10. 转发上游状态、响应头和响应流；仅在配置的响应/SSE 变换需要时改写。
11. 将终态事件写入本地 spool，并异步投影、提取 usage 和结算。

没有 body 变换或模型别名时，原始请求字节保持不变。普通响应不会为了 usage 采集而整体缓冲。

Responses WebSocket 使用同一个 `/v1/responses` 路径的 `GET` Upgrade。握手先验证 API Key
认证与 Responses `proxy` 权限，再要求数据库系统设置、API Key 所属用户和最终候选渠道三层均显式
允许 WebSocket；migration 和新记录均默认关闭。每条顺序的 `response.create` 重新读取当前快照并
独立执行鉴权、准入、选路、变换、usage 和日志。由于
`previous_response_id` 的增量缓存属于具体上游连接，下游连接会固定到一个仍可用的上游渠道和
WebSocket 身份，不做请求多路复用。每个成功请求结束后，上游连接立即回到按 API Key、Session
握手身份、渠道网络配置、目标和最终 Header 精确隔离的有界空闲池；下一条消息优先取回同一连接。
池只复用成功终态后的无残留连接。系统设置动态配置是否启用、最大空闲连接数、空闲超时和连接
最长寿命；发布新快照时会立即清理失效 API Key、用户、渠道、网络身份和超出新容量的空闲连接。
连接池维护进程级空闲/借出数及命中、未命中、丢弃累计计数，并与下游活跃 Session 一同出现在
管理员系统负载快照中。
关闭流程单独跟踪 Axum Upgrade 后的任务：停止新 Upgrade 并清空空闲池，允许当前逻辑请求在全局
grace period 内完成，截止时强制取消，避免 Upgrade 脱离 Hyper connection tracker 后绕过进程排空。

## 重试与 Streaming 边界

- 自动故障转移只覆盖收到响应头前的连接失败、建连超时和响应头超时。
- Responses WebSocket 只在上游 Upgrade/建连完成前故障转移；`response.create`
  一旦发送就不再切换连接或渠道。
- 每次后续尝试排除已经尝试过的渠道，并重新遵守授权、优先级、健康和权重规则。
- 上游返回任意 HTTP 响应头后，不再重试 HTTP 错误。
- 向客户端发送响应头或任何响应字节后，不得切换渠道。
- SSE 变换按事件边界处理，不按网络 chunk 处理，也不缓冲完整流。
- 客户端断开会释放上游响应体；流空闲超时只终止当前流，不再发起新尝试。

## 控制面与一致性

动态配置保存在 PostgreSQL。Console 写操作在事务中完成授权、候选配置校验、审计和提交；提交成功后立即编译并发布新的不可变快照。周期 worker 负责从数据库重新加载，以覆盖进程间或外部变更。

数据面不会为每个请求查询 PostgreSQL。用户 WebSocket 偏好和渠道 WebSocket 能力随完整控制面快照
编译并原子发布。进程内限流、被动健康、in-flight、Session 粘性和 WebSocket 连接池不跨实例共享。

Console 用户采用单用户组模型。内置默认用户组和默认管理员组负责按用户邀请时的角色默认归属；用户
没有单独 API Key Policy 覆盖时，动态继承所在组的默认策略。除管理员按用户签发一次性邀请外，匿名
用户还可以使用管理员维护的可复用注册邀请码自助注册。邀请码明文不入库；注册事务锁定哈希匹配的
邀请码，原子检查启用状态、过期时间和剩余次数，再创建 active user、分配邀请码当前用户组与初始
余额并递增使用次数。注册成功后直接签发 Console session，不经过邮箱确认。

用户批量修改在同一 serializable 事务中验证所有 `updated_at` 版本并统一审计，任一失败会回滚整批。
删除用户采用不可恢复的匿名化：撤销会话、邀请和 API Key，但保留用户主键以维持请求日志与审计记录
的引用完整性。Console session 保存 refresh token 哈希和浏览器 `User-Agent`；本人会话查询在响应中
派生当前、活跃、过期和已撤销状态，撤销操作始终按 JWT 主体限定 user ID。

路由快照为渠道和模型路由分配进程内 dense slot。模型 tier 保存连续的
`CompiledCandidate(slot, channel, weight)` 数组；相同授权范围的 API Key 共享
`AuthorizationProfile`，其中包含允许渠道和可达路由两个位图。模型查找、模型可达性
判断和候选授权均不创建请求级集合，也不按候选 UUID 回查快照。`/v1/models` 只读取
配置可达位图，不受临时健康冷却影响。

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

## 代码边界

| 模块 | 职责 |
| --- | --- |
| `src/http/` | Axum 路由、中间件、传输层错误映射 |
| `src/application/` | 代理、Console、控制面发布、日志编排 |
| `src/domain/` | API 格式、编译路由、凭据和值对象 |
| `src/routing/` | 渠道选择、被动健康、Session 粘性 |
| `src/transforms/` | 受限 JSON/Header/SSE DSL |
| `src/upstream/` | reqwest client 复用、Responses WebSocket 连接池、代理和超时策略 |
| `src/persistence/` | SQLx repository、事务和查询 |
| `src/runtime_config/` | TOML bootstrap 配置和 `ArcSwap` 快照 |
| `src/workers/` | 重载、日志 ingest/投影/结算、渠道自动化、花费排行榜快照 |
| `web/console/` | React Console SPA；仅构建/开发阶段使用 Node |

## 权威来源

| 主题 | 来源 |
| --- | --- |
| 支持的 API 格式 | `src/domain/api_format.rs` |
| 公共路由 | `src/http/mod.rs` |
| Responses WebSocket 转发与连接池 | `src/application/proxy/websocket.rs`、`src/upstream/websocket.rs` |
| Console 路由 | `src/http/console.rs` |
| Console 契约 | `docs/openapi/console-v1.yaml` |
| 配置 schema | `src/runtime_config/mod.rs` |
| 数据库 schema | `migrations/` |
| 当前用户行为 | `docs/user/operations.md` |
| OpenAI 兼容边界 | `docs/reference/openai-compatibility.md` |

未来方向和设计背景见 [产品与架构蓝图](product-blueprint.md)，但不能用蓝图覆盖当前代码行为。
