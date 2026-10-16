# 请求日志耐久化流水线

> 状态：当前。生产请求先获得本地终态落点，数据库接收后再独立计量、结算和投影。

日志“已本地同步”“数据库已接收”“财务已物化”“Console 可查询”是不同阶段。
每个阶段只有在自己的提交边界后才能确认上一阶段；通知队列不承载唯一证据。
PostgreSQL 使用 COPY 入口，Linux SQLite 使用事务批量 INSERT，共用本地准入和业务不变量。

## 目录

- [请求准入与本地同步](#请求准入与本地同步)
- [故障恢复与未知 usage](#故障恢复与未知-usage)
- [数据库接收、计量与投影](#数据库接收计量与投影)
- [独立结算](#独立结算)
- [journal 与升级边界](#journal-与升级边界)
- [连接、批次与观测](#连接批次与观测)
- [关闭与容量](#关闭与容量)
- [验证入口](#验证入口)

## 请求准入与本地同步

```text
派发前同步 intent + 预分配 terminal slot
  → 终态同步 slot → append-only spool
  → durable ingress → fact + attribution + pending + ready
       ├─ unique receipt + balance/Key quota
       └─ request_logs projection → ingress ack
```

完成选路的客户端逻辑请求在 Connector 准备/派发前同步
admissions/<request UUID>.json，并为最大 1MiB 终态 payload 预分配 slot。
HTTP/SSE 重试共用一次预占；WS 每个 response.create（含 generate:false warmup）单独准入。
正常结束与取消由同一个 CompletionGuard 完成。
失败返回 HTTP/WS 503 request_log_unavailable，不派发、不伪造取消日志。

路由前拒绝与 scheduled probe 不属于该准入范围，仍走原终态端口。
取舍背景见[准入决策](request-log-admission-review.md)。

DurableRequestLogSink 在完成时同步交接：

1. 编码版本化终态，不含请求体、成功响应体、Header 或凭据。
   失败可有最长 16KiB、清理控制字符的上游错误详情和网关/传输诊断。
2. 已准入请求先写 slot 的版本、长度、CRC32 并同步。
3. append 本地带长度、UUID、CRC32 的帧；已准入请求同步后才发布末尾位置。
4. 删除 intent/slot 并同步目录，再发送可合并唤醒通知。

通知满不丢事件，spool 才是待处理数据源。
接收提交但 checkpoint 未更新会重放，财务与日志各自按 UUID 比较归并。

生产多线程 Tokio 通过 block_in_place 让出网络执行器，保持同步所有权交接，
避免取消与后台预占竞争。这仍有存储同步延迟，不是无成本或已获吞吐认证。
未准入终态默认 10ms group-sync，断电可能丢最后窗口；优雅关闭再次同步。
文件 allocation、文件/目录 sync 依赖 OS/设备正确执行，不保证磁盘损坏或主机永久丢失无损。

## 故障恢复与未知 usage

| 状态 | 恢复与限制 |
| --- | --- |
| 准入写/sync 失败 | writer 锁定，去除故障后仍拒绝派发，须原目录重启 |
| events.log 失败但 slot 完整 | 其他在途仍可写各自 slot；重启用原 UUID 重放 |
| 终态前 kill、slot 失败/撕裂 | 保留 intent 和非零证据，输出 request_log_reconciliation_required |
| 仅容量不足 | 不删旧记录；已排空文件可压力压缩，恢复容量不必重启 |
| 数据库不可用 | 本地健康且预算足够则继续服务，不查询数据库来决定每请求准入 |

未知不是 failed/cancelled，不伪造 usage、不记零、不进入普通终态结算。
所选策略只保留待核对，不冻结普通用户或全局服务。
重启释放未知 slot 闲置尾部空间，按实际证据大小计费预算（每文件至少 4KiB），
健康且有容量时继续接新请求；**人工核对期间仍有未知费用风险**。

intent 只含版本、请求/用户/Key/定价模型 UUID、开始时间、操作和传输。
它既不是 usage/金额证据，也不证明已派发：sync 与 dispatch 之间仍有窗口。
slot 是版本化二进制 journal，不是 JSON 或可直接导入的终态表。

运维按 ERROR UUID 核对原 admissions、数据库同 UUID 事实/归属/回执/日志及提供方证据。
先确认财务事实，不因页面无日志再扣款。
无可靠 usage 就继续待核对；没有自动核对、补账 API 或 Console 核对页。
归档具体 UUID 必须停实例后人工处理，不批量删除证据或换目录“恢复”。

### 目录与数据库必须配对

每进程使用不同 spool 目录，排他文件锁防止同时写入。
Unix 目录/文件收紧为 0700/0600；重启沿用原目录与原业务数据库。
切换环境必须同时切换 spool，避免把旧用户/Key UUID 写入新库。
备份/恢复保存整套事件、checkpoint、admission 和数据库，不仅复制 events.log。

## 数据库接收、计量与投影

PG request_log_ingest 通过 COPY FROM STDIN 批量保存二进制 payload，
接收时不解析 JSON，允许重复 UUID；入口从低索引设计演进为计量/投影分阶段状态与重试索引。
SQLite 不模拟 COPY，使用有界绑定的整批事务 INSERT。
两者只在接收成功提交后推进本地 checkpoint，部分成功不能确认整批。

计量任务解码并在同一事务保存：
不可变 fact、凭证归属、仅新事实的 pending、metered_at。
坏 journal 或财务冲突保留 ingress，退避后逐条隔离，不标 ready。

投影只读 ready 记录。PG 批量 UNNEST 写 request_logs，SQLite 使用自己的绑定写入。
展示约束失败不撤销事实；投影成功才允许 ack。
数据库 guard 拒绝无 metered/事实/投影的提前删除。
财务重放冲突和展示 duplicate_conflict 分别保留，不覆盖先前内容。

failed/cancelled 在计量和投影前归零，旧积压不会恢复正费用或未知费用；
成功仍需可信 usage/费用才能计价。
结算与费用统计可早于日志可见，但持续投影失败仍消耗共享数据库空间与 I/O。

worker 只持有不透明 IngestReceipt，不计算 sequence 或确认范围。
accept_batch、COPY 编码、数据库确认和方言留在持久化内部，
详见[操作接口](persistence-interfaces.md)。

## 独立结算

结算 worker 按固定间隔扫描 pending，不遍历全部历史事实/日志：

1. 为合格 UUID 插入此前不存在的回执。
2. 只按新回执聚合用户余额扣减、Key 已用额度增加。
3. 校验更新集合，删除对应 pending。
4. 同事务提交；任一步失败全部回滚。
5. 提交后更新进程内 soft-quota。

零费用失败/取消也写一次回执，不改变账户金额。
Console billed_at 来自回执，不是日志列。
unknown、invalid、账户不匹配由 ai_gateway::metering_health 单独报告，
不能把普通 backlog 为空当作全部结清。
事实与回执不可删改，日志删除不能破坏认领证据。
资格、金额与升级细节见[独立计量](independent-metering.md)。

## journal 与升级边界

当前写 v8，读 v2–8；v1 因缺必需 error_summary 不支持。
旧 operation 名称在解码/重放时规范化，历史财务记录不重写。

| 格式或字段 | 兼容行为 |
| --- | --- |
| v3 request_protocol | non_stream/sse/websocket；v2 由 streamed 推导，true 全按 SSE，不能恢复缺失的 WS 证据 |
| reasoning_tokens | 旧 payload 缺字段按 0 |
| reasoning_effort / fast_mode | 旧 payload 按 None / false |
| v6 peak_pricing | 请求开始命中倍率 >1 的每周 UTC 窗口时 true；旧 v2–5 按 false，不读后来现价 |
| v7 六操作 | 旧名归一化，Responses 根据协议识别 WS；财务重放兼容旧名 |
| v8 credential attribution | 必须携带请求时凭证/明确无认证；旧 v2–7 为未知，查询只对未知用冻结注册表回退 |

peak 标记当时提高版本号，是为了让旧 worker 拒绝并保留新记录，而非忽略字段写 false。
这个局部兼容机制**不**表示可以跨当前 schema 硬切换混跑。

- 从写 v1 的版本升级，须先排空 spool 和 ingress。
- 不支持新 journal 的旧二进制不能接管新积压，回退前须排空或按成对恢复协议处理。
- admission 文件虽未改 events journal/schema，旧二进制不理解它；
  回退前用新版本重放完整 slot，未知证据另行核对/保留，不能忽略未重放终态。
- 0063 删除物理 billed_at 并转移扣款权，全部旧 worker 停止后才能迁移，
  不可只回退程序。它本身不改 journal。
- v8 归属随事实同事务；渠道改绑不能改变在途/历史费用。

## 连接、批次与观测

| 配置 | PostgreSQL | SQLite |
| --- | --- | --- |
| database.max_connections | 控制面/Console/重载池 | writer + readers 总预算，至少 2 |
| request_logging.database_max_connections | 独立日志池，模板默认 4 | 不建立独立池，所有仓储共享单 writer |
| ingest / projection / settlement batch | 模板 4096 / 2048 / 4096 | 相同有界业务批次，由后端内部写入 |
| settlement interval | 模板 500ms | 同一 worker 策略 |

计量共用 projection_batch_size，与投影各保持单 worker。
PG 四个日志连接由 COPY、计量、投影、结算和健康采样共享，不是任务各占一池。
同库 CPU、WAL、磁盘和锁仍共享；增加连接不自动增加吞吐。
调批次需测事务时长，不以添加 worker 代替容量验证。

Console GET /console/v1/system/load 是当前实例实时视图，默认每 5 秒刷新，
包含队列、spool、ingress/settlement backlog、累计失败和池压力；不存历史、不跨实例汇总。

健康采样每 10 秒运行，ai_gateway::request_log_health 只在变化时记录：

- backlog 最老记录达到 30 秒：WARN，恢复 INFO；
- PG 日志池满且无空闲持续 30 秒：WARN，恢复 INFO；
- backlog 查询不可用：WARN，恢复 INFO。

池压力在查询前采样，避免把 SQLx 异步归还的观测连接计为自身压力。
append/COPY/计量/投影/结算失败另在发生时 ERROR。
spool_append_failures_total 必须为零；入口积压持续增大需区分计量/投影，
spool pending 增大则检查数据库接收能力或故障。

metrics_interval_seconds=0 默认禁用 ai_gateway::request_log_metrics INFO 心跳，
不禁用面板、内存计数或状态变化日志。无 Console 可按需用 300–900 秒保留周期快照。

## 关闭与容量

关闭顺序：停止新 HTTP 工作 → spool 接收 → 计量 → 投影 → 结算 → 最后同步 spool。
shutdown_drain_seconds 到期后，耐久数据留在 slot/spool/ingress/pending 供重启恢复。

| 配置 | 默认与边界 |
| --- | --- |
| spool_max_bytes | 1GiB，计入整个追加文件、在途 slot、终态追加余量及待核对证据；不是 DB backlog 或跨进程配额 |
| spool_min_free_bytes | 64MiB，另检查文件系统可用空间与在途余量 |
| spool_compaction_threshold_bytes | 生产模板 256MiB，降低频繁 truncate/sync 影响；不得大于总预算 |

预算至少容纳一个完整预占及追加余量。预留依赖真实文件 allocation；
不支持的文件系统拒绝准入。外部磁盘使用与 I/O 故障仍能令终态失败，
free-space 检查不是绝对写入保证。
容量压力唤醒 ingestion，已排空时允许低于通常阈值压缩并重置 reader；
容量恢复与 writer 写/sync 锁定是不同状态。

## 验证入口

- request_log_spool 单测：allocation、sync、CRC、截断、锁、slot 恢复、容量和 checkpoint。
- PG/SQLite 财务契约：接收/ready/ack、冲突、投影阻塞、唯一回执、取消与重放。
- [系统 E2E](system-e2e.md)：真实 Gateway 的 DB outage、局部 syscall 故障及 kill/restart。
- [备份演练](persistence-rehearsal.md)、[SQLite 恢复](../user/sqlite.md)：库与本地证据成对恢复。

这些证据不等于设备掉电认证或性能基准。
配置来源为 src/runtime_config/mod.rs、两份模板；
机器分档与 PG 调优见[生产配置](../user/production-configuration.md)。
