# 请求日志故障准入审查

> 状态：已完成设计记录；2026-09-16 源码审查并由隔离系统测试复现部分边界。生产策略仍保持现状，
> 以下建议不是已经实施的 fail-closed 保证。

## 审查边界

对比背景：[Monoize 工程研究](../reference/monoize-engineering-study.md)。
本次检查 `src/application/request_log.rs`、`src/request_log_spool.rs`、
`src/workers/durable_request_log.rs` 及 `src/persistence/mod.rs`。
拼车另有预占 WAL，不能将其 fail-closed 保证外推给普通请求。
参见[日志流水线](request-log-durability.md)和[拼车账本](codex-sharing.md)。

## 当前故障矩阵

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
写失败场景确认普通请求继续 dispatch、终态日志缺失、writer 在去除注入后仍拒绝 append，
直到重启恢复；这是已复现风险，不是 fail-closed 成功验收。仍不宣称覆盖上表所有故障。

## 需要明确的生产策略

建议目标：普通计费请求在上游 dispatch 前确认日志设施健康并预留有界终态容量；
不可写或容量不足时拒绝新请求，而不是等上游已完成后才发现无法记账。
必须在独立设计/实现中回答：

1. spool 的字节预算、最小保留空间和健康恢复迟滞如何定义？可用空间检查不是写入保证。
2. 预占和异常兜底是否要求同步耐久？业务可接受的延迟/断电窗口是多少？
3. SSE/WS 每个逻辑请求的预占、正常结束、取消与进程中断如何释放或恢复？
4. DB 故障但 spool 仍有预算时是否继续服务？不能把 DB 一次超时直接当全局不可用。
5. 已 dispatch 的请求 append 失败如何保持 pending/待核对？
   不得为了闭环而伪造 usage、静默记零或回退成无计量调用。
6. 与拼车既有 WAL、普通 soft quota、价格快照和重复恢复如何保持一致？

不要简单将 `try_record` 改为返回错误后，就宣称已解决准入：
响应可能已发送，不能撤销上游调用。也不要通过禁用或删除 durable spool 支持 SQLite。

## 后续验证清单

- 在独立目录/容器限制容量，禁止填满宿主磁盘。
- 验证不可写时新请求未 dispatch，已预占请求仍能正确结束或留下可恢复事实。
- 注入终态前后、DB 提交前后、checkpoint 前后的中断，验证不重复扣费。
- 保留未知 usage 的 fail-closed/pending 边界，与失败零费用规则区分。
- 对生产转发/准入的任何实现变化，另行运行 Rust 门禁并取得付费 smoke 授权。
