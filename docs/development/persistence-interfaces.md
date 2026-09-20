# 持久化操作接口

> 状态：当前。第二阶段 P2 的操作边界；S6 已支持 PostgreSQL 和 Linux SQLite，
> 开发 feature 下的 SQLite 身份/普通控制面见 [S3](sqlite-control-plane.md)，
> 耐久财务链路与查询见 [S4](sqlite-metering.md)，Codex provider 操作和拼车 lease 见 [S5](sqlite-codex.md)。

整体演进见[持久化边界设计](persistence-boundaries.md)，业务不变量和费用路径清单见
[契约基线](persistence-contracts.md)。P3 已实现[独立计量事实/回执](independent-metering.md)，
本页侧重调用边界，不重复迁移与恢复协议。

## 当前职责

| 接口 | 责任 |
| --- | --- |
| `ControlPlaneRepository` | 控制面一致性读取、准备业务变更；原始事务方法仅持久化模块可见 |
| `PreparedControlPlaneChange` | 一个已经执行变更但尚未提交的事务；只允许读取完整候选记录或提交既定变更/审计 |
| `CodexRefresh` / `CodexQuotaReset` | 对选定凭证持有锁，提交对应刷新/失败/兑换结果；不提供任意 SQL 能力 |
| `AuthRepository` | 身份与会话事务；HTTP 通过 `ConsoleAuthService` 操作，不再取得 auth repository |
| `RequestLogRepository` | 终态批次耐久接收、ingress 重试/确认和日志物化 |
| `MeteringRepository` | 独立事实、pending 工作项与 ingress 可投影标记的原子物化，核对分类 |
| `RequestLogQueries` | 本人/管理员日志、渠道组状态；不提供结算或 ingress 写入 |
| `MeteringQueries` | 个人 usage、费用统计、排行榜投影/查询、拼车确定费用读取 |
| `SettlementRepository` | 原子认领与账户更新、恢复扫描、结算积压 |
| `DatabaseHealth` | 只读连接池观测，不提供连接获取或事务 |

这些是基于现有用例的具体接口，不是通用 ORM/CRUD/UoW 框架。
`RequestLogRepository` 的 `queries()` / `settlements()` 和查询句柄的 `metering()` 只派生共享
原池的窄句柄，不创建新池；结算 worker 只持有 `SettlementRepository`。
生产 PG 继续使用原控制面池与日志流水线池，不增加配置。
SQLite 开发构造器的所有写仓储共享一个 `Arc<SqliteDatabase>` 和唯一写池。

SQL 仍集中在 `src/persistence/`。公共仓储通过 `backend_auth.rs` /
`backend_control_plane.rs` 显式分派；原 `mod.rs` 中的 PG SQL/行映射和共享 DTO
位于 `postgres_control_plane.rs`，SQLite 有后端私有行映射。
Codex credential/window view 的费用聚合仍封装在该专用 PG 查询内部，已读取独立计量事实。

## 控制面事务与发布

`src/persistence/control_plane_write.rs` 封装管理员、自助 Key、批量操作、目录同步、
用户设置、手工 reload 和自动禁用/恢复的 prepare 路径：

```text
coordinator 取得现有进程内 serial 门
  -> prepare_*：内部 SERIALIZABLE、身份/版本/业务检查、变更
  -> 读取完整 runtime_records
  -> application 编译并校验候选快照
  -> prepared.commit：既定审计与修改一起提交
  -> application 发布完整快照
```

提交前丢弃或取消操作对象由 SQLx 回滚；不允许调用者取得 executor 或追加任意修改。
审计失败会回滚变更；编译失败不得提交或发布。空自动状态转换仍提交其内部空事务，
不发布新快照。用户设置保持原先没有新增审计的语义。

调用者必须先验证候选再 commit；当前唯一业务发布编排位于 coordinator 的
`commit_change`。对象自身不引用 application 编译器，不引入持久化层到应用层的依赖。
数据库提交后进程退出仍靠启动/重载恢复；数据库与 ArcSwap 不是一个分布式原子事务。

## Codex 锁与外部副作用

`src/persistence/codex_write.rs` 将已锁定记录与专属 guard 一起交给应用：

- refresh：generation 不符走 `unchanged`；成功走 `complete`；提供方/身份错误走 `fail`。
- quota reset：锁覆盖外部兑换到结果/审计提交，`complete` 绑定最初选定的凭证和 credit 观测。
- 丢弃 guard 回滚并释放锁；外部 HTTP 仍在 `application/codex/mod.rs`，不进入 PG 仓储。
- 保留现有进程锁、行锁、generation 条件和提交后的 credential reload。
- 不增加事务或外部调用自动重试。token 轮换/兑换成功而数据库提交结果不确定的既有边界未解决。

缩短持锁网络事务、提供方级幂等和 lease/fencing 属于独立设计，不包含在此次重构中。
guard 集成测试验证了锁阻塞/释放、失败状态提交、generation 冲突回滚及兑换结果/审计原子性；
它们不等同于模拟了提供方 token 轮换成功后的 COMMIT 回复丢失。

## PG 优化与错误封装

事件 worker 调用 `accept_batch`，COPY 文本编码、传输和确认都在 PG 实现内部。
`IngestReceipt` 隐藏 PG sequence 的数值类型，worker 只保存/传回不透明 receipt，
不计算序号或构造确认范围。journal payload 保留；P3 在 ack 前额外要求计量事实与 ready 标记，
checkpoint 仍只等待 COPY 耐久接收。

`StorageError` 私有持有 SQLx source，并提供 `StorageFailureKind`：
冲突、输入拒绝、路由依赖、内部失败。SQLSTATE/constraint 白名单只在
`src/persistence/storage_error.rs` 分类；HTTP 只映射语义类别，状态码和错误文案不变。

数据库输入拒绝仍不同于原有业务 `RepositoryError::Validation`，
避免 Auth 对业务错误的特殊包装改变数据库错误响应。
`Error::source` 仅用于底层诊断和 PG 集成测试；应用不能通过 downcast 重新分支 SQLSTATE。
分类不构成自动重试授权，也不把不确定提交误报为明确回滚。

应用/HTTP 不再依赖 PG pool/事务/driver error。组合根、配置连接选项、
持久化实现和 PG 集成测试仍可使用 SQLx。

## 验证与仍有的耦合

`tests/contracts/interfaces.rs` 验证：

- application/http 源码没有 SQLx 类型和数据库错误分支；
- prepared change 取消回滚且释放锁；
- 审计失败不提交、不发布，恢复后只提交一次审计；
- PG 错误分类保留既有区别；
- Codex guard 的锁/generation/失败提交和兑换审计。

同时保留 P1 金额/事务/重放测试、完整控制面与 Console spec 测试。
涉及 Codex 请求路径时执行经授权的真实上游 smoke；系统 E2E 验证原耐久流水线。

S6 已增加 Linux SQLite 部署；仍不提供自动补账、事实清理或日志 TTL。
P3 已移除日志认领权；事实/回执不可删改，日志删除不能删除财务证据。
原有 Codex 长事务与外部结果不确定边界仍存在。
