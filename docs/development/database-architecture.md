# 数据库与控制面架构

> 状态：当前。数据库结构以 `migrations/` 为唯一权威来源；本文只说明当前持久化边界、
> 主要实体组和运行时编译关系，不逐列复制 schema。

## 设计边界

- PostgreSQL 保存控制面配置、Console 身份与会话、Connector 状态、请求日志、统计投影和审计事实。
- TOML 只保存进程/bootstrap 配置，以及数据库系统设置缺失时的一次性初始值。
- 数据面从 `ArcSwap<CompiledRuntimeConfig>` 读取不可变快照，不在每个代理请求中查询数据库。
- Console 控制面写入在事务中完成候选配置校验和审计；提交成功后立即发布完整新快照，定时重载负责
  跨进程收敛。
- schema 只能通过新的有序 migration 演进；不得修改已经部署的 migration 来伪造当前结构。

## 持久化实现边界

应用层不直接持有 SQLx 的 PostgreSQL 连接、连接池或 transaction 类型。启动、系统负载和应用服务
通过 `src/persistence/database.rs` 暴露的 `DatabaseConnectOptions`、`DatabasePool` 和
`RepositoryTransaction` 使用不透明边界；当前唯一可启动和注册仓储的实现仍是 PostgreSQL。
后端中立的运行时快照记录、Console auth/session 契约、系统设置契约和仓储错误分别位于
`src/persistence/records.rs`、`src/persistence/auth.rs` 与 `src/persistence/error.rs`。具体
SQL、COPY、PostgreSQL 查询构造、`FromRow` mapping 和仓储实现位于
`src/persistence/postgres/`；公共 DTO 与仓储 API 继续从 `src/persistence.rs` 重导出。

这个边界不表示当前已经支持其他数据库，也不改变 PostgreSQL migration 或事务语义。新增后端必须
提供独立 schema/migration 和仓储实现，并通过相同应用层 API 与持久化契约测试，不能依赖
`sqlx::AnyPool` 隐藏 SQL 方言差异。

### SQLite 实现状态

Cargo feature `sqlite-backend` 已提供第二后端的 schema、存储类型、runtime-snapshot
只读仓储和 Console login/session/account 仓储，但还不是用户可选的运行模式：

- `migrations/sqlite/0001_current_schema.sql` 是对应 PostgreSQL migration `0049` 之后结构的
  独立新基线；SQLite 从未发布过旧 schema，因此不复制 PostgreSQL 的 49 步历史。
- `src/persistence/sqlite.rs` 暴露独立 `SQLITE_MIGRATOR`、精确十进制 TEXT adapter，以及
  PostgreSQL `uuid[]`/`text[]` 对应的 JSON TEXT adapter；`src/persistence/sqlite/row.rs`
  把这些存储类型显式解码为后端中立记录。
- `SqliteRuntimeConfigRepository` 已能在一个 SQLite read transaction 中加载完整
  `RuntimeConfigRecords` 并交给相同的 runtime compiler，但进程启动仍不会选择这个仓储。
- `SqliteAuthRepository` 已直接实现 login identity、Session 创建/校验/列举/撤销和 refresh
  token rotation/replay 撤销。rotation 使用 `BEGIN IMMEDIATE` 在读取 hash 前取得 SQLite write
  lock；并发提交同一旧 refresh token 时只允许一次 rotation，随后 replay 会撤销该 Session。
  SQLite 内建 `lower()` 仅处理 ASCII，因此该仓储按 Rust Unicode lowercase 后的 canonical email
  精确查询；SQLite user writer 持久化同一形式。
- 同一仓储还实现 Profile/display-name、永久与临时密码、临时密码并发完成、emergency admin
  reset 和 bootstrap-admin。所有跨 user/Session/audit 的 mutation 在 `BEGIN IMMEDIATE` transaction
  内完成；审计 JSON 从 typed row 构建且精确 Decimal 直接写为 JSON number，不经过 `f64`。邀请、
  registration code 和 self-registration 仍只由 PostgreSQL 仓储实现。
- UUID 使用 16-byte BLOB，UTC 时间使用 RFC 3339 TEXT，JSON/数组使用 JSON TEXT，精确金额与
  单价使用十进制 TEXT；禁止经过 SQLite `REAL` 或 Rust `f64` 做计费运算。
- schema 保留外键、主要 check/unique/index、审计 append-only 和 `request_logs` 单次结算边界。
  PostgreSQL 的 Codex projection PL/pgSQL trigger 不在 SQLite 中模拟；后续 SQLite 仓储必须在
  同一事务显式完成这些关联写入。
