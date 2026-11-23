# SQLite 后端

> 状态：当前。

Linux 构建的 `sqlite-backend` feature 支持单实例本地部署。`database.url` 选择后端；身份、控制面、Codex、计量、查询和管理 CLI 共用同一个数据库。启用方式、目录权限和备份命令见[部署指南](../user/sqlite.md)。

## 连接与事务

组合根为 SQLite 文件创建一个 `Arc<SqliteDatabase>`，供仓储共享。`database.max_connections` 是一个 writer 和其余 reader 的总数。reader 使用只读连接和 `query_only`，按需开启一致性读事务。writer 用 `BEGIN IMMEDIATE` 序列化变更，启用 WAL、`synchronous=FULL`、外键和递归触发器。新连接注册日期、金额、枚举与 JSON 等 schema 函数。

控制面在事务中生成完整候选，应用编译成功后提交变更与审计，再原子发布快照。涉及外部 HTTP 的 Codex 操作用短事务写 intent，并在请求结束后按 generation/CAS 提交结果；等待网络时不占用 writer。

## 实现入口

| 修改内容 | 入口 |
| --- | --- |
| 目录所有权、文件身份、初始化及重开 | [SQLite 生命周期](sqlite-lifecycle.md) · [`ownership.rs`](../../src/persistence/sqlite/ownership.rs) |
| 类型、金融精度与约束 | [列编码](sqlite-schema-mapping.md) · [当前基线](../../migrations/sqlite/0001_baseline.sql) |
| 会话、认证与控制面发布 | [控制面](sqlite-control-plane.md) |
| 事实、幂等结算与精确查询 | [计量](sqlite-metering.md) |
| OAuth、额度和外部副作用 | [Codex](sqlite-codex.md) |
| 数据库与 spool 成对恢复 | [`backup.rs`](../../src/persistence/sqlite/backup.rs) · [部署指南](../user/sqlite.md) |

修改后运行 `cargo test --locked --workspace --features sqlite-backend` 和[双后端系统 E2E](system-e2e.md)。真实文件和子进程覆盖连接所有权、崩溃重放、离线恢复；对照 PostgreSQL 验证相同输入产生相同业务结果。
