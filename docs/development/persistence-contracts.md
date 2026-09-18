# 持久化行为契约基线

> 状态：当前。记录 P1 固定的业务不变量，并同步 P2/P3 的实现定位和约束变化。
> 设计与后续切片见[持久化边界提案](persistence-boundaries.md)。

## 范围

P1 仅补充测试和职责清单，不修改生产金额计算、结算路径、公开 API 或 migration。
本页代码定位已随 [P2 接口收拢](persistence-interfaces.md)和
[P3 独立事实](independent-metering.md)更新；收费规则不变，异常隔离与财务来源的变化如下。
以下区分必须保持的业务保证与待替换的 PG 机制/已知缺陷；P2/P3 可以替换实现，
但不能靠删掉失败场景测试宣称保持了保证。

新增 PG 契约测试位于 `tests/contracts/persistence.rs`，作为
`tests/control_plane_integration.rs` 的子模块复用其私有临时数据库/fixture。
它不是独立 Cargo test target，不复制数据库创建和清理代码。
测试专用触发器只创建在随机测试数据库中。

## 金额与状态契约

| 契约 | 当前实现及可执行证据 |
| --- | --- |
| 有效单价先取 12 位，费用在各项求和、除以计价 token 单位后取 8 位；均为中点取偶 | `src/application/billing.rs` 的四个 `tests`，包含中点上下舍入、分项不可提前取整、有效价格先舍入 |
| 已选模型快照、渠道/请求/时间倍率决定费用，当前模型价格不能重算历史 | 原有 `src/application/proxy.rs` billing 单测；新增 `recovery_is_bounded_oldest_first_and_preserves_prices_and_probe_charges` |
| 金额使用 Decimal/NUMERIC；单条最大值精确保存、批量越界整笔失败 | `maximum_amount_is_exact_and_aggregate_overflow_never_partially_settles` |
| 普通余额可负、Key 软配额可透支，但已用额度不能负 | 原有 `settlement_claim_is_concurrent_idempotent_and_allows_soft_quota_overdraft`；migrations `0001`、`0003` |
| 成功缺 usage/费用不结算；路由前 rejected 无计费事实不是已结算零费用 | `unknown_rejected_and_zero_by_policy_remain_distinct_during_recovery`；均保留 `NotBillable`，内部分类见 P3 |
| 明确 failed/cancelled 始终归零，包括缺价格/usage 或旧事件携带正费用 | 同一测试；原有 `zero_cost_migration_refunds_and_reconciles_historical_failures` |
| 有费用不等于具备结算资格 | `missing_price_evidence_is_preserved_without_blocking_eligible_facts`；资格转移到独立事实/回执，不再毒化正常批次 |
| scheduled probe 仍按系统身份和既有价格计费，不能因 source 改为免费 | `recovery_is_bounded_oldest_first_and_preserves_prices_and_probe_charges` |
| 拼车窗口预算每席位除法按 8 位向零截断，不使用普通费用的中点取偶 | 新增 `src/codex_sharing.rs::tests::seat_budgets_truncate_instead_of_rounding_up_at_eight_places` |

P3 用生成的 `amount_state` 保存金额分类，没有改变公共 API 枚举。
只有 intent 的请求不能伪造成 terminal fact。

## 原子性、重放与恢复

| 保证 | 证据 |
| --- | --- |
| 唯一回执、余额、Key 已用额度及 pending 删除同事务；部分账户未更新时全部回滚 | `settlement_rolls_back_claims_and_all_accounts_on_overflow_or_missing_updates`：余额溢出、Key 溢出、用户/Key BEFORE UPDATE 跳过一行，共四个隔离场景 |
| 同 UUID 并发及提交后再次调用只扣一次 | 原有 `settlement_claim_is_concurrent_idempotent_and_allows_soft_quota_overdraft`；`batch_settlement_aggregates_account_updates_and_deduplicates_ids` |
| 用户与 Key 不匹配不可认领；有效行可独立结算 | 原有 `batch_settlement_classifies_ineligible_rows_independently`；`settlement_leaves_account_mismatch_unbilled_and_worker_recovers_durable_logs` |
| 确认丢失后的数据库效果由再次查询/调用恢复，而不是凭返回超时重复扣款 | 原有重复 `settle`/`settle_batch` 测试；未新增真实网络 COMMIT 回复丢失注入，不将调用级重试描述为网络故障证明 |
| 数据库最终写入后才能 ack ingress，COPY 接收后才能推进 spool checkpoint | 原有 `durable_request_log_pipeline_replays_spool_after_an_ingress_outage`；`src/request_log_spool.rs` checkpoint/reopen 测试；根 E2E 的 DB outage/replay 场景 |
| 相同 UUID 内容不一致不能悄悄覆盖 | 原有 `request_log_insert_is_idempotent_and_worker_continues_after_failure`；`request_log_batch_insert_isolates_duplicates_and_invalid_statuses` |
| 恢复扫描按 completed_at/id 有界推进，limit 最小为 1，不再次扣已结算行 | 新增 `recovery_is_bounded_oldest_first_and_preserves_prices_and_probe_charges` 固定跨时间顺序及 limit=0/1；同时间 UUID 排序由当前 SQL 定义 |
| slot 完整重放、未知 intent/撕裂 slot 不制造零费用终态 | 原有 `src/request_log_spool.rs` admission、latched failure、unknown intent、torn slot 测试 |
| 拼车对账可读未 billed 的确定费用，本身不改普通余额 | 新增 `unknown_rejected_and_zero_by_policy_remain_distinct_during_recovery` 直接调用 `sharing_completed_costs`；原有 sharing WAL 幂等/重启测试覆盖窗口账本 |
| 连续 migration 原子执行；失败/取消历史退款只执行一次 | 原有 `pending_migrations_commit_and_rollback_as_one_batch`、`zero_cost_migration_refunds_and_reconciles_historical_failures` |

