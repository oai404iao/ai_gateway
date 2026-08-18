# 数据库 Repository 契约与 M1 方法台账

> 状态：当前。本文记录 M1 后端中立持久化契约和基线方法台账；实现、测试与 migration 仍具有更高来源优先级。

## 当前状态 (Current status)

M1 将应用可见 DTO、错误和事务意图移到后端中立模块，但暂时保留三个 PostgreSQL repository
实现类型作为公开 facade 名称：

- `AuthRepository`
- `ControlPlaneRepository`
- `RequestLogRepository`

`DatabaseConnectOptions`、`DatabasePool` 与 `RepositoryTransaction` 对应用调用方提供 opaque
公共边界；应用、HTTP、MCP、worker、runtime compiler 和进程入口不得访问具体 SQLx pool、
connection、transaction 或 backend module。但 `src/persistence/database.rs` 内部的
`DatabaseConnectOptions`/`connect_pool`、migrator 与 `From<PgPool>` 仍是 PostgreSQL/SQLx-backed
过渡性生命周期 plumbing；M2 才负责 URL/backend lifecycle parsing、闭合枚举 facade dispatch
和具体 conversion 的移除。

当前应用可见 repository 名称**仍只连接 PostgreSQL**。即使编译 `sqlite-backend`，正常验证的
`[database].url` 仍由 `src/runtime_config/mod.rs` 拒绝 `sqlite:` URL；不得把
`DatabaseConnectOptions::from_str` 当作独立配置激活门禁。feature-gated SQLite direct adapter
只用于当前 storage/repository contract 和 integration tests，不参与进程运行时选择，直到后续
里程碑完成并通过激活门禁。

## M1 权威方法台账

### 计数规则

本基线按 repository 调用方可见的操作入口计数，总数固定为 **104**：

| Family | 数量 | 计数边界 |
| --- | ---: | --- |
| Auth | 24 | 统计 `AuthRepository` 的公开异步操作，不统计 constructor `new` |
| RequestLog | 21 | 统计公开操作和 worker 可见的 `pub(crate)` 操作，不统计 constructor `new` |
| ControlPlane core | 34 | 统计核心公开操作，不统计 constructor `new` 和 adapter helper `proxy_record` |
| ControlPlane Codex | 25 | 统计 `ControlPlaneRepository` 的 Codex 公开操作 |
| **总计** | **104** | 方法重载、内部 SQL helper、row adapter 和 DTO 不单独计数 |

新增、删除、重命名或改变可见性、asyncness、参数、返回值或 lifetime 的 repository 操作必须同步
更新本台账和 `tests/fixtures/persistence-repository-signatures.txt`。
`tests/persistence_architecture_integration.rs` 按 family 对已归一化空白的完整签名做集合比较。
constructor、私有 helper、row decoder 和 backend adapter 不属于应用操作契约。

### 生产调用方

| Repository | 当前生产调用方 |
| --- | --- |
| `AuthRepository` | `src/main.rs`、`src/application/auth.rs` |
| `ControlPlaneRepository` | `src/main.rs`、`src/application/control_plane.rs`、`src/application/codex/`、`src/application/proxy_test.rs`、`src/workers/` |
| `RequestLogRepository` | `src/main.rs`、`src/application/request_log.rs`、`src/http/console.rs`、`src/workers/` |

测试可以直接建立 fixture 或调用 repository，但不能据此把 concrete SQLx 类型扩散到上述生产
调用方。后续调用方变化必须同步本表和架构门禁。

### Auth（24）

1. `find_login_user`
2. `validate_console_identity`
3. `create_session`
4. `password_user`
5. `rotate_session`
6. `revoke_session_for_user`
7. `revoke_all_sessions`
8. `revoke_other_sessions`
9. `sessions_for_user`
10. `profile`
11. `update_display_name`
12. `registration_invitation_codes`
13. `registration_invitation_code`
14. `change_password`
15. `issue_temporary_password`
16. `complete_temporary_password`
17. `reset_active_admin_password`
18. `create_registration_invitation_code`
19. `update_registration_invitation_code`
20. `register_with_invitation_code`
21. `invite_user`
22. `reissue_invitation`
23. `accept_invitation`
24. `bootstrap_admin`

### RequestLog（21）

以下前七项为 durable worker 所需的 crate-visible 接缝，其余为查询、统计、插入与结算入口：

1. `copy_ingest_batch`
2. `load_ingest_batch`
3. `acknowledge_ingest`
4. `defer_ingest`
5. `ingest_backlog`
6. `settlement_backlog`
7. `pool_status`
8. `list_for_user`
9. `get_for_user`
10. `list_all`
11. `get`
12. `personal_usage`
13. `channel_group_status`
14. `cost_statistics`
15. `refresh_spend_leaderboard_snapshots`
16. `spend_leaderboard`
17. `insert`
18. `insert_batch`
19. `settle`
20. `settle_batch`
21. `settle_pending`

