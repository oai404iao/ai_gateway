# SQLite 耐久计量、结算与查询

> 状态：当前 S4 实现。仅供 `sqlite-backend` 开发验证；生产配置仍关闭。
> PostgreSQL 行为基线见[独立计量事实](independent-metering.md)，完整进度见
> [SQLite 双后端实施](sqlite-backend.md)。

## 接口和所有权

`RequestLogRepository`、`MeteringRepository`、`SettlementRepository`、
`RequestLogQueries` 和 `MeteringQueries` 使用显式后端分派，
保留 `new(PgPool)`，增加 feature-gated `from_sqlite(Arc<SqliteDatabase>)`。
派生的 `queries()` / `metering()` / `settlements()` 共享同一个数据库与唯一写池，
不创建日志专用第二写者，不暴露原始 SQL 给 worker。

实现入口：

- `src/persistence/backend_pipeline.rs` / `backend_queries.rs`：公共窄接口分派；
- `src/persistence/sqlite/pipeline.rs`：ingress、事实、独立日志投影、结算与恢复；
- `src/persistence/sqlite/queries.rs`：日志、费用、usage、渠道状态和拼车费用读取；
- `src/persistence/sqlite/aggregate.rs`：精确 scale-eight 累计与 PG 结果解码边界；
- `src/persistence/sqlite/leaderboard.rs` / `status.sql`：磁盘 staging 与百分位排序。

`IngestReceipt`、journal payload 和 worker 方法保持 crate-private，
没有为测试开放 executor 或新增公共 HTTP 接口。
`DatabaseHealth` / 日志池观测读取现有 writer+reader 池的计数，不获取连接；
生产容量配置与组合根接线仍属于 S6。

## 写入和恢复

```text
spool terminal
  → 同一写事务内批量 INSERT ingress → commit → 才能 checkpoint
  → 不可变事实 + 新事实 pending + ingress.metered_at 同事务
       ├─ UUID 回执 + 账户余额/Key 额度 + 删除 pending 同事务
       └─ 独立日志投影 → 事实/日志均存在才允许 ack ingress
```

SQLite 不模拟 PostgreSQL COPY；使用有界绑定批次的 INSERT，整批仍是一个事务。
计量和日志分别持久化重试次数、下次时间和原因；坏 journal、财务冲突、展示冲突
不得通过提前 ack 丢弃证据。

财务比较采用存储后的列语义：UUID、微秒时间、12 位价格和 8 位费用。
同 UUID 同财务内容重放 accepted；不同财务内容保持原事实并返回 conflict。
列精度量化同时用于插入和事实比较，不能把合法的首次写入误判成冲突。
仅新事实创建 pending；已结算重放不能重新创建工作或再次扣款。

事实与回执不可修改/删除；日志只是展示投影。投影字段校验失败、投影删除或暂不可写，
不撤回已提交的事实，不阻止已确认费用结算。Console `billed_at` 来自回执 JOIN。
`insert` / `insert_batch` 先提交事实，再尝试投影；durable projector 使用独立
`project_batch`，并非另一套计费入口。

结算在唯一 `BEGIN IMMEDIATE` 内处理此前不存在的合格 UUID 回执，按本次实际新增回执
汇总账户效果，再删除相应 pending。余额、Key 额度、约束或提交失败会整批回滚。
余额允许既有软配额透支，但不扩大 `numeric(24,8)` 列范围。
恢复只按 `(completed_at,request_id)` 扫描 pending，不扫描全部已结算历史。

`priced`、`zero_by_policy`、`unknown`、`invalid`、`not_applicable` 分类与 PG 保持一致；
Key 所有者不匹配不扣款，未知费用不记零。拼车费用读取只取合格事实，
不等待普通账户回执，也不更新拼车 WAL；S5 协议尚未实现。

取消到达 COMMIT 之后，结果仍可能不确定；恢复按不可变 UUID 回执核对，而不是重放
某条余额 UPDATE。SIGKILL 测试覆盖已提交回执保留、未提交账户修改回滚及 pending 恢复，
不声称模拟了断电或硬件损坏。

