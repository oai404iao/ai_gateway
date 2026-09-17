# 0063 升级与成对备份恢复演练

> 状态：当前。P4 的可重复小数据量演练；不是生产硬件容量认证，也不操作真实业务库。

## 范围与安全边界

本次部署者确认当前数据很少，选择目录 Compose 的开发 PostgreSQL，并指定升级离线窗口
10 分钟、完整恢复流程 30 分钟。因此不生成百万/千万行负载，也不运行 forwarding
性能测试或付费真实上游。

实现位于 `tests/contracts/rehearsal.rs`，通过 `contracts/facts.rs` 复用 schema 62
夹具和真实 PG 仓储；默认 `cargo test` 忽略此手动测试。它只创建和操作两个随机
`ai_gateway_test_<uuid>` 数据库，不读取、迁移、覆盖或恢复 `ai_gateway` 业务库。
数据库、Key、Codex 凭证和请求均为合成数据，不能连接真实提供方。

这里验证的是停写条件下的数据库/本地证据操作流程。没有启动旧版本二进制，
而是实际验证 schema 62 的 migration registry 拒绝 schema 63，以及旧扣款/ack SQL
被数据库拒绝。Gateway 启停、HTTP/CLI 和故障重放由独立的[系统 E2E](system-e2e.md)覆盖；
本报告时长不包含实例排空、服务管理器启动、DNS、人工操作或外部流量恢复。

## 操作入口

前提：

- 使用根目录 Compose 已启动的 `ai-gateway-postgres`，无需再起第二个开发栈。
- `TEST_DATABASE_ADMIN_URL` 通过进程环境指向同一实例的 `postgres` 管理库；
  由本地受保护配置读取密码，不把 URL/密码写入报告或提交到仓库。
- 容器内使用 `ai_gateway` 角色的本地 socket 调用 PG 同版本 `pg_dump` / `pg_restore`；
  依赖 Compose 镜像的 `timeout` 命令。不要改为任意生产容器。
- 创建私有、全新的输出目录；备份和报告保留供审查，不自动清理。

从任务 worktree 执行：

```bash
umask 077
mkdir -p "$HOME/.local/state/agents/tmp"
export PERSISTENCE_REHEARSAL_DIRECTORY="$(
  mktemp -d "$HOME/.local/state/agents/tmp/persistence-rehearsal.XXXXXXXX"
)"
export PERSISTENCE_REHEARSAL_CONTAINER=ai-gateway-postgres
# TEST_DATABASE_ADMIN_URL 已由受保护的本地配置注入环境。
TMPDIR="$PERSISTENCE_REHEARSAL_DIRECTORY" \
  cargo test --locked --test control_plane_integration \
  schema62_backup_cutover_and_paired_restore -- --ignored --nocapture
```

每次测试在指定目录中再创建随机子目录：

| 路径 | 内容 |
| --- | --- |
| `backup/database.dump` | schema 62 的 PG custom-format 完整合成库备份 |
| `backup/spool/` | 同一停写点的 events、checkpoint、admissions、完整拼车 ledger/WAL |
| `backup-manifest.json` | 备份内每个文件的相对路径与 SHA-256 |
| `source-spool/`、`restored-spool/` | 分别升级原库、恢复副本时使用的本地证据 |
| `report.json` | 结果、规模、时限、版本/脏工作区标记、库名、空间和 WAL 测量 |

成功后仅删除本次两个随机数据库；备份和报告保留。失败保持 `incomplete` 报告及已记录的
随机库名，不能当作通过；按报告逐个核实是否还有 PG 工具/连接，再决定重试或人工清理。
不扫描删除其他会话的测试库或输出目录。
PG 工具以容器内 30 分钟 watchdog 限时，不依赖工具可能自行重置的会话超时设置；
迁移有 10 分钟总超时、
590 秒单语句超时与 10 秒锁超时。失败后不得绕过保护直接使用部分恢复结果。

