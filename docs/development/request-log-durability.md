# 请求日志耐久化流水线

> 状态：当前。

请求日志不再直接依赖最终 `request_logs` 宽表的瞬时写入能力。生产启动路径使用三段式流水线：

```text
派发前同步 intent + 预分配终态 slot
  -> 终态同步 slot
  -> 本地 append-only spool
  -> PostgreSQL request_log_ingest（COPY FROM，低索引）
  -> request_logs（查询宽表）
  -> 用户余额与 API Key 额度批量结算
```

## 耐久边界

所有完成路由选择的客户端逻辑请求，在 Connector 准备/上游派发前同步持久化
`admissions/<request UUID>.json`，并为最大 1MiB 终态 payload 预分配 `.slot` 文件。
一个 HTTP/SSE 请求或每个 WS `response.create`（包括 `generate:false` warmup）只预占一次；
重试不重复预占，正常结束和客户端取消都由同一个 CompletionGuard 完成。
准入失败返回 HTTP/WS `503 request_log_unavailable`，不派发、不伪造取消日志。
路由选择前拒绝和后台 scheduled probe 不属于该准入范围，仍使用原终态端口。
策略决策见[日志故障准入审查](request-log-admission-review.md)。

`DurableRequestLogSink` 在请求完成时同步完成以下操作：

1. 将不含请求体、成功响应体、Header 或凭据的终态事件编码为带版本的 JSON；失败事件可包含最长 16KiB、已清理控制字符的上游错误响应详情，以及网关或传输错误诊断。
2. 已准入请求先把终态写入预分配 slot，带版本、长度和 CRC32，并同步文件。
3. 写入带长度、UUID 和 CRC32 的本地追加文件。已准入请求同步追加文件后才发布末尾位置，
   删除 intent/slot，并同步相应目录变更。
4. 发送一个可合并的后台唤醒通知。

通知队列满不会丢日志，因为队列只负责唤醒；本地 spool 才是待处理数据源。进程重启时会从持久化 checkpoint 后继续读取。数据库提交成功但 checkpoint 尚未更新时会安全重放，并由最终表 UUID 主键保持幂等。

已准入客户端请求使用逐请求同步，不等待 PostgreSQL；生产 Tokio 多线程运行时通过
`block_in_place` 让出网络执行器线程，并保持同步所有权交接，避免取消与后台预占竞争。
这些本地同步不是无延迟操作，吞吐依赖存储同步性能，本次没有运行性能基准。
没有准入的路由前拒绝和 scheduled probe 日志仍默认每 10ms group-sync，
该部分主机掉电可能损失最后一个窗口；优雅关闭会再次同步。
逐请求同步以文件系统/设备正确执行 allocation、文件/目录 sync 为前提，
不是磁盘损毁、主机永久丢失或所有断电场景的证明。

## 故障恢复与未知 usage

- 准入写/同步失败会锁定 writer；移除故障后仍拒绝新派发，必须用原目录重启恢复。
- `events.log` 写失败时，已经准入的其他在途请求仍可写自己的预分配 slot。
  slot 完整时，启动自动使用同一 UUID 重放，再沿原 COPY/投影/结算链路幂等处理。
- 如果 slot 也失败、进程在终态前被杀或只留下撕裂终态，保留 intent 和非零 slot 证据，
  输出 `request_log_reconciliation_required`。**未知不等于失败/取消，不自动记零、不伪造 usage，
  不进入普通终态结算。**
- 本轮策略明确选择“只保留待核对”：未知记录本身不冻结用户或整个实例。
  重启会释放未知 slot 尾部的闲置预分配空间，预算只保留实际证据大小（每文件至少按 4KiB）；
  设施健康且预算允许时继续服务。因此存在人工核对期间的未知费用风险。
- intent 只有版本、请求/用户/Key/定价模型 UUID、开始时间、操作和传输类型；
  它不是 usage 或费用证据，亦不能证明已经派发（同步成功至真正 dispatch 之间仍有窗口）。
  不保存请求体、Header、成功响应体或凭据。

运维应按 ERROR 中的 UUID，在原 `admissions/` 中核对 JSON intent、slot、
数据库同 UUID 日志及提供方证据。数据库已有完整同 UUID 终态时先验证其结算状态，
不要手工再扣一次。没有可靠 usage 时保留待核对，不把缺失证据变成零费用成功记录。
当前没有自动核对、Console 待核对页面或补账 API；经核实需要归档的文件只能在实例停止后
由运维按具体 UUID 操作，禁止批量删除目录或通过换 spool 目录“恢复”。
slot 是二进制版本化 journal，不能当 JSON 或正常终态表直接导入。