- 文件连接的目标策略是 foreign keys、WAL、`synchronous=FULL` 和 busy timeout；当前测试固定
  这些要求，运行时连接工厂会在其余仓储与 dispatch 完成后接入。
- `tests/sqlite_schema_integration.rs` 在两个独立临时数据库上应用各自 migration，并固定 27 张
  领域表、334 个列及其 SQLite storage affinity、38 个外键和显式索引的一一对应。
- `tests/sqlite_runtime_repository_integration.rs` 固定 UUID BLOB、decimal TEXT、JSON
  list/document、时间和 secret 字段的 runtime record 解码，排除 system Key/deleted MCP 行，
  并把读取结果送入现有 `compile_runtime_config`。
- `tests/sqlite_auth_repository_integration.rs` 固定 login/access identity、normal/password-change
  Session、refresh rotation/replay、并发 write-lock、ownership revocation、Session ordering/state
  和 malformed timestamp fail-closed 行为。
- `tests/sqlite_account_repository_integration.rs` 固定 Profile exact-decimal decoding、
  `updated_at` trigger 后重读、password/Session mutation、并发 completion/bootstrap、canonical
  email、完整 redacted audit shape 和 audit insertion 失败时的 transaction rollback。

`src/runtime_config/mod.rs` 仍只接受 `postgres://`/`postgresql://`，默认构建也不包含
`sqlite-backend`。在 SQLite 仓储、事务和日志/结算实现完成前，不得把 `sqlite:` URL 写入配置
模板或用户文档，也不得宣称 SQLite 已可用于部署。

最初的 11 表方案及其当时的取舍已移入
[首版数据库设计归档](../archive/initial-database-design.md)。它不能作为当前列名、表数量或功能边界
的依据。

## 当前实体组

截至 migration `0049_user_group_fast_mode_filter.sql`，migration 历史创建了 27 张表。维护者不应
把这个数量写成稳定产品契约；新增 schema 时应直接阅读全部 migration。当前实体可按职责分为：

| 领域 | 主要表 | 责任 |
| --- | --- | --- |
| 身份与授权 | `users`、`user_groups`、`user_sessions`、`user_invitations`、`registration_invitation_codes`、`api_key_policies`、`api_keys` | Console 身份、角色、生命周期、注册/邀请、用户可选路由边界和具体 Key 限制。 |
| 模型与路由 | `models`、`model_rules`、`channel_groups`、`channels`、`proxies`、`config_templates`、`system_settings` | 价格、上游 wire 模型、格式隔离、路由目标、Connector、网络/变换和数据库动态系统策略。 |
| Codex Connector | `connector_pools`、`codex_oauth_credentials`、`codex_oauth_credential_channels`、`codex_oauth_flows`、`codex_quota_window_periods`、`codex_quota_reset_events`、`user_group_codex_quota_visibility` | 共享逻辑凭证、Responses/Images 投影、OAuth、quota 历史和用户组可见性。 |
| MCP | `mcp_servers` | 静态内置 kind 的实例定义；transport 全局设置保存在 `system_settings`。 |
| 日志与统计 | `request_log_ingest`、`request_logs`、`spend_leaderboard_periods`、`spend_leaderboard_entries`、`audit_logs` | 耐久日志入口、查询/结算事实、排行榜投影和控制面审计。 |

## 关键当前语义

### 格式、模型和路由

- `api_format` 当前包含 `open_ai_chat_completions`、`open_ai_responses` 和
  `open_ai_images`。
- `model_rules` 以 `(client_model, api_format)` 唯一；`upstream_model_id` 指向一个
  `models` 行。
- 被引用 `models.source_model_id` 同时是发往上游的 wire 模型名和该请求的价格来源。migration
  `0006_simplify_model_routes_and_request_log_filters.sql` 已删除旧的独立
  `model_rules.upstream_model` 列。
- 规则、渠道组和渠道必须保持格式一致。启用规则可以暂时没有模型兼容或活跃渠道；快照仍可发布，
  实际请求按普通路由错误失败。
- `channel_groups.request_compression` 当前为 `default` 或 `zstd`；只有 Responses group 可以
  选择 `zstd`。
- `channels.health_check` 已在 migration `0017_remove_legacy_compatibility.sql` 删除。当前定时测试、
  自动禁用和超时策略来自现有渠道列与 `system_settings.forwarding_policy`。

### 用户组与 Fast 过滤

