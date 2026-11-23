# SQLite 列编码与数据库约束

> 状态：当前。

[SQLite 基线](../../migrations/sqlite/0001_baseline.sql) 定义业务表、索引、触发器与初始数据。`STRICT` 表和连接上注册的校验函数共同约束写入；[生命周期](sqlite-lifecycle.md)说明业务基线、文件身份和重开条件。

## 列编码

| 值 | SQLite 表示 |
| --- | --- |
| UUID | 规范小写、带连字符的 TEXT，`SqliteUuid` 负责编解码 |
| 枚举、布尔、计数 | 枚举为 TEXT + CHECK；布尔和计数为有取值范围的 INTEGER |
| 时间戳、日期 | UTC 六位微秒加 `Z`，或 `YYYY-MM-DD`；微秒量化与 PostgreSQL 端一致 |
| 二进制与 JSON | 哈希等二进制字段为 BLOB；JSON 对象与数组按列校验类型、元素和 NULL 约束 |
| 金额 | 规范十进制 TEXT；`SqliteAmount` 对应 numeric(24,8)，`SqliteUnitPrice` 对应 numeric(24,12)，`SqliteTokenRate` 对应 numeric(14,4) |

列适配器检查精度和范围，计费逻辑决定舍入。写余额用 checked Decimal；统计查询用精确整数中间态，不使用 SQLite 的浮点 `SUM` 或 TEXT 字典序比较金额。见[计量与结算](sqlite-metering.md)。

## 写入与查询

- 更新受事务时钟保护的行时设置 `updated_at=ag_now()`。备份与直接 SQL 检查也要使用注册了 schema 函数的连接。
- 事实在派发时记录凭证归属，事实与结算回执不可更新或删除。查询日志可独立删除。已成功结算的 UUID 再次交付不会产生新的待结算项。
- 操作规则、tier 和候选通过外键、唯一性及操作约束关联；控制面在提交前编译整个路由图。标记删除的模型和渠道不再接受新引用。
- SQLite BUSY/LOCKED 映射为并发冲突；长度、类型或约束不满足映射为无效输入。路由依赖的业务错误由仓储返回，Console 不暴露驱动消息。

`tests/contracts/sqlite_schema.rs` 使用真实文件验证编码、直接写入约束与重开。`tests/contracts/sqlite_parity.rs` 对照 PostgreSQL 当前 schema 和初始数据。