## 夹具与验收

旧库包含 5 条日志：已 billed、可计价未 billed、成功但费用未知、失败策略归零、
无费用拒绝。ingress 有 3 条记录，其中 2 条重复已 billed UUID；spool 有 2 个终态，
其中一个又重复已 billed UUID，另有一个只写 intent 的未决请求。
拼车包含已使用金额、待恢复终态预占和无终态预占。

演练顺序：

1. 完成夹具写入并停止写者，flush 拼车 WAL。PG dump 与整个 spool 树在同一静止状态复制，
   生成成对 SHA-256 清单。不能把活跃写入时的顺序复制当作一致快照。
2. 通过生产 `run_migrations` 执行 0063，确认回填不更新余额/Key 额度，
   已 billed 的 UUID 和原时间一致。旧 claim、提前 ack 和旧 migration registry 必须失败。
3. 启动真实 durable worker 恢复 ingress/spool；仅对三个新增的已知费用各扣一次，
   失败写零额回执。明确检查 unknown 的费用仍为 NULL、rejected 不适用，两者均无回执。
4. 校验完整备份清单，在第二个全新库以 `pg_restore --single-transaction --exit-on-error`
   恢复 schema 62；复制配套 spool，核对旧账户、拼车账本身份、金额、预占。
5. 再迁移并重放。逐 UUID 比较全部财务字段、回执金额/资格、所有账户余额/Key 额度，
   不是只比较总金额。新产生回执的时间可不同，历史回执时间必须相同。
6. 拼车从计量事实恢复已有终态的费用，仅保留无终态预占；未决 intent 文件仍存在。
   再次重启 worker，仍不得重复扣款，unknown/rejected 仍不能被偷偷结清。

配套自动测试（不需要备份工具）：

```bash
cargo test --locked --test control_plane_integration metering_facts
```

其中 migration 等待在途旧认领、异常历史回执导致整个迁移回滚、事实/ready 原子性、
日志投影故障下结算/拼车恢复等故障测试，见[独立计量事实](independent-metering.md)。
完整门禁仍包括 Rust format/clippy/workspace tests、系统 E2E 与文档检查。

## 测量口径与发布判定

- 升级离线计时：静止后的备份开始，直到迁移、首次重放与账户核对完成。
- 恢复总计时：检查备份、创建新库、restore、复制证据、账本恢复、再迁移与两次重放核对。
- `database_bytes_before/after` 是 PG 逻辑库大小，不是文件系统峰值、容器卷或临时文件峰值。
- `cluster_wal_bytes_upper_bound` 来自集群级 WAL insert LSN 差值，可能包括其他开发库的写入，
  不能称为本次迁移的精确 WAL；不估计 PITR 归档容量。
- 锁观测以 5ms 间隔轮询目标库日志/ingress 锁；无正样本不等于证明没有短暂锁等待。
- 小数据成功只验收当前小数据假设下的操作流程和 10/30 分钟预算，不外推到大历史库。

真实发布仍需停止全部旧写者，备份实际业务库和所有实例 spool/WAL，并检查文件清单。
如果实际数据量、硬件、积压或运维时间与本次假设不同，应重新验收，不能引用小样本报告
承诺停机时长。迁移提交后不能只回滚二进制；优先 forward-fix。

本演练备份后没有外部请求，因此恢复点之后没有需要对账的真实提供方费用。真实环境如果
已经接流量，旧备份不能代表零数据丢失：停止写入，保留新产生的证据并逐请求对账，
不能恢复旧库/旧 spool 后直接宣称 RPO=0。恢复时应将同一时点的数据库和全部本地证据
作为一个整体；错配、缺失、损坏或未知来源的备份不能放行。

## 相关文档

- [独立计量事实与结算回执](independent-metering.md)
- [持久化边界与验收计划](persistence-boundaries.md)
- [生产部署](../user/production-deployment.md)
