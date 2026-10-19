# 维护者文档

> 状态：当前索引。面向后端、前端、测试和发布维护者。

首次参与开发，先[设置环境](getting-started.md)，再阅读[当前架构](architecture.md)。
部署和调用步骤在[用户指南](../user/README.md)，无需先读完所有设计记录。

## 修改转发与连接器

| 任务 | 阅读 |
| --- | --- |
| 理解请求顺序、快照和模块职责 | [架构](architecture.md) |
| 修改上游身份、授权、能力与操作路由 | [上游实体与路由设计](upstream-identity-capabilities.md) |
| 实现或升级原生插件 | [插件边界](connector-plugins.md)、[SDK](../../crates/connector-sdk/README.md)、[命令契约](../../crates/connector-sdk/docs/commands.md) |
| 修改 Codex 生命周期与宿主适配 | [Codex 设计记录](codex-oauth-connector.md) |
| 修改 Images generation/edit | [Images 转发](openai-images.md) |
| 实现同格式 JSON/SSE 响应适配 | [响应适配 SDK](connector-response-adapters.md) |
| 修改用量解析与缓存计数 | [usage 规范化](usage-normalization.md) |
| 修改受限变换 | [Transform DSL](transform-dsl.md) |

公共请求策略以[请求策略 JSON](../reference/request-allowlists.json)为准；供应商特有隐私和
body/Header 策略归外部插件。转发变更需 deterministic tests 和经明确授权的真实上游验证，
见[测试入口](getting-started.md#选择验证层级)。

## 修改 Console

- [认证与授权](console-auth.md)：JWT、会话、角色与归属边界。
- [前端架构](console-ui.md)与[前端命令](../../web/console/README.md)：开发、构建和嵌入式交付。
- [导航与页面交互](console-interaction-standard.md)：列表状态、返回、草稿和历史入口。
- [统计口径](statistics.md)：个人与系统范围、费用与监控定义。

请求/响应形状先修改 [OpenAPI](../openapi/console-v1.yaml)，再生成前端类型并更新测试。
不要从设计记录或生成类型反向定义契约。

## 修改持久化与恢复

- [数据库与控制面](database-architecture.md)：实体、事务、迁移与快照发布。
- [软删除](soft-deletion.md)：墓碑、解绑、历史查询与不可恢复边界。
- [持久化行为契约](persistence-contracts.md)和[操作接口](persistence-interfaces.md)：金额、原子性、错误与仓储边界。
- [耐久请求流水线](request-log-durability.md)：spool、入口、独立投影与结算。
- [独立计量事实](independent-metering.md)：财务事实、pending 集合和唯一结算回执。
- [日志故障准入](request-log-admission-review.md)：故障矩阵与准入策略。
- [成对备份恢复演练](persistence-rehearsal.md)：0063 演练的步骤、测量口径与限制。
- [持久化边界设计记录](persistence-boundaries.md)：拆分动机与切换约束。

### SQLite 专题

部署请直接阅读[用户指南](../user/sqlite.md)。修改实现时按职责阅读：

| 职责 | 文档 |
| --- | --- |
| 整体范围与双后端决策 | [SQLite 后端](sqlite-backend.md) |
| 文件、lease、身份、迁移与恢复 | [生命周期](sqlite-lifecycle.md) |
| 类型编码、约束与索引 | [schema 映射](sqlite-schema-mapping.md) |
| 认证、管理与快照 | [身份与控制面](sqlite-control-plane.md) |
| 计量、结算与查询 | [耐久计量](sqlite-metering.md) |
| OAuth、外部副作用与不确定结果 | [Codex 仓储](sqlite-codex.md) |

## 验证、排障与发布

- [系统 E2E](system-e2e.md)：真实浏览器、CLI、隔离网关和双数据库场景。
- [真实上游 smoke](real-upstream-smoke.md)：显式授权、付费外部调用。
- [转发性能测试](forwarding-performance.md)：仅按需执行的隔离负载工具。
- [CI 与安全扫描](continuous-integration.md)：路径选择、`ci-gate` 和 cache 写入限制。
- [安全告警处置](security-alert-triage.md)：逐条证据、历史核查与重新评估条件。
- [Rust 工具链](rust-toolchain-policy.md)：固定版本及协调升级。
- [版本发布](releasing.md)：验证、产物、tag 与发布后检查。

## 维护仓库与文档

编码 Agent 另需遵守 [`AGENTS.md`](../../AGENTS.md)。

文档变更阅读[编写规范](../documentation-standard.md)；组织和写法的依据见
[Pi 文档研究](documentation-design.md)。[历史归档](../archive/README.md)只用于追溯，
不要把过去的实施切片、验收记录或退出条件当成当前部署状态。
