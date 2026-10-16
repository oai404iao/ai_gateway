# 持久化行为契约基线

> 状态：当前。金额、事务和恢复的业务基线；两后端用不同机制实现同一不变量。

修改金额、结算、恢复或统计前，先用本页确认哪些行为不能改变，再按读写路径找到实现。
事实与回执是财务来源，查询日志只是投影；不能为了接口重构或后端适配删掉故障测试。

## 目录

- [金额与状态契约](#金额与状态契约)
- [原子性、重放与恢复](#原子性重放与恢复)
- [费用与状态读写路径清单](#费用与状态读写路径清单)
- [保证与证明边界](#保证与证明边界)
- [验证入口](#验证入口)

PG 基线位于 `tests/contracts/persistence.rs`，作为
`tests/control_plane_integration.rs` 的子模块复用其私有临时数据库/fixture。
它不是独立 Cargo test target，不复制数据库创建和清理代码。
测试触发器只创建于随机库。SQLite 的真实文件库和共享 case 见
[计量对照](sqlite-metering.md)；PG 原生锁/COPY 测试不因双后端出现而移除。

## 金额与状态契约

| 契约 | 当前实现及可执行证据 |
| --- | --- |
| 有效单价先取 12 位，费用在各项求和、除以计价 token 单位后取 8 位；均为中点取偶 | `src/application/billing.rs` 的四个 `tests`，包含中点上下舍入、分项不可提前取整、有效价格先舍入 |
| 已选模型快照、渠道/请求/时间倍率决定费用，当前模型价格不能重算历史 | `src/application/proxy.rs` billing 单测；`recovery_is_bounded_oldest_first_and_preserves_prices_and_probe_charges` |
| 金额使用 Decimal/NUMERIC；单条最大值精确保存、批量越界整笔失败 | `maximum_amount_is_exact_and_aggregate_overflow_never_partially_settles` |
| 普通余额可负、Key 软配额可透支，但已用额度不能负 | `settlement_claim_is_concurrent_idempotent_and_allows_soft_quota_overdraft`；migrations `0001`、`0003` |
| 成功缺 usage/费用不结算；路由前 rejected 无计费事实不是已结算零费用 | `unknown_rejected_and_zero_by_policy_remain_distinct_during_recovery`；均保留 `NotBillable`，内部分类见 P3 |
| 明确 failed/cancelled 始终归零，包括缺价格/usage 或旧事件携带正费用 | 同一测试；`zero_cost_migration_refunds_and_reconciles_historical_failures` |
| 有费用不等于具备结算资格 | `missing_price_evidence_is_preserved_without_blocking_eligible_facts`；资格转移到独立事实/回执，不再毒化正常批次 |
| scheduled probe 仍按系统身份和既有价格计费，不能因 source 改为免费 | `recovery_is_bounded_oldest_first_and_preserves_prices_and_probe_charges` |

生成的 `amount_state` 保存内部金额分类，不改变公共 API 枚举。
只有 intent 的请求不能伪造成 terminal fact。

## 原子性、重放与恢复

| 保证 | 证据 |
| --- | --- |
| 唯一回执、余额、Key 已用额度及 pending 删除同事务；部分账户未更新时全部回滚 | `settlement_rolls_back_claims_and_all_accounts_on_overflow_or_missing_updates`：余额溢出、Key 溢出、用户/Key BEFORE UPDATE 跳过一行，共四个隔离场景 |
| 同 UUID 并发及提交后再次调用只扣一次 | `settlement_claim_is_concurrent_idempotent_and_allows_soft_quota_overdraft`；`batch_settlement_aggregates_account_updates_and_deduplicates_ids` |
| 用户与 Key 不匹配不可认领；有效行可独立结算 | `batch_settlement_classifies_ineligible_rows_independently`；`settlement_leaves_account_mismatch_unbilled_and_worker_recovers_durable_logs` |
| 确认丢失后的数据库效果由再次查询/调用恢复，而不是凭返回超时重复扣款 | 重复 `settle`/`settle_batch`；证明调用级幂等，不证明真实网络 COMMIT 回复丢失 |
| 数据库最终写入后才能 ack ingress，COPY 接收后才能推进 spool checkpoint | `durable_request_log_pipeline_replays_spool_after_an_ingress_outage`；spool checkpoint/reopen；系统 E2E DB outage/replay |
| 相同 UUID 内容不一致不能悄悄覆盖 | `request_log_insert_is_idempotent_and_worker_continues_after_failure`；`request_log_batch_insert_isolates_duplicates_and_invalid_statuses` |
| 恢复扫描按 completed_at/id 有界推进，limit 最小为 1，不再次扣已结算行 | `recovery_is_bounded_oldest_first_and_preserves_prices_and_probe_charges` 固定跨时间顺序及 limit=0/1；同时间 UUID 排序由 SQL 定义 |
| slot 完整重放、未知 intent/撕裂 slot 不制造零费用终态 | spool admission、latched failure、unknown intent、torn slot 测试 |
| 连续 migration 原子执行；失败/取消历史退款只执行一次 | `pending_migrations_commit_and_rollback_as_one_batch`、`zero_cost_migration_refunds_and_reconciles_historical_failures` |

`database_constraints_reject_invalid_money_and_illegal_log_updates` 绕过应用直接执行 SQL，
验证 failed/cancelled 正费用、部分价格快照、非 USD、非正 token 单位、
负费用及越界 reasoning tokens 被 CHECK 拒绝；非结算字段更新与撤销回执被触发器拒绝。
`tests/contracts/facts.rs` 另验证无合格模型/价格的事实不能获得回执及旧认领列不可写。
这些是 PG 数据库保护证据；SQLite 证明同等业务效果，不要求复刻 SQLSTATE。

## 费用与状态读写路径清单

下表使用稳定符号定位；SQL 的精确过滤条件仍以代码为准。

### 产生、落盘、结算与账户管理

| 路径 | 当前所有者 | 存储/事务与后续归属 |
| --- | --- | --- |
| 客户端终态费用；探测费用 | `application/billing.rs::request_billing`；`proxy.rs::CompletionGuard`；`workers/channel_probe.rs` | 只生成事件，不逐请求查库扣款；保留不可变快照 |
| 派发前 intent、终态 slot/spool | `application/request_log.rs`、`request_log_admission.rs`、`request_log_spool.rs`、`request_log_journal.rs` | 本地版本化耐久证据；保持第一阶段语义 |
| COPY、投影、ack/defer | `RequestLogRepository::{accept_batch,project_batch,acknowledge_ingest,defer_ingest}` | COPY 后由计量物化，再独立写 `request_logs`；确认必须有事实和投影 |
| 计量物化 | `persistence/metering.rs::MeteringRepository` | 事实、工作项和 ingress ready 原子提交；兼容直接终态入口 `insert_batch` 先提交事实再尝试投影 |
| 普通账户扣款及恢复 | `SettlementRepository::{settle,settle_batch,settle_pending}` | 唯一回执与账户更新，不依赖查询表；扫描 pending 而非全部历史 |
| 自动结算调用方 | `workers/durable_request_log.rs` 与 `workers/mod.rs::RequestLogWorker` | 生产耐久 worker 与旧入口共用同一事实/回执结算规则 |
| 进程内 Key 额度 | `workers/mod.rs::handle_settlement_outcome` → `admission/mod.rs::record_settled_quota_usage` | 提交后单调更新缓存；不是数据库余额事实 |
| 管理员单条设余额 | `persistence/postgres_control_plane.rs::user_update`，经 `ControlPlaneCoordinator::mutate` | 绝对赋值；控制面 SERIALIZABLE + 候选校验 + 审计，不属于请求结算回执 |
| 管理员批量 set/increase/decrease | `ControlPlaneRepository::update_users_batch`，经 coordinator 同名方法 | 仍属控制面原子管理操作；不得拆成逐行非事务更新 |
| 初始余额/邀请码余额 | `persistence/postgres_control_plane.rs::user_create`；`persistence/auth.rs::{invite_user,register_with_invitation}` | 用户/邀请事务；不是上游消费 |
| Key 修改/撤销/软删除 | `persistence/postgres_control_plane.rs` Key mutation 与 `revoke_own_api_key` | 修改授权或额度上限不重置 `quota_used_amount`，墓碑保留旧费用归属 |
| 历史冲正 | migration `0061_zero_failed_and_cancelled_costs.sql` | 已 billed 正费用失败/取消按原 user/key 退款，整批预检与回滚；没有运行时自动补账 API |

### 查询与财务读源

除 Codex 模块外，下列 PG 查询位于 `persistence/postgres_control_plane.rs` 的 `RequestLogQueries`、
`MeteringQueries` 和 `SettlementRepository`。
费用读源迁移不得统一成一种 source 或时间口径。

| 查询 | 时间/source/权限边界 | 当前读源 |
| --- | --- | --- |
| `query_console_request_logs` / `query_console_request_log` | 本人查询绑定 user；管理视图及字段脱敏保持原状；时间筛选含两端；`billed` 过滤 billed_at | 日志投影 + 回执 LEFT JOIN，不再拥有认领权 |
| `personal_usage` | started_at 闭开区间、UTC 日桶、仅 client、本人 user | 费用/usage 读独立事实 |
| `cost_statistics` | started_at 闭开区间；可按 user/Key/channel/credential 限定；不默认排除 scheduled_test | 事实提供全部聚合维度；保留 NULL 费用与请求计数差异 |
| `channel_group_status` | started_at 闭开区间，24h/3d/7d UTC 分桶；活动组/渠道的已启用统计能力；不默认排除 scheduled_test | 成功率、延迟和吞吐来自日志投影，按组与 API format 区分 |
| `refresh_spend_leaderboard_snapshots` | 仅 client；Asia/Shanghai 日/周/月边界；写入 `spend_leaderboard_*` | 保留统计快照，费用源改为事实 |
| `persistence/codex.rs::CODEX_CURRENT_WINDOW_COSTS_LATERAL`、管理员/本人 quota history | 按不可变请求凭证归属；started_at 在 period 起点至 ended_at 或 min(now,reset_at) 的闭开区间；cost 非 NULL；本人视图遵守用户组的渠道组可见性 | 事实 JOIN request_credential_identities；旧未知才用冻结历史回退，不依赖日志、当前改绑或回执 |
| `settlement_backlog` / `settle_pending` | 只扫描合格 pending；排除账户不匹配 | unknown/invalid/账户异常有独立核对计数，不伪装成清零 |

### Codex quota 不是普通 Key 额度重置

- `persistence/codex.rs::persist_codex_quota` 和 `reconcile_codex_quota_window` 更新
  提供方 quota 观测/窗口历史，行锁与观测版本保证归并；窗口切换不是普通账户退款。
- `application/codex/mod.rs::reset_quota` 在既有锁范围内调用外部兑换，
  `record_codex_quota_reset_transaction` 写 reset 事件/审计；
  `claim_manual_codex_quota_reset` 以 reset event 记录窗口应用状态，不重置 Key 已用金额。

## 保证与证明边界

行为基线保证“不误扣、无部分扣款”，不把旧日志表认领机制固化为业务要求：

- 缺价格的历史费用归为 invalid，不再回滚正常结算批次。
- unknown/invalid/账户异常有独立核对计数与变化日志，但没有自动补账或 Console 核对页。
- 日志 DELETE 不再移除财务认领证据；事实/回执禁止删改，但没有启用日志 TTL。
- 查询表被锁或展示字段非法时，事实、结算与费用统计仍可推进。

驱动边界由[接口契约](persistence-interfaces.md)固定。当前同时支持 PostgreSQL 和 Linux SQLite；
应用不解释 SQLx source，两后端各自保留真实约束/迁移/恢复测试。
重复调用证明幂等账户效果，不证明端到端 exactly-once；SIGKILL、取消和重启也不证明
所有硬件断电组合。既有调用级重试测试没有注入真实 PG COMMIT 网络回复丢失。

这组测试最初在 P1 固定收费规则，P2 收拢驱动接口，P3 将财务来源迁出日志。
这些编号只解释演进，不表示 SQLite 或独立事实仍待实现；设计理由见
[持久化边界](persistence-boundaries.md)。

## 验证入口

```bash
cargo test --locked --lib application::billing::tests
cargo test --locked --test control_plane_integration persistence_contracts
cargo test --locked --test control_plane_integration metering_facts
cargo fmt --check
cargo clippy --locked --workspace --all-targets
cargo test --locked --workspace
cargo test --locked --workspace --features sqlite-backend
git diff --check
python3 scripts/check-docs.py
```

PG 测试使用现有 helper 创建随机 `ai_gateway_test_*` 数据库并清理，不修改业务库。
`TEST_DATABASE_ADMIN_URL` 必须指向管理数据库而不是 `ai_gateway`。
这些契约命令不调用付费上游或性能负载；系统 E2E 是独立验证入口，
仓储 case 不替代真实服务、CLI 与浏览器的故障恢复验收。
