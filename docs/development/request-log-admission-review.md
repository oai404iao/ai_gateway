# 请求日志故障准入审查

> 状态：已完成设计记录；2026-09-16 审查、故障复现后已实施生产日志准入。
> 当前实现细节及运维恢复以[日志流水线](request-log-durability.md)为准。

## 审查边界

对比背景：[Monoize 工程研究](../reference/monoize-engineering-study.md)。
本次检查 `src/application/request_log.rs`、`src/request_log_spool.rs`、
`src/workers/durable_request_log.rs` 及 `src/persistence/mod.rs`。
拼车另有预占 WAL，不能将其 fail-closed 保证外推给普通请求。
参见[日志流水线](request-log-durability.md)和[拼车账本](codex-sharing.md)。

## 改动前的故障矩阵

| 故障 | 当前代码行为 | 后果/恢复边界 |
| --- | --- | --- |
| 通知队列满 | durable append 后合并通知 | 事件仍在 spool，不等于日志丢失 |
| DB/COPY 暂时失败 | 保留 spool，不推进成功 checkpoint | 可重放；磁盘积压可能继续增长 |
| 投影失败 | 保留 ingress 行，延迟重试 | 查询和依赖最终表的结算落后 |
| 结算事务失败 | claim 与账户更新一同回滚 | 由 `billed_at IS NULL` 恢复 |
| spool append 失败 | 增加指标、ERROR，sink 返回 `()` | 普通请求没有持久化确认回传；事件可能没有结算来源 |
| 请求终态前进程被杀 | 普通日志只在终态构造/append | 不能把已完成 write 的恢复保证外推给尚未产生的事件 |
| group sync 前主机断电 | 默认 10ms 同步窗口 | 不保证逐条断电耐久 |
| worker 停止但 append 成功 | 保留 spool 并输出告警 | 可在重启后恢复，不代表马上可查询 |

现有自动化依据：

- `src/request_log_spool.rs`：checkpoint 重放、截断尾帧修复、单写者锁、压缩后继续 append。
- `tests/control_plane_integration.rs`：
  `durable_request_log_pipeline_replays_spool_after_an_ingress_outage`、
  `settlement_claim_is_concurrent_idempotent_and_allows_soft_quota_overdraft`、
  `batch_settlement_aggregates_account_updates_and_deduplicates_ids`。

这些测试不等价于真实主机断电、磁盘耗尽或所有请求阶段的 kill 注入。
[系统 E2E](system-e2e.md) 还验证 DB 暂停后的 kill/restart 恢复、
checkpoint 回退重复重放不重复扣费，以及真实 Gateway 的局部 ENOSPC/EACCES syscall 注入。
改动前曾确认普通请求继续 dispatch、终态日志缺失、writer 在去除注入后仍拒绝 append。
改动后的场景已改为断言拒绝新派发，并验证完整 slot 重放，不保留旧的危险预期。

## 已选生产策略

经用户确认，选择逐请求同步，但未知费用只保留待核对，不冻结相关用户或全局服务：

1. 每个客户端逻辑请求在 Connector 准备前预分配最大终态 slot 并同步 intent。
   HTTP 重试复用预占；WS 每个 `response.create` 独立准入。
2. 默认本地预算 1GiB、最小剩余空间 64MiB；容量不足返回 503。
   已排空文件可因容量压力提前压缩，写/同步错误则锁定到重启，不作盲目写入探测。
3. 终态先同步 slot，再同步 events.log，然后删除预占。终态写日志失败不撤销已发出的响应；
   完整 slot 在重启后使用原 UUID 重放，未知记录不投影为零费用失败/取消。
4. DB 不可用但本地容量允许时继续服务；不在请求路径查询 PostgreSQL。
5. 未知请求在重启后释放闲置预分配空间，只保留证据和待核对告警。
   新请求可以继续；**人工核对前存在未知费用风险**，这不是普通用户金额 fail-closed。
6. 不修改现有计费公式、价格快照终态、UUID 幂等结算或拼车 WAL/金额准入。
   新日志准入也覆盖拼车客户端请求，但不代替其独立的资金预占。

不要简单将 `try_record` 改为返回错误后，就宣称已解决准入：
响应可能已发送，不能撤销上游调用。也不要通过禁用或删除 durable spool 支持 SQLite。

## 已验证与剩余边界

- Rust 单测覆盖预占/终态同步、slot 恢复、容量压力压缩、剩余空间拒绝和未知记录反复恢复。
- HTTP 所有操作及每次 WS create 的确定性测试验证拒绝前无上游调用。
- 系统 E2E 验证派发前 ENOSPC/EACCES、终态追加失败重放、终态前 kill 留待核对、
  sync EIO、DB 停止后的 kill/restart 和 checkpoint 回退不重复扣费；不会填满宿主磁盘。
- Rust 门禁与获授权的真实上游 smoke 已执行，后续转发改动仍必须按仓库规范重跑。
- 不宣称覆盖所有部分写/断电组合；没有运行性能基准。
- 自动核对、补账 API 和 Console 待核对页面仍未实现，未知费用不会自动扣除。
