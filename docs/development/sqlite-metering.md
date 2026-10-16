# SQLite 耐久计量、结算与查询

> 状态：当前。Linux SQLite 与 PostgreSQL 共用财务业务契约，使用不同写入与聚合机制。

SQLite 的单 writer 不改变财务所有权：事实不可变，回执是唯一扣款权，
日志是可独立失败的展示投影。金额不能经过浮点、TEXT 排序或 SQLite 原生 SUM。
本页用于维护财务写入、恢复及报告；本地 slot/spool 见[日志耐久化](request-log-durability.md)。

## 目录

- [接口与实现地图](#接口与实现地图)
- [接收、计量与重放](#接收计量与重放)
- [唯一结算与恢复](#唯一结算与恢复)
- [查询和精确聚合](#查询和精确聚合)
- [验证](#验证)

## 接口与实现地图

RequestLogRepository、MeteringRepository、SettlementRepository、
RequestLogQueries 和 MeteringQueries 显式后端分派，保留 new(PgPool)，
feature 下用 from_sqlite(Arc<SqliteDatabase>)。
queries()/metering()/settlements() 派生同一 owner/池，不建第二写者、不向 worker 暴露 SQL。

| 文件 | 责任 |
| --- | --- |
| backend_pipeline.rs / backend_queries.rs | 公共窄接口分派 |
| sqlite/pipeline.rs | ingress、事实、独立投影、回执和恢复 |
| sqlite/queries.rs | 日志、费用、usage、渠道状态 |
| sqlite/aggregate.rs | 精确 scale-eight 累计及 PG 结果解码边界 |
| sqlite/leaderboard.rs / status.sql | 磁盘 staging、精确排序和百分位 |

IngestReceipt、payload 与 worker 方法保持 crate-private，
不为测试开放 executor 或增加公共 HTTP。
DatabaseHealth 读取既有 writer/readers 计数，不获取连接；
总容量由 database.max_connections 决定，日志池设置只对 PG 生效。

## 接收、计量与重放

```text
spool terminal
  → 整批事务 INSERT ingress → commit → checkpoint
  → fact + credential attribution + 新事实 pending + metered_at 同事务
       ├─ unique receipt + 账户更新 + pending 删除
       └─ 独立日志投影 → fact/日志均存在才可 ack
```

SQLite 不模拟 COPY，按有界绑定块写 INSERT，但整个接收批次仍是一个事务。
计量/展示各有重试次数、时间和原因；
坏 journal、财务冲突、展示冲突都不得提前 ack 丢证据。

财务比较使用存储列语义：UUID、微秒时间、12 位价格、8 位费用以及请求时归属。
首次插入与重放比较使用同一量化，不把合法首写误判冲突。
同 UUID 同规范内容 accepted，不同内容保留原事实并 conflict，不覆盖、不冲正。
明确无认证不回退当前/旧绑定；旧未知归属才用冻结注册表。

仅新事实创建 pending，结算后的重放不能再造工作。
fact/attribution/ready 失败一起回滚。
日志字段非法、删除或暂不可写不撤销事实，也不阻止确认费用结算。
insert/insert_batch 先提交事实再试投影；durable projector 调 project_batch，
不是第二套收费入口。Console billed_at 从回执 JOIN 派生。

## 唯一结算与恢复

唯一 BEGIN IMMEDIATE 内：

1. 对此前不存在的合格 UUID 插入回执。
2. 只按本次新增回执汇总用户余额/Key 已用额度。
3. checked Decimal 运算并检查列范围、账户更新集合。
4. 删除对应 pending，再提交。

余额、Key、约束、溢出或提交失败整批回滚；
软配额允许负余额但不扩大 numeric(24,8) 范围。
priced、zero_by_policy、unknown、invalid、not_applicable 与 PG 相同；
Key/user 不匹配不扣款，未知不记零。
事实与回执不可删改，展示删除不能重新开放扣款权。

恢复按 (completed_at,request_id) 扫 pending，不扫描全部已结算历史。
COMMIT 后取消仍可能结果未知，应查询 UUID 回执，不重放某条余额 UPDATE。
SIGKILL 测试覆盖已提交保留、未提交回滚及 pending 恢复，不等于断电/硬件损坏证明。
完整资格与保留要求见[独立计量](independent-metering.md)。

## 查询和精确聚合

usage、费用及排行榜读事实；成功率/延迟读日志。
本人查询保留所有权和渠道脱敏，管理员筛选/分页及 billed 语义不变。
凭证费用通过不可变归属，不因当前渠道改绑变化。

### 金额中间态

- numeric(24,8) 转成八位小数整数单位，用 BigInt 精确累计。
  Decimal::checked_add 可能降 scale，不能冒充 PG SUM 中间精度。
- 聚合结束才进入锁定 SQLx 0.9.0 的 PG NUMERIC → Decimal 解码边界，
  再用共享报告折叠规则；保持既有容量/显示，不扩大 API 为任意精度金额。
- 结算使用有界列金额与 checked Decimal；最终列越界整批回滚。
- 不读日志展示 JSON，不用金额 TEXT 字典序、SUM 或浮点 CAST，
  也不按当前模型价格重算历史。

对照 case 累计 80,000 个最大列金额，覆盖跨模型和超过 96-bit coefficient 的中间态；
这是财务精度测试，不是 forwarding 负载。

### 有界读取与快照

历史流式读取，内存只留报告聚合键，不加载全部历史行。
渠道百分位用窗口排序的相邻秩和 PG 插值，只返回聚合行，不保留样本 Vec。
排行榜以文件临时表和 512 行 keyset 页归并精确金额，不在内存保留全部 (period,user)。
temp_store=FILE 的临时排序/staging 是 SQLite 管理中间态，不是备份业务状态。

排行榜刷新在数据库共享的非阻塞 guard 下执行，
并发返回 AlreadyRunning，取消释放；读取事实和发布快照在同一写事务。
summary、entries、历史导航及渠道状态的多次读取保持单读事务，避免混合版本。

时间桶/上海日历先去亚秒，避免 SQLite 日期函数把最后微秒进位到下一周期。
人类名称按 BINARY 排序，不模拟部署者 PG locale；
金额、UUID、时间和 API format 有专属规范。
状态纳入开关属于能力，组报告按 (group_id,api_format) 区分。

## 验证

```bash
cargo test --locked --features sqlite-backend --lib persistence::sqlite
cargo test --locked --features sqlite-backend --test sqlite_foundation
cargo test --locked --features sqlite-backend --test control_plane_integration sqlite_s4_parity
cargo clippy --locked --workspace --all-targets --features sqlite-backend
cargo test --locked --workspace
cargo test --locked --workspace --features sqlite-backend
```

| 测试 | 关键证据 |
| --- | --- |
| pipeline.rs 单元契约 | ingress/重试、fact/pending/ready 原子、提前 ack、分块仍整批、有界扫描 |
| sqlite_pipeline.rs | 两类冲突、资格/归属、账户溢出回滚、写入各阶段取消、子进程 SIGKILL |
| sqlite_queries.rs | 日志/全部费用视图、筛选、不可变来源 |
| leaderboard 单元契约 | 独立句柄共享刷新 guard、取消释放 |
| sqlite_s4_parity.rs | 同 case 跑 PG/SQLite，规范时钟后序列化报告，金额容量和日/月/周/状态微秒边界 |

仓储对照与[系统 E2E](system-e2e.md)真实 CLI/浏览器、成对备份恢复互补。
OAuth/额度外部不确定结果见[Codex](sqlite-codex.md)，
文件和最低 native 版本见[生命周期](sqlite-lifecycle.md)，部署见[用户指南](../user/sqlite.md)。
