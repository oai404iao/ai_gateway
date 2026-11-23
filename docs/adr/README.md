# 架构决策记录

> 状态：当前索引。

| 决策 | 约束与取舍 | 当前实现 |
| --- | --- | --- |
| [宿主持有传输，插件生成协议计划](connector-host-boundary.md) | 出站策略、连接复用和派发后的重试边界不能按供应商分叉 | [请求准备与派发](../development/relay-boundaries.md) |
| [派发前预留终态，计量独立于日志查询](durable-metering.md) | 上游调用不可撤销，展示故障不能阻塞已知费用结算 | [本地耐久化](../development/request-log-durability.md)、[计量与回执](../development/independent-metering.md) |
| [新账本使用独立基线](fresh-ledger-baseline.md) | 旧状态只由匹配的旧软件解释，不能静默漏账或改变凭证归属 | [数据库初始化](../development/database-architecture.md#迁移与升级)、[部署切换](../user/upgrades.md) |