## 查询和精确聚合

费用统计、usage、排行榜和拼车恢复读取事实；渠道成功率/延迟仍读取日志投影。
本人日志保持原所有权条件和渠道信息脱敏，管理员筛选/分页及 billed 筛选保持原接口。

金额不经过 SQLite `SUM`、浮点 CAST、TEXT 字典序或日志展示 JSON：

- 每个 `numeric(24,8)` 金额转换为八位小数的整数单位，用 `BigInt` 精确累计。
  `Decimal::checked_add` 可能降低 scale，不能充当 SQL SUM 的精确中间态。
- 聚合结束才进入锁定 SQLx 0.8.6 的 PG NUMERIC → Decimal 解码边界，
  然后使用既有共享报告折叠规则。这里保留现有结果类型及其容量/显示语义，
  不将旧 API 扩展为任意精度金额，也不重新计算历史费用。
- 普通账户结算使用有界列金额和 checked Decimal；任何最终列溢出整笔回滚。
- 对照测试累计 80,000 个最大 `numeric(24,8)` 金额，覆盖跨模型分组及超过
  96-bit coefficient 的中间总和，确认与真实 PG 报告一致；这不是转发压测。

历史事实流式读取；内存仅保留报告所需聚合键，不加载全部历史行。
渠道百分位通过窗口排序取相邻秩并使用 PG 插值公式，只返回聚合行，不保留样本 Vec。
排行榜使用文件临时表和 512 行 keyset 页归并，按精确金额排序，
不会在 Rust 中保留所有历史 `(period,user)` 组合。
物理连接固定 `temp_store=FILE`；临时排序和 staging 是 SQLite 管理的临时存储，
不是可备份的业务状态。

排行榜刷新在数据库共享的非阻塞 guard 内运行；并发调用返回 `AlreadyRunning`，
取消释放 guard，发布快照和事实读取仍在同一个写事务内。
排行榜 summary、entries、导航以及渠道状态的多次读取使用单一读事务，防止混合版本。
时间桶与上海日历计算先去除亚秒，避免 SQLite 日期函数把最后一个微秒进位到下一周期。
人类名称按 SQLite BINARY 排序，不模拟部署者的 PostgreSQL locale；金额、UUID、
时间及 API format 排序有专属规范，不依赖名称排序。

## 验证和后续边界

```bash
cargo test --locked --features sqlite-backend --lib persistence::sqlite
cargo test --locked --features sqlite-backend --test sqlite_foundation
cargo test --locked --features sqlite-backend --test control_plane_integration sqlite_s4_parity
cargo clippy --locked --workspace --all-targets --features sqlite-backend
cargo test --locked --workspace
cargo test --locked --workspace --features sqlite-backend
```

- `pipeline.rs` 单元契约验证 crate-private ingress、重试、事实/pending/ready 原子性、
  提前 ack 拒绝、分块仍整批提交及有界恢复。
- `sqlite_pipeline.rs` 验证财务与展示冲突、状态/归属、余额/额度、溢出回滚，
  并在事实、投影、账户修改期间取消，以及真实子进程 SIGKILL 后恢复。
- `sqlite_queries.rs` 验证日志、全部费用视图、筛选与不可变来源；
  leaderboard 单元契约验证独立句柄共享刷新 guard 和取消释放。
- `sqlite_s4_parity.rs` 对 PG 和 SQLite 执行相同操作与断言，
  对比规范化时钟后的序列化报告，覆盖金额容量和微秒日/月/周/状态桶边界。

S4 未开放 SQLite 生产配置，也未实现 S5 OAuth/额度/WAL 协议或 S6 CLI、备份恢复、
完整系统部署验收。[原生 SQLite 修复版本门槛](sqlite-lifecycle.md#原生-sqlite-版本门槛)
仍必须在生产开放前解决。