spool 目录必须可写，并且同一台主机上的每个 Gateway 进程必须使用不同目录。进程会持有排他文件锁，防止两个实例同时写坏同一个 spool；Unix 下目录和文件会分别收紧为 `0700` 与 `0600`。
重启时应继续使用同一目录和同一业务数据库；切换数据库环境时必须同时切换 spool 目录，避免把旧环境的用户/API Key UUID 投影到新数据库。

## 升级边界

新增的 admission 文件不改变 `events.log` journal 版本，也不修改数据库 schema。
回滚到不理解 admission 的旧二进制前，必须先由新二进制重放完整 slot 并排空流水线，
将未知记录另行完成核对/保留，不能让旧二进制忽略尚未重放的终态文件。

Journal v3 的每条事件都显式包含 `request_protocol`，取值为
`non_stream`、`sse` 或 `websocket`。读取器仍兼容 v2，并根据旧事件的
`streamed` 推导 `non_stream` 或 `sse`；旧格式没有足够信息区分已经记录的
WebSocket 与 SSE，因此 v2 中所有 `streamed = true` 的积压事件都会按 SSE
投影。v1 仍因缺少必需的 `error_summary` 而不受支持；从会写入 v1 payload
的旧二进制升级前，必须先排空本地 spool 和 `request_log_ingest`。

`billing.usage.reasoning_tokens` 是 Journal v2/v3 的向后兼容扩展。已有
payload 缺少该字段时按 `0` 解码，因此这一扩展不要求排空现有 spool 或
`request_log_ingest`。

`reasoning_effort` 和 `fast_mode` 同样是 Journal v2/v3 的向后兼容扩展。旧 payload
分别按 `None` 和 `false` 解码，因此升级不要求排空已有 spool 或
`request_log_ingest`。

Journal v6 新增 `billing.peak_pricing`。它在请求开始时命中倍率大于 `1` 的每周 UTC
价格窗口时写入 `true`，并投影到 `request_logs.peak_pricing`；v5 及更早 payload 按
`false` 解码。版本号必须递增：共享数据库中仍运行的旧投影 Worker 会拒绝并保留 v6
入口记录，而不是把未知字段忽略后错误写成 `false`；新 Worker 随后可正常投影。滚动升级
应先完成数据库 migration 和所有实例替换，再依赖高峰标记；回滚到不支持 v6 的二进制前，
必须排空新版本实例的本地 spool 与 `request_log_ingest`。该快照字段让 Console 的
`Peak` 标记不依赖后来可能已修改的模型价格配置。

## 独立数据库连接池

日志流水线使用独立的 SQLx PostgreSQL 连接池：

- 控制面、Console 与运行时重载继续使用 `[database].max_connections`。
- spool ingestion、最终表投影、指标查询和结算只使用
  `[request_logging].database_max_connections`。
- 增加日志连接数不会自动提升总吞吐；同一 PostgreSQL 实例仍共享 CPU、WAL、磁盘和行锁。

默认日志池为四个连接，分别覆盖 COPY ingestion、低并发投影、结算和健康采样。最终表投影保持单 Worker，避免重新出现多个写 Worker 抢占转发资源的问题。

生产模板默认使用 4096 条 COPY、2048 条投影、4096 条结算批次与
500ms 结算间隔。较小机器可以将三种批次减半；较大机器应先扩大批次并验证
事务时长，而不是直接增加数据库 Worker。

## 低索引入口与最终投影

Migration `0012_request_log_ingest.sql` 创建 `request_log_ingest`：

- 数据使用 PostgreSQL `COPY FROM STDIN` 成批写入二进制 payload，入口阶段不解析 JSON。
- 入口表只维护 identity 主键和一个仅覆盖失败重试的部分索引。
- checkpoint 只在 COPY 提交后推进。
- 入口表允许重放产生重复 UUID；最终 `request_logs` 主键负责幂等归并。

投影 Worker 按 sequence 读取入口记录，解码后复用批量 `UNNEST` 写入现有 `request_logs`。成功行从入口表删除；格式错误、约束冲突或暂时失败的行保留在入口表并延迟重试，不会阻塞后续正常记录。

PG sequence 只作为不透明 `IngestReceipt` 交给 worker；批量接收接口为 `accept_batch`，
COPY 编码与数据库确认留在持久化实现内。结算 worker 使用独立 `SettlementRepository`，
查询/计量读取使用各自句柄，但本节的实际表、提交与重放顺序未改变。

