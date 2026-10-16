# 持久化操作接口

> 状态：当前。公共接口显式分派 PostgreSQL 和 Linux SQLite，应用不接触驱动或 SQL 方言。

应用持有能完成具体业务操作的句柄，而不是数据库连接。
prepared change 负责提交一个已准备的变更；Codex guard 负责一次维护操作；
结算句柄独占扣款权。接口分离不意味着为每个职责再建连接池。

## 目录

- [接口与实现位置](#接口与实现位置)
- [控制面事务与发布](#控制面事务与发布)
- [Codex 外部副作用](#codex-外部副作用)
- [事件、错误与观测](#事件错误与观测)
- [验证与限制](#验证与限制)

## 接口与实现位置

| 接口 | 责任 |
| --- | --- |
| ControlPlaneRepository | 一致读取和指定 prepare 操作；原始事务能力只在存储内 |
| PreparedControlPlaneChange | 读取完整候选记录，提交既定变更/审计；丢弃则回滚 |
| AuthRepository | 身份/会话事务；HTTP 通过 ConsoleAuthService 使用，不取仓储 |
| CodexRefresh / CodexQuotaReset | 凭证专属维护 guard，不提供任意 SQL |
| RequestLogRepository | 耐久接收、ingress 重试/确认和日志投影 |
| MeteringRepository | fact、归属、首次 pending 和 ready 原子物化及核对分类 |
| RequestLogQueries | 本人/管理员日志、渠道状态；不提供 ingress 写入或扣款 |
| MeteringQueries | usage、费用和排行榜 |
| SettlementRepository | 唯一回执、账户变更、恢复扫描和积压 |
| DatabaseHealth | 不获取连接的只读池观测 |

这是具体用例集合，不是 ORM、通用 CRUD 或事务执行框架。
queries()/metering()/settlements() 派生共享原池的窄句柄；
结算 worker 只持有 SettlementRepository。
PG 保留控制面和日志两池；SQLite 所有仓储共享 Arc<SqliteDatabase>、唯一写池和 owner。

| 位置 | 内容 |
| --- | --- |
| backend_auth.rs / backend_control_plane.rs | 公共仓储和 prepared-change 的封闭后端分派 |
| backend_codex.rs | 后端中立 Codex guard |
| backend_pipeline.rs / backend_queries.rs | 财务流水线与查询分派 |
| postgres_control_plane.rs / auth.rs / control_plane_write.rs / codex_write.rs | PG SQL、行映射及专属事务 |
| sqlite/ | SQLite 私有映射、文件所有权、单写事务、查询与外部操作协议 |
| storage_error.rs | 后端错误到稳定语义的翻译 |

SQLx 仍可用于组合根、配置连接选项、migration 和真实后端测试，
不能从应用通过 downcast 或 Error::source 恢复 SQL 分支。

## 控制面事务与发布

```text
coordinator 取得 serial 门
  → prepare_*：权限/身份/版本检查和业务修改
  → 同一事务读取完整 runtime_records
  → application 编译并校验候选
  → prepared.commit：既定审计和修改一起提交
  → 发布完整 ArcSwap 快照
```

PG 使用 SERIALIZABLE，SQLite 使用 BEGIN IMMEDIATE。
管理员、自助 Key、批量、目录同步、设置、自动状态和 reload 共用此边界。
调用者不能追加 SQL；提交前丢弃/取消会回滚。编译或审计失败不提交、不发布。
空自动状态转换提交空事务但不发布；用户设置保持原本不新增审计的语义。

调用者必须先验证候选，唯一业务编排位于 coordinator::commit_change。
prepared 对象不反向依赖应用编译器。
提交后退出由启动/重载恢复；数据库与 ArcSwap 不构成分布式事务。

## Codex 外部副作用

公共 guard 绑定最初选定凭证，外部 HTTP 始终由 application/codex 发起。
不增加 Token 轮换或兑换的自动重试。

| 后端 | 操作协议 |
| --- | --- |
| PG | 保留进程互斥、行锁和 generation 检查；事务覆盖外部调用，complete/fail 提交既定状态，丢弃回滚 |
| SQLite | 每凭证互斥 + 短事务 CAS；完整本地校验后 prepare_dispatch 持久化 attempt intent，再发送 HTTP；结果短事务校验 attempt/generation 并提交 |

PG 的 prepare_dispatch 是 no-op，不表示已有 SQLite 的耐久 intent fencing。
SQLite 不能在全局写锁下等待 HTTP；取消、未知结果或提交失败保留 intent。
未知 refresh 需显式重导入；未知 quota-reset 没有确认 API，重导入、超时或重启均不能清除。
guard 还持有逻辑 owner，close 必须等待释放，见[SQLite Codex](sqlite-codex.md)。

guard 测试证明锁/取消/generation/审计边界，不等于模拟了提供方 Token 已轮换后
PG COMMIT 回复丢失。外部服务与数据库没有原子事务。

## 事件、错误与观测

worker 调用 accept_batch，PG COPY 编码/确认或 SQLite 批量 INSERT 留在后端内部。
IngestReceipt 是不透明工作标识，调用者不能计算序号或构造范围。
接收提交才推进 checkpoint；ack 额外要求 fact、ready 和日志都存在。
journal payload 保留，计量与展示有独立重试，详见[独立事实](independent-metering.md)。

StorageError 私有持有驱动 source，StorageFailureKind 提供冲突、输入拒绝、
路由依赖、内部失败。SQLSTATE/约束名白名单只在 storage_error.rs；
SQLite 根据自身错误码分类，不伪装 SQLSTATE。
HTTP 映射语义类别，保留既有状态码和文案。

数据库输入拒绝与 RepositoryError::Validation 仍有区别，避免改变 Auth 包装行为。
底层 source 仅供诊断和后端测试；分类不构成重试授权，
也不能把提交结果未知当成明确回滚。

## 验证与限制

tests/contracts/interfaces.rs 固定：

- application/http 无 SQLx 类型或数据库错误分支；
- prepared change 取消回滚并释放锁；
- 审计失败无提交/发布，恢复后仅一次审计；
- 错误分类不合并原有区别；
- PG Codex guard 的锁、generation、失败状态和兑换审计。

双后端控制面、财务和 Codex 契约分别见
[SQLite 控制面](sqlite-control-plane.md)、[计量](sqlite-metering.md)、[Codex](sqlite-codex.md)。
P1 金额/幂等和真实 PG COPY/锁测试继续保留；系统 E2E 验证真实服务恢复。
转发路径变化另需授权上游 smoke。

接口没有提供自动补账、事实清理或日志 TTL；日志删除不能删除财务证据。
PG 的持锁外部 HTTP 与其不确定结果边界仍存在，不因接口中立而消失。

## 相关文档

[持久化边界](persistence-boundaries.md)解释设计取舍；
[行为契约](persistence-contracts.md)列出金额与读写路径；
[数据库架构](database-architecture.md)提供实体地图。
