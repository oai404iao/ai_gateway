# ai-gateway 文档中心

> 状态：当前索引。

本目录按读者和用途分组。开始阅读前，先选择与你当前任务对应的入口。

## 文档分类

| 分类 | 面向读者 | 内容 |
| --- | --- | --- |
| [用户文档](user/README.md) | 部署者、运维人员、API 使用者 | 启动、配置、部署、数据面和 Console 使用说明 |
| [开发文档](development/README.md) | 后端、前端、测试和发布维护者 | 当前架构、设计约束、测试、性能和发布流程 |
| [外部参考](reference/README.md) | 兼容性开发者、上游接入人员 | OpenAI 接口语义、网关兼容边界和权威外部链接 |
| [OpenAPI 契约](openapi/console-v1.yaml) | Console 后端与前端开发者 | Console API 请求/响应的机器可读权威规范 |
| [请求白名单契约](reference/request-allowlists.json) | 数据面与 Connector 维护者 | 客户端/Codex Header、顶层 body 字段动作和 Codex 隐私归一化/安全补全 |
| [历史归档](archive/README.md) | 追溯历史决策的维护者 | 已完成 MVP 清单和已被替代的早期蓝图/实施计划；不能作为当前行为依据 |

仓库根目录的 [`AGENTS.md`](../AGENTS.md) 是编码 Agent 的操作手册，不是用户文档或架构文档。

处理 GitHub 安全告警时，参见[安全告警核查与处置](development/security-alert-triage.md)，
区分真实修复、测试用途和误报，并保留逐条证据。

配置专用 Codex 共享凭证时，参见[拼车使用说明](user/codex-sharing.md)和
[金额账本实现](development/codex-sharing.md)。该功能仅支持单实例。

上游实体重构计划见[身份、渠道能力与路由目标设计](development/upstream-identity-capabilities.md)；
其中第一阶段独立凭证身份已实现并通过验收；能力与路由改为联合切换，
新拓扑转存、历史引用迁移和快照编译已具备隔离测试覆盖，
Codex 生命周期、新拓扑管理接口/界面和服务加载入口已接通，
但启动迁移、授权写入与旧 Console 入口退役尚未闭环，任务分支不可发布。
当前管理边界见[上游凭证管理](user/upstream-credentials.md)。

## 来源优先级

项目级验收入口见[系统 E2E](development/system-e2e.md)；
工程对比背景见 [Monoize 源码研究](reference/monoize-engineering-study.md)。
第二阶段设计见[持久化边界与独立计量事实](development/persistence-boundaries.md)；
当前接口见[持久化操作接口](development/persistence-interfaces.md)，
独立财务来源与升级边界见[独立计量事实](development/independent-metering.md)，
成对备份与恢复入口见[0063 演练](development/persistence-rehearsal.md)。
第三阶段见 [SQLite 双后端实施](development/sqlite-backend.md)；
目前已实现 schema/生命周期、[身份与控制面](development/sqlite-control-plane.md)
以及[计量与结算](development/sqlite-metering.md)、[Codex 与拼车](development/sqlite-codex.md)，
Linux SQLite 的配置、停机备份与恢复见[部署指南](user/sqlite.md)。

当文档之间出现差异时，按以下优先级判断：

1. 当前实现、测试、migration、配置反序列化类型。
2. `docs/openapi/console-v1.yaml`、`docs/reference/request-allowlists.json` 等机器可读契约。
3. 标记为“当前”的用户文档和开发文档。
4. 设计提案、产品蓝图和外部参考。
5. `docs/archive/` 中的历史材料。

外部 API 会持续变化。`docs/reference/` 只记录本项目需要依赖的语义和检查日期，不复制完整第三方文档。

## 维护规范

新增、移动或修改文档前，请阅读 [文档规范](documentation-standard.md)。文档变更至少需要：

```bash
git diff --check
python3 scripts/check-docs.py
```

同时确认所有相对链接、命令、文件路径和状态说明仍然有效。
