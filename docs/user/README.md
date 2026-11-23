# 用户文档

> 状态：当前。

第一次运行网关，请按[快速开始](quickstart.md)配置上游并发出一条请求。已有实例可按下列任务查找说明。

## 调用与管理

- [调用公共数据面](data-plane.md)：可用接口、请求策略、图片、独立搜索和重试边界。
- [配置模型与路由](routing.md)：建立接入、渠道、能力、候选及 Key 授权，配置健康测试和 Session 粘性。
- [使用 Console](console.md)：登录、邀请、注册、账户恢复、用户与管理员 API。
- [管理上游凭证](upstream-credentials.md)：目标范围、复用、轮换、停用和删除。
- [接入 Codex](codex.md)：OAuth、导入导出、Token/quota、Responses、搜索与图片；先[安装插件](connector-plugins.md)。
- [使用 Responses WebSocket](websocket.md)：启用条件、顺序调用与丢失上下文后的恢复。
- [配置计费与额度](billing.md)：USD 价格、倍率、时间窗口和软额度边界。

## 部署与运维

- [Docker Compose 生产部署](production-deployment.md)：配置、密钥、持久卷、启动和停止。
- [配置与容量调优](production-configuration.md)：TOML 与运行时设置、超时、请求体、PostgreSQL 和磁盘预算。
- [SQLite 单实例部署与恢复](sqlite.md)：Linux 文件后端、进程所有权和离线成对备份。
- [查看日志、负载与用量](observability.md)：当前实例监控、查询日志、统计延迟和日志故障处理。
- [存储基线切换与回滚](upgrades.md)：排空原账本、新建数据库与 spool、重建授权和配套恢复。
- [安全运行与账户恢复](security.md)：网络入口、秘密、原生代码和恢复证据。

## 查协议与契约

公共数据面和 OpenAI 的兼容范围见[外部参考](../reference/README.md)。Console API 形状以 [OpenAPI](../openapi/console-v1.yaml) 为准；公共 Header 与顶层字段动作以[请求白名单契约](../reference/request-allowlists.json)为准。

架构、测试和发布维护进入[开发文档](../development/README.md)；完整来源优先级见[文档中心](../README.md)。
