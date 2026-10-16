# 请求日志故障准入决策

> 状态：已完成设计记录。2026-09-16 的审查促成派发前同步准入；
> 当前格式、容量与恢复操作见[日志耐久化](request-log-durability.md)。

这项决策解决的是“上游已经派发，却没有可靠本地终态落点”的风险。
仅在终态 append 失败时返回错误已经太晚：客户端可能收到响应，上游调用也无法撤销。
因此客户端逻辑请求必须先同步 intent、预留终态 slot，再允许派发。

## 目录

- [选定策略](#选定策略)
- [未知结果的取舍](#未知结果的取舍)
- [为什么原流水线不够](#为什么原流水线不够)
- [证据与剩余边界](#证据与剩余边界)

## 选定策略

1. 完成选路后、Connector 准备前，为每个客户端逻辑请求分配最大终态 slot 并同步 intent。
   HTTP 重试复用预占，每个 WS response.create 单独准入。
2. 默认本地预算 1GiB、最小剩余空间 64MiB。容量不足返回 503，不派发；
   不把拒绝伪造为取消终态。路由前拒绝和 scheduled probe 不属于此范围。
3. 终态先同步 slot，再同步 events.log，最后删除预占。
   append 失败不撤回已发响应；完整 slot 用原 UUID 在重启后恢复。
4. DB 不可用但本地容量允许时继续服务，请求路径不查询数据库。
5. 容量压力可提前压缩已排空 spool；写/同步故障则锁定 writer，使用原目录重启恢复，
   不通过盲写探测、删除证据或更换目录绕过。
6. 不改变计费公式、价格快照或 UUID 幂等结算。财务事实和回执另见
   [独立计量](independent-metering.md)。

不能仅把 try_record 改为返回错误就声称已完成准入；
也不能为了 SQLite 删除或禁用 durable spool。

## 未知结果的取舍

所选策略是**只保留待核对，不因未知费用冻结普通用户或全局服务**：

- 只有 intent、无完整可信终态时，不制造 failed/cancelled，不记零，不伪造 usage。
- intent 不证明已派发；同步与真正 dispatch 之间仍有窗口。
- 重启释放未知 slot 的闲置预分配空间，保留实际证据和告警；
  设施健康且预算允许时，新请求继续。
- 人工核对前仍存在未知费用风险。这不是普通用户金额 fail-closed，也不是费用已经结清。
- 自动核对、补账 API 和 Console 待核对页尚未提供。

## 为什么原流水线不够

以下是**改动前**的故障矩阵，不是当前运行时描述：

| 故障 | 原处理及局限 |
| --- | --- |
| 通知队列满 | append 后合并通知，事件仍在 spool，队列满本身不丢日志 |
| DB/COPY 故障 | 不推进 checkpoint，保留 spool；本地空间仍可能耗尽 |
| 查询投影失败 | 保留 ingress；当时结算依赖查询表，会一起落后 |
| 结算失败 | billed_at 认领与账户回滚；现已由独立回执替代 |
| spool append 失败 | 只增指标/ERROR，sink 返回 ()，普通请求可能缺结算证据 |
| 终态前进程退出 | 事件尚未生成，已有 append 的恢复保证无法覆盖 |
| group sync 前断电 | 默认 10ms 窗口不保证逐条掉电耐久 |
| worker 停止而 append 成功 | 数据可重启恢复，不等于立即可查询 |

后续独立事实切换解决了日志投影与扣款权的逻辑耦合；
它不替代派发前本地准入，也不隔离共享数据库的硬件资源。

## 证据与剩余边界

代码入口：application/request_log.rs、request_log_admission.rs、
request_log_spool.rs、workers/durable_request_log.rs。

- spool 单测覆盖预占/终态同步、CRC/撕裂 slot、未知 intent、容量拒绝、
  压力压缩、checkpoint/reopen 与故障锁定。
- HTTP 各操作及每次 WS create 的确定性测试断言：准入拒绝前没有上游调用。
- [系统 E2E](system-e2e.md)覆盖 DB 暂停后 kill/restart、checkpoint 回退无双扣、
  局部 ENOSPC/EACCES、终态 append 失败后 slot 重放、终态前 kill 及 sync EIO。
  注入不填满宿主磁盘。
- 原故障复现确认“继续派发但无终态、去除注入仍锁定”的风险；
  当前测试已改为拒绝新派发并恢复完整 slot，不保留危险旧预期。
- 2026-09-16 的 Rust 与获授权上游回归仅是对应实现版本的历史证据。
  后续转发变化仍需按仓库规范验证，不继承历史付费授权。

这些测试不覆盖所有部分写/断电组合，不保证设备损坏或主机永久丢失后无损；
没有性能基准结论。工程对比背景见[Monoize 研究](../reference/monoize-engineering-study.md)。
