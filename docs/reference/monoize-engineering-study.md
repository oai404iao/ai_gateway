# Monoize 工程实践对比

> 状态：已完成设计记录；外部源码研究，不是 ai-gateway 的实现承诺。
>
> 类型：外部参考
>
> 最近核对：2026-09-16
>
> 权威来源：[Ikaleio/monoize](https://github.com/Ikaleio/monoize/tree/7b5624a36d6bd23162241443e9c1dae89eddebce)，
> 固定提交 `7b5624a36d6bd23162241443e9c1dae89eddebce`。
> 本项目对比基线：`bb3a65ef6124d79445d314c4e0487bae6cb8ba1b`。

## 范围和结论

本文回答“哪些工程方法值得借鉴”，不是两项目的实时功能排行榜。
研究只读检查固定提交中的测试、Mock、数据库、迁移、日志和 CI；未运行 Monoize，
未验证吞吐、断电恢复或全部 PostgreSQL 行为。目录、规范、实现和实际验收分别看待。

| 要研究的问题 | 本文结论 | 本项目当前入口 |
| --- | --- | --- |
| 如何证明客户端真能用 | 验证工具调用闭环与 wire 证据，不只看 200/退出码 | [系统 E2E](../development/system-e2e.md) |
| 是否统一所有 Mock | 共享场景和证据，不强迫共享执行器 | [系统 E2E 场景](../development/system-e2e.md) |
| 如何支持双数据库 | 隔离业务事务和后端优化，不靠换 ORM | [数据库架构](../development/database-architecture.md)、[SQLite](../development/sqlite-backend.md) |
| 是否能简化耐久管线 | 先定义财务事实和恢复保证，不能用易失队列代替 | [独立计量](../development/independent-metering.md) |

下文“研究时本项目”均指页首的 `bb3a65e` 基线，不能当作今天的缺口清单。
当前实现沿右列入口检查；重新研究外部项目时记录新提交和实际执行范围。

## 项目级 E2E

其根 E2E 主要不是浏览器测试，而是：
真实 Agent CLI → 下游 recorder → 网关容器 → 上游 recorder → fixture 或真实上游。
每场景隔离容器、网络、SQLite 和客户端目录，包含 Codex、Claude、OpenCode、Pi。
验收要求工具调用、成功执行、对应工具结果及后续生成闭环；
Codex WS 还要求真实 WS、`previous_response_id` 延续及禁止 HTTP fallback。
200、零退出码或最终文字不够；内置任务的文件证据也不证明生成内容语义正确。

来源：[编排](https://github.com/Ikaleio/monoize/blob/7b5624a36d6bd23162241443e9c1dae89eddebce/e2e/runner.ts)、
[验证](https://github.com/Ikaleio/monoize/blob/7b5624a36d6bd23162241443e9c1dae89eddebce/e2e/validate.ts)、
[规范](https://github.com/Ikaleio/monoize/blob/7b5624a36d6bd23162241443e9c1dae89eddebce/spec/protocol-e2e.spec.md)。

研究时本项目已有网络/PG/契约/Streaming/WS 集成测试、opt-in 真实上游及性能工具，
`web/console/e2e/` 使用模拟 API，另有 ignored Codex fallback 测试；当时识别的缺口是
真实客户端矩阵与浏览器到 API、数据库、转发、结算的一体化验收。
这解释了后续[系统 E2E](../development/system-e2e.md)的动机，不表示当前仍没有该套件。

检查的 Monoize 工作流没有发现根 E2E 的常规 PR 门禁；Docker 场景显式开启。
路径感知 CI 和稳定 `ci-gate` 不能被手动工具替代。目录显眼不代表保障已经生效。
来源：[容器测试](https://github.com/Ikaleio/monoize/blob/7b5624a36d6bd23162241443e9c1dae89eddebce/e2e/docker.test.ts)、
[工作流](https://github.com/Ikaleio/monoize/tree/7b5624a36d6bd23162241443e9c1dae89eddebce/.github/workflows)。

## Mock 的统一边界

根 `mock/server.ts` 是独立开发上游，E2E 用另一份 `e2e/fixture.ts`；
没有发现全部测试共享一个实现。来源：
[根 Mock](https://github.com/Ikaleio/monoize/blob/7b5624a36d6bd23162241443e9c1dae89eddebce/mock/server.ts)、
[fixture](https://github.com/Ikaleio/monoize/blob/7b5624a36d6bd23162241443e9c1dae89eddebce/e2e/fixture.ts)。

可迁移的方法是共享场景 ID、wire 样本、预期终态、错误分类与证据格式，
不强制 MSW、Playwright、Rust 故障注入和性能 Mock 使用相同执行器。
Console Mock 负责页面状态，协议 Mock 负责 wire contract，故障 Mock 负责分片/断连，
性能 Mock 负责低开销。共享样本仍需独立负向测试，避免实现与 oracle 同源掩盖错误。

## 双数据库支持

SeaORM/SQLx 双驱动加自有 `DbPool`：SQLite 单写连接、多读连接、WAL、
进程内写 mutex；PG 使用连接池及必要行锁。类型偏向 TEXT/INTEGER，
降低 UUID、时间、金额、JSON 的差异。占位符、迁移和金额聚合仍有方言分支。
写 mutex 不是跨进程协调，根 E2E 使用 SQLite；此次未找到双后端 CI 行为矩阵。

来源：[连接与事务](https://github.com/Ikaleio/monoize/blob/7b5624a36d6bd23162241443e9c1dae89eddebce/src/db/mod.rs)、
[schema](https://github.com/Ikaleio/monoize/blob/7b5624a36d6bd23162241443e9c1dae89eddebce/src/migration/m20250101_000001_create_tables.rs)、
[金额聚合](https://github.com/Ikaleio/monoize/blob/7b5624a36d6bd23162241443e9c1dae89eddebce/src/users/request_logs.rs)。

研究时本项目的 PG 绑定远超日志：`PgPool`、`Transaction<Postgres>`、枚举/数组/JSONB/NUMERIC、
`ANY`/`UNNEST`、JSON 聚合、行锁、迁移 advisory lock 和 PL/pgSQL 业务触发器。
PG 事务也进入部分 application 接口；当时判断运行时快照、转发、终态事件和本地 spool
可以保留。由此得到的原则是隔离业务事务与后端实现，不是简单更换 ORM。
SQLite 仍需精确金额、幂等结算、恢复和约束，不必模拟 PG 的 COPY 高吞吐实现。

本项目现在已提供 Linux `sqlite-backend`，当前后端边界和验收要求分别见
[持久化接口](../development/persistence-interfaces.md)、[SQLite 实施](../development/sqlite-backend.md)
与[部署指南](../user/sqlite.md)。这不是本次对 Monoize 双后端质量的新判断。

## 日志与计费

Monoize 同样有持久化 spool、事务批量 INSERT、失败重试和 ID 幂等，提交后才删除
spool，不是靠易失队列换可移植性。它在请求开始时预留容量、持久化异常兜底，
失败可以拒绝请求。来源：
[日志准入](https://github.com/Ikaleio/monoize/blob/7b5624a36d6bd23162241443e9c1dae89eddebce/src/handlers/request_logging.rs)、
[spool 与批写](https://github.com/Ikaleio/monoize/blob/7b5624a36d6bd23162241443e9c1dae89eddebce/src/db_cache.rs)。

研究基线使用 spool → COPY ingress → request_logs → 幂等结算。COPY 隔离宽表压力，
但属于 PG 优化，不是业务保证的唯一实现；SQLite 可用串行事务批写。
本次未压测，不能断言所有阶段必要或过度设计。

基线的 `request_logs` 兼任查询事实和结算来源；`billed_at` 认领、余额和 Key 额度同事务
虽提供幂等性，但耦合了查询保留与财务恢复。这是历史问题描述：当前由不可变
`request_metering_facts` 和独立 settlement receipt 承担财务来源，查询投影可独立推进。
不能按旧图把 request_logs 重新变成结算前置条件。详见[独立计量](../development/independent-metering.md)。

研究时普通 `RequestLogSink::try_record` 返回 `()`，终态 append 失败只输出指标/ERROR，
无法撤销已完成上游调用。后续宿主增加独立 `admit` 端口、派发前同步 intent、
预分配终态 slot 和恢复；未知费用选择只保留待核对，不冻结用户。

耐久性要区分进程崩溃和断电：Monoize SQLite 的 `synchronous=NORMAL` 存在近期
事务断电丢失窗口；本项目已准入的客户端请求逐条同步，
路由前拒绝和 scheduled probe 等未准入日志仍有默认 10ms group-sync 窗口。
参见 [SQLite 官方说明](https://www.sqlite.org/pragma.html#pragma_synchronous)
和[本项目耐久边界](../development/request-log-durability.md)。

## 改进顺序

原研究建议先补项目级证据，再分离持久化边界，最后用相同业务契约支持轻量数据库。
它是决策顺序，不再作为实施进度看板：

- 验收范围与命令：[系统 E2E](../development/system-e2e.md)。
- 事务、事实和查询投影：[持久化边界](../development/persistence-boundaries.md)、
  [独立计量](../development/independent-metering.md)。
- 当前双后端能力：[SQLite](../development/sqlite-backend.md)。

不照搬跨协议转换矩阵：格式隔离是我们的产品约束。
不把手动 E2E 当 CI、不把 fixture 当真实上游，不自动运行付费调用或性能压测。
