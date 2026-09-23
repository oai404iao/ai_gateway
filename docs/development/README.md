# 开发文档

> 状态：当前索引。面向后端、前端、测试、运维工具和发布维护者。

## 当前架构与设计

- [当前架构](architecture.md)：运行拓扑、请求链路、模块边界和来源优先级。
- [Codex OAuth Connector 设计记录](codex-oauth-connector.md)：进程内 Connector、
  managed channel、凭证快照、quota 与粘性边界。
- [OpenAI Images 转发设计与分阶段实施](openai-images.md)：格式/操作拆分、generation/edit
  转发、Codex 凭证共享投影，以及 replayable 大 body 边界。
- [数据库与控制面架构](database-architecture.md)：当前持久化实体组、快照编译和 schema
  修改流程；逐列结构以 migration 为准。
- [控制面软删除](soft-deletion.md)：不可恢复墓碑语义、自动解绑边界和三阶段实施记录。
- [Console 认证与授权设计记录](console-auth.md)
- [Console Web UI 架构与开发指南](console-ui.md)：当前 Base UI 技术栈、会话、安全、构建和测试。
- [Console 导航与页面交互规范](console-interaction-standard.md)：父级返回、列表、详情布局、草稿和历史入口。
- [请求日志耐久化流水线](request-log-durability.md)
- [持久化行为契约基线](persistence-contracts.md)：金额、结算原子性、重放、约束测试和费用读写路径；
  明确独立计量事实实施前的已知限制。
- [持久化操作接口](persistence-interfaces.md)：P2 的专属事务操作、窄仓储句柄、PG 错误与池观测边界。
- [SQLite 双后端实施](sqlite-backend.md)：第三阶段范围、开发基础、分阶段契约与生产启用门槛；尚不可部署。
- [SQLite 文件与迁移生命周期](sqlite-lifecycle.md)：协作式进程 lease、数据库身份、原子迁移与故障验收。
- [SQLite schema 约束映射](sqlite-schema-mapping.md)：0063 后完整业务 baseline、数据库保护、类型编码与归一化写入契约。
- [SQLite 身份与控制面](sqlite-control-plane.md)：S3 后端分派、完整认证/管理操作与双后端契约；生产配置仍关闭。
- [SQLite 耐久计量、结算与查询](sqlite-metering.md)：S4 原子财务链路、精确聚合、独立查询投影和取消/崩溃恢复。
- [SQLite Codex 与拼车](sqlite-codex.md)：S5 OAuth/配额/成对投影、外部调用 fencing、账本所有权与 WAL 恢复。
- [独立计量事实与结算回执](independent-metering.md)：P3 的财务来源、pending 工作集合、独立投影和 0063 停机切换。
- [0063 升级与成对备份恢复演练](persistence-rehearsal.md)：P4 的手动小数据量演练、PG dump/restore、spool/WAL 配对及 10/30 分钟预算。
- [Codex 拼车金额账本](codex-sharing.md)：固定席位、窗口准入、WAL 恢复与单实例边界。
- [统计功能设计](statistics.md)
- [Transform DSL](transform-dsl.md)

早期的产品蓝图、11 表数据库方案和 Console UI 分阶段计划已移入
[`docs/archive/`](../archive/README.md)，只用于追溯。

## 设计提案

- [上游身份、渠道能力与路由目标重构](upstream-identity-capabilities.md)：三阶段设计，
  独立凭证身份优先实施；协议转换不在范围内。
- [持久化边界与独立计量事实](persistence-boundaries.md)：第二阶段的存储职责、金额与恢复契约、
  PG 封装，以及独立计量事实/结算回执的迁移和验收计划；P4 提供小数据量演练，实际发布容量须匹配部署条件。

## 测试、性能与发布

- [系统 E2E](system-e2e.md)：真实浏览器、CLI、Gateway、临时 PostgreSQL 与共享协议场景。
- [日志故障准入审查](request-log-admission-review.md)：现状、风险、决策边界与后续故障验证。

- [持续集成与安全扫描](continuous-integration.md)：路径感知门禁、稳定
  `ci-gate`、cache 写入边界、Playwright、CodeQL 与默认分支 ruleset。
- [安全告警核查与处置](security-alert-triage.md)：逐条 CodeQL 核查证据、测试用途与误报边界，
  以及必须重新评估的条件。
- [Rust 单工具链策略](rust-toolchain-policy.md)：固定开发、CI、Release 和容器
  构建使用的同一版本，并定义协调升级门禁。
- [真实上游 smoke test](real-upstream-smoke.md)：付费、显式执行的转发验证。
- [转发性能测试](forwarding-performance.md)：隔离的手动性能 Harness。
- [版本发布流程](releasing.md)

## 开发入口

编码 Agent 应先阅读仓库根目录 [`AGENTS.md`](../../AGENTS.md)。普通贡献者也可使用其中的命令和变更工作流，但产品使用方式应以 [`docs/user/`](../user/README.md) 为准。

Console API 契约变更必须从 [`docs/openapi/console-v1.yaml`](../openapi/console-v1.yaml) 开始，并重新生成前端类型。