### ControlPlane core（34）

1. `ensure_system_probe_identity`
2. `ensure_system_settings`
3. `load`
4. `load_runtime`
5. `load_runtime_transaction`
6. `system_settings`
7. `user_settings`
8. `update_user_settings`
9. `load_transaction`
10. `begin_management_write`
11. `active_user_exists`
12. `active_admin_exists`
13. `automatically_disable_channel`
14. `automatically_recover_channel`
15. `control_plane_lists`
16. `control_plane_channel_detail`
17. `control_plane_config_template_detail`
18. `control_plane_mcp_server`
19. `audit_logs`
20. `own_api_keys`
21. `own_api_key`
22. `own_api_key_options`
23. `create_own_api_key`
24. `update_own_api_key`
25. `revoke_own_api_key`
26. `update_users_batch`
27. `update_channels_batch`
28. `apply_control_plane_mutation`
29. `model_source_ids`
30. `apply_catalog_models`
31. `insert_audit`
32. `insert_self_audit`
33. `insert_system_audit`
34. `insert_manual_reload_audit`

### ControlPlane Codex（25）

1. `begin_codex_refresh`
2. `begin_codex_quota_reset`
3. `codex_credentials`
4. `codex_credential_view`
5. `codex_quota_window_history`
6. `self_codex_quota_credentials`
7. `self_codex_quota_window_history`
8. `codex_credential`
9. `codex_credential_for_update`
10. `load_codex_credentials`
11. `set_codex_user_id_if_missing`
12. `export_codex_credentials`
13. `create_codex_oauth_flow`
14. `codex_oauth_flow`
15. `insert_codex_credential`
16. `update_codex_credential`
17. `delete_codex_credential`
18. `update_codex_credentials_batch`
19. `persist_codex_token_refresh_transaction`
20. `persist_codex_quota`
21. `record_codex_quota_reset`
22. `record_codex_quota_reset_transaction`
23. `mark_codex_credential_error`
24. `mark_codex_credential_error_transaction`
25. `cleanup_codex_oauth_flows`

## 当前 SQLite 直接实现

当前 SQLite repository 直接实现 **26** 个操作：

- Auth 的上述全部 24 个操作；
- runtime reader 的 `load` 与 `load_runtime`。

这 26 项是 feature-gated SQLite foundation，不表示应用 repository 名称或正常运行时已支持
SQLite，也不能从 104 项 PostgreSQL 基线中扣除或重复加入总数。

## 契约模块 ownership

| 模块 | 拥有的后端中立契约 |
| --- | --- |
| `src/persistence/auth.rs` | Console identity、Session、Profile、密码、邀请和注册 DTO；`numeric(24,8)` normalization |
| `src/persistence/request_log.rs` | 请求日志查询/统计 DTO、ingest/backlog DTO、insert/batch/settlement outcome |
| `src/persistence/control_plane.rs` | 非 Codex 控制面 DTO、mutation input/result 和 batch input |
| `src/persistence/codex.rs` | Codex OAuth、credential、quota、window、import/export 和 batch DTO |
| `src/persistence/records.rs` | runtime snapshot 读取所需的 backend-neutral storage records |
| `src/persistence/error.rs` | `RepositoryError`、opaque `RepositoryErrorSource` 和错误分类 |
| `src/persistence/database.rs` | 应用 opaque 的 pool/transaction 边界和 `TransactionIntent`；其中 connect options、pool 创建、migrator 和 concrete pool conversion 在 M2 前仍是 SQLx-backed 过渡 plumbing |
| `src/persistence/postgres/` | PostgreSQL SQL、row mapping、SQLx error adapter 使用方和三个临时 repository 实现 |
| `src/persistence/sqlite/` | feature-gated SQLite schema/row adapter、runtime reader 和直接 auth 实现 |

`src/persistence.rs` 是公共重导出面。它不得公开 `postgres`/`sqlite` module，也不得用
`pub use postgres::*` 把 backend implementation detail 扩散给调用方。

## 后端中立错误类别

持久化 adapter 必须先分类 backend error，再交给 application/HTTP；上层不得解析 SQLSTATE、
SQLite code 或错误字符串。

- `NotFound`：记录不存在或当前状态不可变更。
- `Conflict`：业务/optimistic concurrency 冲突。
- `Validation`：repository input 无效。
- `TransactionConflict`：serialization、deadlock 或等价的 transaction 竞争。
- `Constraint`：adapter 已明确分类的唯一键、外键、非空、check 和既有 Console
  validation SQLSTATE。扩大分类集合会改变 HTTP 行为，必须作为单独的跨后端契约变更评审，
  未识别的表示错误在此之前进入 `DatabaseFailure`。