投影时统一把 `failed`/`cancelled` 事件的费用归一为 `0`，因此升级前遗留在本地 spool 或
`request_log_ingest` 中的旧事件不会重新写入正费用或未知费用。成功事件仍要求 usage 才能得到费用。

这使“日志已耐久接收”与“日志已可在 Console 查询”成为两个不同阶段。持续流量高于最终宽表能力时，入口 backlog 会增长，但请求路径不会因宽表索引写放大而同步等待。

## 独立结算

结算 Worker 不再依赖每个插入批次的内存通知。它按固定间隔直接扫描最终表中的未结算记录，并继续：

- 在一个事务内 claim `billed_at`。
- 按用户聚合余额扣减。
- 按 API Key 聚合额度增加。
- 在提交后更新进程内 soft-quota 状态。

数据库行是恢复来源，因此结算允许落后于日志投影。关闭时会在配置的 drain deadline 内继续结算；未完成记录由下次启动恢复。
零费用失败/取消记录同样会取得一次 `billed_at`，但不会改变余额或额度。

## 实时面板与状态变化日志

Console `GET /console/v1/system/load` 是当前实例的主要实时视图，按请求读取队列、spool、
ingress/settlement backlog、累计失败数和数据库池压力。页面默认每 5 秒刷新，但不保存历史，也不做
多实例聚合。

后台健康采样固定每 10 秒运行，并只在状态变化时输出日志目标
`ai_gateway::request_log_health`：

- ingress 或 settlement backlog 的最老记录达到 30 秒时输出一次 `WARN`，恢复后输出一次 `INFO`；
- 日志数据库池达到配置容量且无空闲连接持续 30 秒时输出一次 `WARN`，恢复后输出一次 `INFO`；
- backlog 健康查询不可用时输出一次 `WARN`，查询恢复后输出一次 `INFO`。

连接池压力在发起 backlog 健康查询前采样。SQLx 会异步归还查询连接，因此该顺序避免后台采样器
和 Console 实时快照把自身的两个查询短暂计为日志连接池占用。

spool append、COPY、投影和结算操作本身的失败仍在发生时直接输出 `ERROR`。其中
`spool_append_failures_total` 必须为零；入口 backlog 持续增长表示最终表投影能力低于持续流量，
spool pending 持续增长表示 PostgreSQL 入口本身不可用或 COPY 能力不足。

完整日志目标 `ai_gateway::request_log_metrics` 改为可选 INFO 心跳。
`request_logging.metrics_interval_seconds = 0`（默认）关闭周期快照，但不会关闭面板、内存计数或
状态变化日志。无 Console 的部署如需日志历史，可设置为 `300`–`900` 秒。快照继续包含接收、spool、
COPY、投影、结算、backlog、耗时和日志数据库池累计字段。

## 关闭与容量

关闭顺序为：

1. 停止接收新的 HTTP 工作。
2. 将本地 spool 尽量 COPY 到数据库入口表。
3. 将入口记录尽量投影到最终表。
4. 批量恢复未结算记录。
5. 最后同步 spool 文件。

整个日志流水线达到 `shutdown_drain_seconds` 后，未完成数据保留在 spool、slot 或入口表供重启恢复。
`spool_max_bytes` 默认 1GiB，预算包括整个追加文件、每个在途请求的 slot 与终态追加余量、
以及待核对证据；它不是 PostgreSQL backlog 限额或跨进程磁盘配额。
`spool_min_free_bytes` 默认 64MiB，准入另检查文件系统可用空间和在途终态余量。
真正预留通过文件 allocation 完成，不支持该操作的文件系统会拒绝准入。
外部磁盘使用、I/O 错误仍可能使终态失败，不能把 free-space 检查当成绝对写入保证。

DB 故障但本地预算足够时继续服务；容量不足只拒绝新派发，不删除旧日志。
正常压缩阈值不能大于总预算，总预算至少容纳一个完整预占及追加余量。
容量压力会唤醒 ingestion Worker，在文件已排空时允许低于普通阈值压缩并重置 reader；
纯容量恢复不要求重启，写/同步失败锁定则要求重启。

生产模板将已排空 spool 的压缩阈值设为 256MiB，以减少高请求率下频繁
truncate/sync 对尾延迟的影响。完整机器分档和 PostgreSQL 参数见
[生产配置与容量调优](../user/production-configuration.md)。
