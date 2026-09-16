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

只读检查测试、Mock、数据库、迁移、日志和 CI；未运行 Monoize，未验证其吞吐、
断电恢复或全部 PostgreSQL 行为。目录布局、规范、实现和实际通过的验收必须分别看待。
值得借鉴的是项目级验收、真实客户端闭环、失败证据、持久化准入和数据库后端边界，
不是简单搬目录、换 ORM 或删除耐久日志。重新比较时必须记录新的源码版本。

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

我们已有网络/PG/契约/Streaming/WS 集成测试及 opt-in 真实上游、性能测试。
`web/console/e2e/` 是真实浏览器配模拟 API；另有 ignored 的 Codex fallback 测试。
缺口是客户端矩阵及浏览器到真实 API、数据库、转发、结算的一体化验收。

检查的 Monoize 工作流没有发现根 E2E 的常规 PR 门禁；Docker 场景显式开启。
我们现有路径感知 CI 和稳定 `ci-gate` 应保留。目录显眼不代表保障已经生效。
来源：[容器测试](https://github.com/Ikaleio/monoize/blob/7b5624a36d6bd23162241443e9c1dae89eddebce/e2e/docker.test.ts)、
[工作流](https://github.com/Ikaleio/monoize/tree/7b5624a36d6bd23162241443e9c1dae89eddebce/.github/workflows)。

## Mock 的统一边界

根 `mock/server.ts` 是独立开发上游，E2E 用另一份 `e2e/fixture.ts`；
没有发现全部测试共享一个实现。来源：
[根 Mock](https://github.com/Ikaleio/monoize/blob/7b5624a36d6bd23162241443e9c1dae89eddebce/mock/server.ts)、
[fixture](https://github.com/Ikaleio/monoize/blob/7b5624a36d6bd23162241443e9c1dae89eddebce/e2e/fixture.ts)。

我们应共享场景 ID、wire 样本、预期终态、错误分类与证据格式，
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

我们的 PG 绑定远超日志：`PgPool`、`Transaction<Postgres>`、枚举/数组/JSONB/NUMERIC、
`ANY`/`UNNEST`、JSON 聚合、行锁、迁移 advisory lock 和 PL/pgSQL 业务触发器。
PG 事务也进入部分 application 接口。但运行时快照、转发、终态事件、本地 spool
可以保留。参见[数据库架构](../development/database-architecture.md)。
可移植性的关键是业务事务和后端边界，不是更换 ORM。SQLite 轻量模式仍需精确金额、
幂等结算、恢复和约束保障，不需要模拟 PG 高吞吐实现。

## 日志与计费

Monoize 同样有持久化 spool、事务批量 INSERT、失败重试和 ID 幂等，提交后才删除
spool，不是靠易失队列换可移植性。它在请求开始时预留容量、持久化异常兜底，
失败可以拒绝请求。来源：
[日志准入](https://github.com/Ikaleio/monoize/blob/7b5624a36d6bd23162241443e9c1dae89eddebce/src/handlers/request_logging.rs)、
[spool 与批写](https://github.com/Ikaleio/monoize/blob/7b5624a36d6bd23162241443e9c1dae89eddebce/src/db_cache.rs)。

我们使用 spool → COPY ingress → request_logs → 幂等结算。COPY 入口隔离宽表压力，
但应视作 PG 优化，而不是业务保证的唯一实现。SQLite 可采用串行事务批写。
本次未压测，不能断言现有各阶段全部必要或过度设计。

`request_logs` 兼任查询事实与结算来源；`billed_at` 认领、余额和 Key 额度同事务
是合理的幂等设计，但查询投影、保留策略与结算恢复耦合。应明确计量事实和查询投影
的生命周期，不预设引入消息中间件或重写账本。

普通 `RequestLogSink::try_record` 返回 `()`；终态 append 失败只输出指标/ERROR，
不能撤销已完成上游调用。拼车另有预占 WAL，必须分开审查。
需要明确磁盘满、不可写、DB 长期不可用时的准入，而不是声称“有 spool 就不丢”。

耐久性要区分进程崩溃和断电：Monoize SQLite 的 `synchronous=NORMAL` 存在近期
事务断电丢失窗口，我们默认 spool 也有 10ms group-sync 窗口。
参见 [SQLite 官方说明](https://www.sqlite.org/pragma.html#pragma_synchronous)
和[本项目耐久边界](../development/request-log-durability.md)。

## 改进顺序

1. 第一阶段：[系统 E2E](../development/system-e2e.md)、真实 CLI 工具闭环、
   少量浏览器全栈、共享场景/报告和普通日志故障准入审查。
2. 第二阶段：收拢事务接口，区分控制面、计量结算和查询投影，将 PG 优化留在后端内。
3. 第三阶段：确认轻量部署需求后增加 SQLite，运行相同业务契约；不直接承诺双数据库。

不照搬跨协议转换矩阵：格式隔离是我们的产品约束。
不把手动 E2E 当 CI、不把 fixture 当真实上游，不自动运行付费调用或性能压测。