- `Busy` / `Timeout`：资源暂时繁忙或操作超时。
- `BackendMismatch`：repository、transaction、backend 或 pool identity 不匹配。
- `Corrupt`：存储或已持久化数据损坏、无法安全解码。
- `StorageUnavailable`：pool/connection/I/O/TLS 等存储不可用。
- `Migration`：为 migration 执行或版本问题冻结的中立类别；M1 尚未把 `run_migrations` 的
  `MigrateError` 或连接建立错误接入该分类，M2 负责 lifecycle mapping。
- `DatabaseFailure`：无法安全归入以上类别的 opaque fallback。

既有 domain-specific variants（例如受保护用户组、重复 request-log immutable facts）继续表达比
通用类别更精确的业务结果。`RepositoryError::source()` 可以返回 opaque
`RepositoryErrorSource` wrapper；该 wrapper 是 terminal source，自己的 `source()` 返回 `None`，
且 `Debug`/`Display` 不泄露私有保存的具体 backend error，因此 application/HTTP 无法沿标准错误链
遍历或 downcast SQLx error。

## TransactionIntent

四个稳定意图及其标识为：

| Variant | 稳定标识 | 含义 |
| --- | --- | --- |
| `ConsistentRead` | `consistent_read` | 在一致快照中读取完整 runtime/control-plane view |
| `ManagementWrite` | `management_write` | 原子执行管理 mutation、并发检查和 audit |
| `RequestLogWrite` | `request_log_write` | 短事务执行幂等 ingest/final projection |
| `Settlement` | `settlement` | claim 日志并原子更新 billing、余额和 API Key quota |

M1 冻结的目标契约是：transaction 只能传给由**同一 backend 和同一逻辑 pool identity**创建的
repository，任何错配返回 unit variant `BackendMismatch`，不得 panic、隐式重连或访问另一
backend 分支。当前 PostgreSQL-only facade 尚没有可发生的跨 backend dispatch，也未携带完整 pool
identity；M2 构造闭合枚举 facade 时负责实际执行该检查。

## Request-log outcome 语义

### 单条 insert

`insert` 返回 `RequestLogInsertOutcome::Inserted` 或 `ExactDuplicate`。相同 UUID 只有在所有
immutable event facts（含数据库时间精度 normalization 后）完全相同时才是幂等成功；冲突事实返回
`DuplicateConflict` error，非法 HTTP status 返回 `InvalidResponseStatus` error。

### batch insert

`insert_batch` 为每个输入按原顺序返回 `RequestLogBatchInsertResult`，其 outcome 为 `Inserted`、
`ExactDuplicate`、`DuplicateConflict` 或 `InvalidResponseStatus { status }`。单个输入的冲突或验证
失败不遮蔽同批其他输入；backend statement/transaction failure 仍使整个 database operation
失败。重复 UUID 的分类必须确定且不改写已存在 immutable facts。

### settlement

`settle`/`settle_batch` 返回：

- `Settled { request_log_id, api_key_id, quota_used_amount }`：唯一 claim、用户余额、API Key quota
  和 `billed_at` 已在同一事务提交；
- `AlreadyBilled`：此前已完成结算，幂等 no-op；
- `NotBillable`：没有可结算成本；
- `AccountMismatch`：日志 user 与 API Key owner 不一致；
- `NotFound`：日志不存在。

`settle_batch` 对输入 UUID 去重并保留首次出现顺序。claim 或任一 account update 失败必须整体
rollback；只有 commit 后才可更新进程内 soft quota。`settle_pending` 只扫描有成本且 account
匹配的未结算事实，并复用同一 settlement 语义。

## 架构门禁

`tests/persistence_architecture_integration.rs` 固定以下 M1 边界：

- production caller 不直接提及 concrete SQLx backend API/type；
- neutral contract module 不依赖 `sqlx`、`FromRow` 或 Pg/Sqlite concrete type；
- concrete backend module 保持私有，PostgreSQL module 只公开三个临时 repository 实现类型；
- 六个既有 facade 名称、错误与事务意图保持可导入；
- 上述四个 family 的 `(family, visibility, asyncness, name, normalized full signature)` 和
  **104** 总数保持一致。

rustdoc `compile_fail` 示例另行证明外部调用方不能访问 `DatabasePool::postgres()` 或
`persistence::postgres`。

## 相关文档

- [数据库后端抽象与 SQLite 完成总计划](database-backend-completion-plan.md)
- [数据库与控制面架构](database-architecture.md)
- [请求日志耐久化流水线](request-log-durability.md)
- [文档规范](../documentation-standard.md)
