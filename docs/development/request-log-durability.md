# 请求日志耐久化流水线

> 状态：当前。

请求日志不再直接依赖最终 `request_logs` 宽表的瞬时写入能力。生产启动路径使用三段式流水线：

```text
终态请求事件
  -> 本地 append-only spool
  -> PostgreSQL request_log_ingest（COPY FROM，低索引）
  -> request_logs（查询宽表）
  -> 用户余额与 API Key 额度批量结算
```

## 耐久边界

`DurableRequestLogSink` 在请求完成时同步完成以下操作：

1. 将不含请求体、响应体、Header 或凭据的安全事件编码为带版本的 JSON；事件可包含从结构化 SSE 错误中提取并限制长度的错误代码与消息摘要。
2. 写入带长度、UUID 和 CRC32 校验的本地追加文件。
3. 更新文件末尾位置并发送一个可合并的后台唤醒通知。

通知队列满不会丢日志，因为队列只负责唤醒；本地 spool 才是待处理数据源。进程重启时会从持久化 checkpoint 后继续读取。数据库提交成功但 checkpoint 尚未更新时会安全重放，并由最终表 UUID 主键保持幂等。

默认每 10ms 对 spool 执行一次 `sync_data`，并在优雅关闭时再次同步。因此普通进程崩溃可以恢复已经完成 `write` 的事件；主机掉电时仍可能损失最后一个 group-sync 窗口。要求掉电场景也具有逐条确认语义时，应使用同步持久化存储或 Kafka/JetStream 等外部 durable broker，不能仅依赖异步本地文件。

spool 目录必须可写，并且同一台主机上的每个 Gateway 进程必须使用不同目录。进程会持有排他文件锁，防止两个实例同时写坏同一个 spool；Unix 下目录和文件会分别收紧为 `0700` 与 `0600`。
重启时应继续使用同一目录和同一业务数据库；切换数据库环境时必须同时切换 spool 目录，避免把旧环境的用户/API Key UUID 投影到新数据库。

## 升级边界

Journal v3 的每条事件都显式包含 `request_protocol`，取值为
`non_stream`、`sse` 或 `websocket`。读取器仍兼容 v2，并根据旧事件的
`streamed` 推导 `non_stream` 或 `sse`；旧格式没有足够信息区分已经记录的
WebSocket 与 SSE，因此 v2 中所有 `streamed = true` 的积压事件都会按 SSE
投影。v1 仍因缺少必需的 `error_summary` 而不受支持；从会写入 v1 payload
的旧二进制升级前，必须先排空本地 spool 和 `request_log_ingest`。

## 独立数据库连接池

日志流水线使用独立的 SQLx PostgreSQL 连接池：

- 控制面、Console 与运行时重载继续使用 `[database].max_connections`。
- spool ingestion、最终表投影、指标查询和结算只使用
  `[request_logging].database_max_connections`。
- 增加日志连接数不会自动提升总吞吐；同一 PostgreSQL 实例仍共享 CPU、WAL、磁盘和行锁。

默认日志池为四个连接，分别覆盖 COPY ingestion、低并发投影、结算和周期指标。最终表投影保持单 Worker，避免重新出现多个写 Worker 抢占转发资源的问题。

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

这使“日志已耐久接收”与“日志已可在 Console 查询”成为两个不同阶段。持续流量高于最终宽表能力时，入口 backlog 会增长，但请求路径不会因宽表索引写放大而同步等待。

## 独立结算

结算 Worker 不再依赖每个插入批次的内存通知。它按固定间隔直接扫描最终表中的未结算记录，并继续：

- 在一个事务内 claim `billed_at`。
- 按用户聚合余额扣减。
- 按 API Key 聚合额度增加。
- 在提交后更新进程内 soft-quota 状态。

数据库行是恢复来源，因此结算允许落后于日志投影。关闭时会在配置的 drain deadline 内继续结算；未完成记录由下次启动恢复。

## 结构化指标

日志目标 `ai_gateway::request_log_metrics` 周期输出：

- `recorded_total`
- `spooled_total`
- `spool_append_failures_total`
- `spool_pending_bytes`
- COPY 批次、行数、失败数及总/最大耗时
- `ingress_backlog_rows_estimate` 与最老 backlog 年龄
- 投影成功、延迟重试、失败及耗时
- 结算成功、失败及耗时
- 独立日志连接池 size/idle

`spool_append_failures_total` 必须为零。入口 backlog 持续增长表示最终表投影能力低于持续流量；spool pending 持续增长表示 PostgreSQL 入口本身不可用或 COPY 能力不足。

## 关闭与容量

关闭顺序为：

1. 停止接收新的 HTTP 工作。
2. 将本地 spool 尽量 COPY 到数据库入口表。
3. 将入口记录尽量投影到最终表。
4. 批量恢复未结算记录。
5. 最后同步 spool 文件。

整个日志流水线达到 `shutdown_drain_seconds` 后，未完成数据保留在 spool 或入口表供重启恢复，而不是被丢弃。磁盘空间仍是硬容量边界；生产环境必须监控 spool 目录和 PostgreSQL 存储，并为持续流量提供足够容量。

生产模板将已排空 spool 的压缩阈值设为 256MiB，以减少高请求率下频繁
truncate/sync 对尾延迟的影响。完整机器分档和 PostgreSQL 参数见
[生产配置与容量调优](../user/production-configuration.md)。