新增 `database_constraints_reject_invalid_money_and_illegal_log_updates` 绕过应用直接执行 SQL，
验证 failed/cancelled 正费用、部分价格快照、非 USD、非正 token 单位、
负费用及越界 reasoning tokens 被 CHECK 拒绝；非结算字段更新与撤销回执被触发器拒绝。
`tests/contracts/facts.rs` 另验证无合格模型/价格的事实不能获得回执及旧认领列不可写。
这些是 PG 实现契约，未来后端必须证明同等业务效果，不要求复刻同一 SQLSTATE。

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
`MeteringQueries` 和 `SettlementRepository`；拼车费用读取也归 `MeteringQueries`。
费用读源迁移不得统一成一种 source 或时间口径。

| 查询 | 时间/source/权限边界 | 当前 P3 归属 |
| --- | --- | --- |
| `query_console_request_logs` / `query_console_request_log` | 本人查询绑定 user；管理视图及字段脱敏保持原状；时间筛选含两端；`billed` 过滤 billed_at | 日志投影 + 回执 LEFT JOIN，不再拥有认领权 |
| `personal_usage` | started_at 闭开区间、UTC 日桶、仅 client、本人 user | 费用/usage 读独立事实 |
| `cost_statistics` | started_at 闭开区间；可按 user/Key/channel/credential 限定；不默认排除 scheduled_test | 事实提供全部聚合维度；保留 NULL 费用与请求计数差异 |
| `channel_group_status` | started_at 闭开区间，24h/3d/7d UTC 分桶；启用统计且未删除组；不默认排除 scheduled_test | 财务部分来自事实；成功率/状态指标需保留当前定义 |
| `refresh_spend_leaderboard_snapshots` | 仅 client；Asia/Shanghai 日/周/月边界；写入 `spend_leaderboard_*` | 保留统计快照，费用源改为事实 |
| `persistence/codex.rs::CODEX_CURRENT_WINDOW_COSTS_LATERAL`、管理员/本人 quota history | 两个 managed projection 的 channel；started_at 在 period 起点至 ended_at 或 min(now,reset_at) 的闭开区间；cost 非 NULL；本人视图遵守 group/pool 可见性 | 读事实，不依赖日志或普通结算回执 |
| `persistence/codex_sharing.rs::sharing_completed_costs` | 按请求 UUID，最多 1000 个；cost 非 NULL；不要求 billed | 拼车恢复读事实，保持单向费用证据读取 |
| `settlement_backlog` / `settle_pending` | 只扫描合格 pending；排除账户不匹配 | unknown/invalid/账户异常有独立核对计数，不伪装成清零 |

### Codex quota 与拼车不是普通 Key 额度重置

- `persistence/codex.rs::persist_codex_quota` 和 `reconcile_codex_quota_window` 更新
  提供方 quota 观测/窗口历史，行锁与观测版本保证归并；窗口切换不是普通账户退款。
- `application/codex/mod.rs::reset_quota` 在既有锁范围内调用外部兑换，
  `record_codex_quota_reset_transaction` 写 reset 事件/审计；
  `claim_manual_codex_quota_reset` 以 reset event 记录窗口应用状态，不重置 Key 已用金额。
- `src/codex_sharing.rs` 的 `LedgerStore` / `Actor::{reserve,finish}` 拥有独立
  request UUID、窗口/席位/用户金额及 uncertain 状态；不得写普通余额或复用普通回执当窗口账本。
- `persistence/codex_sharing.rs::claim_sharing_ledger` 与 `main.rs` 保活拥有单实例锁；
  配置存储、窗口完整性加载不替代本地 WAL。

## 基线限制的演进

P1 固定了“不误扣、无部分扣款”，没有把旧日志表实现固化成未来保证。P3 已改进：

- 缺价格的历史费用归为 invalid，不再回滚正常结算批次。
- unknown/invalid/账户异常有独立核对计数与变化日志，但没有自动补账或 Console 核对页。
- 日志 DELETE 不再移除财务认领证据；事实/回执禁止删改，但没有启用日志 TTL。
- 查询表被锁或展示字段非法时，事实、结算、费用统计与拼车恢复仍可推进。

P2 的驱动边界继续由测试固定。具体实现仍仅支持 PostgreSQL，
SQLx source 只在 PG 测试/内部诊断中使用；没有 SQLite、分布式 exactly-once 或无损硬件保证。

## 验证入口

```bash
cargo test --locked --lib application::billing::tests
cargo test --locked --lib codex_sharing::tests
cargo test --locked --test control_plane_integration persistence_contracts
cargo test --locked --test control_plane_integration metering_facts
cargo fmt --check
cargo clippy --locked --workspace --all-targets
cargo test --locked --workspace
git diff --check
python3 scripts/check-docs.py
```

PG 测试使用现有 helper 创建随机 `ai_gateway_test_*` 数据库并清理，不修改业务库。
`TEST_DATABASE_ADMIN_URL` 必须指向管理数据库而不是 `ai_gateway`。
仅测试代码变更不调用付费上游或性能负载；对应系统 E2E 场景是现有证据入口，
不代表本次重复运行了 E2E。
