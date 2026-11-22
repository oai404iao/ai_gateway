# ai-gateway 文档

> 状态：当前。

第一次运行，从[快速上手](user/quickstart.md)开始：启动网关，配置上游，用客户端 Key 发出第一条请求。

| 我想…… | 入口 |
| --- | --- |
| 部署或升级 | [生产部署](user/production-deployment.md) · [SQLite 部署与恢复](user/sqlite.md) · [升级与回滚](user/upgrades.md) |
| 接入上游并分配访问权限 | [上游凭证](user/upstream-credentials.md) · [模型与路由](user/routing.md) · [Console](user/console.md) |
| 调用网关或排查请求 | [公共 API](user/data-plane.md) · [请求日志](user/observability.md) · [计费与额度](user/billing.md) |
| 查协议细节 | [OpenAI 接口范围与外部参考](reference/README.md) · [公共请求策略](reference/request-allowlists.json) · [Console OpenAPI](openapi/console-v1.yaml) |
| 修改代码 | [开发环境](development/getting-started.md) · [架构](development/architecture.md) · [维护者专题](development/README.md) |

[全部用户指南](user/README.md) · [项目概览（中文）](../README.zh-CN.md) · [Project overview (English)](../README.md)

## 来源优先级

查字段与端点，读 OpenAPI 和请求策略；查配置与存储，读[配置模板](../config.example.toml)、[配置类型](../src/runtime_config/mod.rs)和 [migrations](../migrations/)。用实现和测试核对实际行为。外部参考注明对应版本或日期，[历史归档](archive/README.md)保留旧方案。[文档编写规范](documentation-standard.md)说明改动后要同步哪些页面。
