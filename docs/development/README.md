# 开发文档

> 状态：当前索引。面向后端、前端、测试、运维工具和发布维护者。

## 当前架构与设计

- [当前架构](architecture.md)：运行拓扑、请求链路、模块边界和来源优先级。
- [Codex OAuth Connector 设计记录](codex-oauth-connector.md)：进程内 Connector、
  managed channel、凭证快照、quota 与粘性边界。
- [MCP 服务架构与扩展边界](mcp-services.md)：已实现的无状态 Search 与 Images
  generation/edit transport、多 MCP registry、API Key 鉴权和后续非目标。
- [OpenAI Images 转发设计与分阶段实施](openai-images.md)：格式/操作拆分、generation/edit
  转发、Codex 凭证共享投影，以及 replayable 大 body 边界。
- [数据库与控制面架构](database-architecture.md)：当前持久化实体组、快照编译和 schema
  修改流程；逐列结构以 migration 为准。
- [数据库后端抽象与 SQLite 完成总计划](database-backend-completion-plan.md)：**提案**；
  完整数据库 facade、生产级 SQLite 功能等价与分阶段实施的控制性路线图，不代表当前可用能力。
- [Console 认证与授权设计记录](console-auth.md)
- [Console Web UI 架构与开发指南](console-ui.md)：当前 Base UI 技术栈、会话、安全、构建和测试。
- [请求日志耐久化流水线](request-log-durability.md)
- [统计功能设计](statistics.md)
- [Transform DSL](transform-dsl.md)

早期的产品蓝图、11 表数据库方案和 Console UI 分阶段计划已移入
[`docs/archive/`](../archive/README.md)，只用于追溯。

## 测试、性能与发布

- [持续集成与安全扫描](continuous-integration.md)：路径感知门禁、稳定
  `ci-gate`、cache 写入边界、Playwright、CodeQL 与默认分支 ruleset。
- [Rust 工具链与 MSRV 策略](rust-toolchain-policy.md)：区分默认构建工具链与
  最低支持版本，并定义兼容窗口和升级门禁。
- [真实上游 smoke test](real-upstream-smoke.md)：付费、显式执行的转发验证。
- [转发性能测试](forwarding-performance.md)：隔离的手动性能 Harness。
- [版本发布流程](releasing.md)

## 开发入口

编码 Agent 应先阅读仓库根目录 [`AGENTS.md`](../../AGENTS.md)。普通贡献者也可使用其中的命令和变更工作流，但产品使用方式应以 [`docs/user/`](../user/README.md) 为准。

Console API 契约变更必须从 [`docs/openapi/console-v1.yaml`](../openapi/console-v1.yaml) 开始，并重新生成前端类型。