`user_groups.filter_fast_mode` 会编译进每个 `CompiledApiKey`。启用时，数据面在客户端白名单之后
删除顶层 `service_tier`，因此后续日志元数据、请求计费倍率、Session affinity、Transform 和
Connector 都只观察过滤后的请求。

### 系统设置

`system_settings` 不是预留表。固定的 `forwarding_policy` 文档保存并热更新以下策略：

- 上游超时、请求重试、被动健康、自动禁用和定时测试；
- Session affinity 与 Responses WebSocket；
- MCP transport、协议兼容和 request/result limits；
- Codex 合成 workspace path、HTTPS Git remote 等转发元数据策略。

首次启动只在对应设置不存在时使用 TOML bootstrap 值。之后数据库记录是动态运行时来源。

### 请求日志与结算

```text
terminal RequestLogEvent
  -> process-unique local durable spool
  -> request_log_ingest
  -> indexed request_logs
  -> idempotent settlement and statistics projection
```

- 本地 spool 覆盖数据库写入前的进程崩溃恢复；入口和最终表都依赖请求 UUID 幂等。
- `request_logs` 保存最终选中路由、usage、有效价格快照、成本和有界错误诊断，不保存 prompt、
  completion、完整 Header、Cookie 或密钥。
- Codex quota 当前与历史窗口的凭证总花费按逻辑凭证的 Responses/Images projection、周期边界和
  `cost_amount IS NOT NULL` 从该表聚合；现有 `(channel_id, started_at)` 索引支撑该只读查询。
- Chat Completions、Responses 和 standalone web search 可按数据库策略在收到上游响应头前自动
  重试不同渠道；Images 不自动重试，Responses WebSocket 发送上游消息后也不重试。
- 当前每个逻辑请求仍只写一条最终 `request_logs` 记录。数据库的旧 `attempts` JSONB 列不承载
  当前重试详情；尝试次数只进入完成 tracing。
- 结算以 `billed_at IS NULL` 取得唯一处理权，在同一事务更新用户余额和 API Key 已用额度。

完整耐久性和故障边界见[请求日志耐久化流水线](request-log-durability.md)。

## 修改数据库的流程

1. 新增有序 PostgreSQL migration，不修改已发布 migration；同时新增对应的 SQLite migration
   并保持最终语义一致。SQLite `0001` 已是发布基线，后续同样只能向前追加。
2. 同步 `src/persistence/` DTO/查询、领域类型、运行时编译器和 Console mutation。
3. 若 Console API 形状变化，先改 `docs/openapi/console-v1.yaml`，再生成并提交 TypeScript 类型。
4. 更新本文件或相应专题设计文档，但不要复制可从 migration 直接读取的完整列清单。
5. 使用任务专用数据库或隔离 stack 验证 migration；不要把未发布 schema 应用到其他 worktree
   共用的开发数据库。
6. 运行 PostgreSQL 相关 Rust 门禁和 Console 契约测试；涉及 SQLite 时还要运行
   `cargo clippy --all-targets --features sqlite-backend` 和
   `cargo test --features sqlite-backend --lib`，再运行
   `cargo test --features sqlite-backend --test sqlite_schema_integration`、
   `cargo test --features sqlite-backend --test sqlite_runtime_repository_integration` 与
   `cargo test --features sqlite-backend --test sqlite_auth_repository_integration`，再运行
   `cargo test --features sqlite-backend --test sqlite_account_repository_integration`。

## 来源

- PostgreSQL schema：`migrations/*.sql`
- SQLite schema：`migrations/sqlite/*.sql`
- 数据库不透明边界：`src/persistence/database.rs`
- 后端中立运行时记录、auth/session 契约与错误：`src/persistence/records.rs`、
  `src/persistence/auth.rs`、`src/persistence/error.rs`
- PostgreSQL row mapping 与仓储：`src/persistence/postgres/`
- SQLite migration 与存储 adapter：`src/persistence/sqlite.rs`
- SQLite runtime row mapping 与只读仓储：`src/persistence/sqlite/row.rs`、
  `src/persistence/sqlite/runtime.rs`
- SQLite Console login/session/account 仓储：`src/persistence/sqlite/auth.rs`、
  `src/persistence/sqlite/auth/account.rs`
- 快照编译：`src/runtime_config/mod.rs`
- 当前请求链路：[当前架构](architecture.md)
- Console API：`docs/openapi/console-v1.yaml`
- 用户可观察行为：[运行与接口说明](../user/operations.md)
