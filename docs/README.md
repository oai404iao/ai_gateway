# ai-gateway 文档

> 状态：当前索引。

`ai-gateway` 把 OpenAI 兼容请求路由到已配置的上游，提供独立的管理 Console，
并异步持久化用量和费用。第一次使用，从[快速上手](user/quickstart.md)完成第一条请求；
已有运行环境，直接选择下面的任务。

项目概览：[English](../README.md) / [简体中文](../README.zh-CN.md)。

## 启动、配置与部署

- [快速上手](user/quickstart.md)：本地启动、Console、首个管理员、路由与请求。
- [生产部署](user/production-deployment.md)：发行镜像、Compose、密钥、TLS 与升级。
- [配置与容量](user/production-configuration.md)：TOML、运行时设置、连接池、spool 和观测。
- [SQLite 部署与恢复](user/sqlite.md)：Linux 单实例文件后端及停机成对备份。
- [安全运行](user/security.md)：访问边界、敏感数据、插件信任与恢复限制。

## 使用 API 与管理上游

- [使用与运维](user/operations.md)：按任务查找数据面、Console、路由、日志和计费说明。
- [管理上游凭证](user/upstream-credentials.md)：共享凭证、目标范围与轮换。
- [安装连接器插件](user/connector-plugins.md)：独立安装、显式启用和升级 Codex 插件。
- [查阅兼容性](reference/README.md)：支持的接口、请求策略和外部语义。

## 理解实现与参与开发

- [设置开发环境](development/getting-started.md)，选择对应的检查层级。
- [理解当前架构](development/architecture.md)：监听器、转发、快照和后台流水线。
- [查找维护者专题](development/README.md)：连接器、Console、持久化、测试、CI 与发布。
- [开发 Console UI](../web/console/README.md) 或 [实现原生连接器](../crates/connector-sdk/README.md)。
- [Console 视觉规范](../design.md)：Nova 主题、图标、动画与布局原语边界。

## 查阅契约与历史

| 查什么 | 权威入口 |
| --- | --- |
| Console 请求、响应与并发写入契约 | [Console OpenAPI](openapi/console-v1.yaml) |
| 公共请求 Header、顶层字段与通用出站策略 | [请求策略 JSON](reference/request-allowlists.json)及[解释](reference/request-allowlists.md) |
| 进程配置字段与默认值 | [配置模板](../config.example.toml)、[反序列化实现](../src/runtime_config/mod.rs) |
| 数据库结构 | [PostgreSQL migrations](../migrations/)、[SQLite 映射](development/sqlite-schema-mapping.md) |
| 旧产品蓝图与已退役计划 | [历史归档](archive/README.md)，不能作为当前行为依据 |

## 来源优先级

发现文档和实际行为不一致时，先核对当前实现、测试、migration 与配置类型，再对照机器可读契约。
当前用户/开发文档解释这些来源；设计记录保留决策背景，不替代运行时保证。
外部参考只说明所标版本或核对日期下的外部语义，不能推导网关已实现某个功能。
归档不覆盖以上任何来源。实现与契约之间的差异应作为缺陷核查，而不是靠修改说明掩盖。

## 维护文档

阅读[编写规范](documentation-standard.md)和[Pi 文档研究与组织设计](development/documentation-design.md)。
新页面按任务进入对应分类索引，不把实施进度追加到首页。
编码 Agent 的操作约束仍在 [`AGENTS.md`](../AGENTS.md)，不与用户指南混用。

```bash
git diff --check
python3 scripts/check-docs.py
```
