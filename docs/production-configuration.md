# 生产配置与容量调优

仓库默认配置面向单个 Gateway 进程与单节点 PostgreSQL，基线机器为：

- 4–8 个 CPU 核心。
- 4–8 GiB 内存。
- PostgreSQL 与 Gateway 同机，或位于低延迟私有网络。
- 可提供可靠 `fsync` 的本地 SSD/NVMe 或云块存储。

默认值的目标是保持请求日志耐久性、限制数据库竞争并提供足够的诊断数据，而不是在某一台开发机上追求最高基准分数。任何生产发布仍需按实际流量、日志保留周期、机器规格和存储类型做容量验证。

## 首次启动

默认 Compose 使用文件型数据库密码，不在仓库、Compose 环境变量或进程参数中内置弱密码：

```bash
mkdir -p ./config
openssl rand -hex 32 > ./config/postgres-password
chmod 600 ./config/postgres-password

cp config.example.toml ./config/config.toml
docker compose up -d
```

`config.example.toml` 默认包含：

```toml
[database]
url = "postgres://ai_gateway@127.0.0.1:5432/ai_gateway"
password_file = "./config/postgres-password"
```

相对路径以 Gateway 进程的工作目录为基准。数据库 URL 内联密码仍受支持，适合部分托管 PostgreSQL 连接串，但不能与 `password_file` 同时配置。内联密码中的特殊字符必须进行 URL 编码。

### 升级已有 Compose 数据卷

`POSTGRES_PASSWORD_FILE` 只在初始化新数据卷时设置数据库角色密码。已有
`postgres-data` volume 会保留原密码，不能通过替换密码文件自动轮换。

升级时可以先保留现有 `[database].url`，应用新的 PostgreSQL 参数和
migration；计划密码轮换时，再通过受保护的 `psql` 会话执行：

```text
\password ai_gateway
```

将角色密码设置为 `./config/postgres-password` 中的值后，再把 Gateway
配置切换到 `password_file`。不要删除生产 volume 来“应用”新密码或 init
脚本；删除 volume 会删除数据库。

Compose 将 PostgreSQL 端口默认绑定到 `127.0.0.1`。只有 Gateway 位于另一台受控主机或容器网络时，才应通过 `AI_GATEWAY_POSTGRES_BIND_ADDRESS` 放宽监听，并同时配置防火墙、私有网络和 PostgreSQL 主机认证。

## Gateway 参数

默认生产模板使用：

| 参数 | 默认值 | 说明 |
| --- | ---: | --- |
| `[database].max_connections` | 10 | migration、Console、控制面与重载 |
| `request_logging.database_max_connections` | 4 | COPY、投影、结算、指标各有连接余量 |
| `ingest_batch_size` | 4096 | 本地 spool 到入口表的 COPY 批次 |
| `projection_batch_size` | 2048 | 单 Worker 投影最终宽表 |
| `settlement_batch_size` | 4096 | 一次 claim/聚合的结算行数 |
| `settlement_interval_milliseconds` | 500 | 默认软额度更新延迟 |
| `spool_sync_interval_milliseconds` | 10 | 主机掉电时的默认 group-sync 窗口 |
| `spool_compaction_threshold_bytes` | 256 MiB | 降低高流量下的截断与同步频率 |

PostgreSQL 的连接预算至少应满足：

```text
Gateway 实例数
  × (database.max_connections + request_logging.database_max_connections)
  + migration / 管理 / 监控余量
```

默认 PostgreSQL `max_connections=50` 可容纳三个默认 Gateway 实例并保留少量管理余量。同一主机上的多个 Gateway 实例必须使用不同的 spool 目录；同一 spool 目录不能被多个进程共享。

不要仅通过增加日志连接数扩大吞吐。当前日志流水线的投影与结算并发受到刻意限制；连接过多会增加 WAL、索引、CPU 和热账户行锁竞争。

### 日志级别

默认过滤器保留生命周期、控制面和请求日志流水线指标，同时关闭逐请求完成日志：

```toml
[observability]
filter = "ai_gateway=info,ai_gateway::application::proxy=warn,ai_gateway::request_log_metrics=info,tower_http=warn"
```

高流量生产环境不应长期启用全局 `debug`。故障排查时应限制时间和范围，并确保容器或服务管理器具有日志轮转。

## PostgreSQL 参数分档

Compose 中所有主要参数都可以通过 `AI_GATEWAY_POSTGRES_*` 环境变量覆盖。以下数值是起点，不是容量承诺：

| 主机规格 | shared_buffers | effective_cache_size | work_mem | maintenance_work_mem | max_wal_size | 建议应用批次 |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| 2–4 GiB / 2–4 核 | 256–512MB | 1–2GB | 4MB | 128MB | 2GB | 2048 / 1024 / 2048 |
| 4–8 GiB / 4–8 核 | **1GB** | **4GB** | **8MB** | **256MB** | **4GB** | **4096 / 2048 / 4096** |
| 16–32 GiB / 8–16 核 | 4–8GB | 12–24GB | 8–16MB | 512MB–1GB | 8–16GB | 先测试 8192 / 4096 / 8192 |

应用批次依次表示 ingress、projection、settlement。大批次可以减少 SQL 往返，但会增加单事务时间、内存峰值和锁持有时间，必须通过真实负载验证。

例如，在 4GiB 机器上降低数据库内存：

```bash
AI_GATEWAY_POSTGRES_SHARED_BUFFERS=512MB \
AI_GATEWAY_POSTGRES_EFFECTIVE_CACHE_SIZE=2GB \
AI_GATEWAY_POSTGRES_MAINTENANCE_WORK_MEM=128MB \
AI_GATEWAY_POSTGRES_MAX_WAL_SIZE=2GB \
docker compose up -d
```

这些环境变量只由 Docker Compose 展开；Gateway 二进制仍只读取 TOML。
长期部署应把覆盖值保存在服务管理器或部署系统的受控配置中，而不是依赖临时
shell 环境。

`work_mem` 可能被一个查询的多个执行节点、多个并行 Worker 和多个连接同时分配，不能按“连接数 × work_mem”简单视为固定上限。内存受限环境应优先保持 4–8MB。

默认 Compose 还配置：

- `wal_buffers=16MB`
- `min_wal_size=1GB`
- `max_wal_size=4GB`
- `checkpoint_timeout=15min`
- `checkpoint_completion_target=0.9`
- `wal_compression=lz4`
- `autovacuum_max_workers=4`
- `autovacuum_naptime=30s`
- `jit=off`
- PostgreSQL 18 `io_method=worker`

以下耐久参数必须保持开启：

```text
fsync=on
synchronous_commit=on
full_page_writes=on
data_checksums=on
```

关闭它们可能提高短期基准分数，但会削弱数据库提交和请求日志的故障恢复语义。

## Autovacuum 与日志表

Migration `0013_request_log_autovacuum_tuning.sql` 对两张高变更表设置了独立策略：

- `request_logs`：按 2% 变更比例触发 vacuum/analyze。
- `request_log_ingest`：按 1% vacuum 比例处理持续 COPY/DELETE 周期。
- 两者保留 1000 行固定阈值，避免小型环境频繁启动 vacuum。

生产环境应监控：

```sql
SELECT
    relname,
    n_live_tup,
    n_dead_tup,
    last_autovacuum,
    last_autoanalyze
FROM pg_stat_user_tables
WHERE relname IN ('request_logs', 'request_log_ingest');
```

如果 dead tuple 长期增长，应先确认 autovacuum 是否受 I/O、锁或 worker 数量限制，再调整 scale factor 或 cost 参数。

## 查询与 I/O 观测

新 Compose 数据卷初始化时会启用 `pg_stat_statements`。现有数据卷不会重新执行 `/docker-entrypoint-initdb.d`，可手动执行一次：

```bash
docker compose exec postgres \
  psql -U ai_gateway -d ai_gateway \
  -c 'CREATE EXTENSION IF NOT EXISTS pg_stat_statements'
```

默认还启用：

- `track_io_timing`
- `track_wal_io_timing`
- 1 秒慢查询日志
- checkpoint 与锁等待日志
- Docker local 日志驱动，最多保留约 5 × 50MB

初步检查：

```sql
SELECT * FROM pg_stat_wal;
SELECT * FROM pg_stat_checkpointer;
SELECT * FROM pg_stat_io;

SELECT
    query,
    calls,
    total_exec_time,
    mean_exec_time,
    rows
FROM pg_stat_statements
ORDER BY total_exec_time DESC
LIMIT 20;
```

不要仅根据开发库的 `idx_scan=0` 删除索引。应覆盖 Console 查询、结算、统计报表和足够长的真实运行周期后再判断。

## 存储

数据库写入性能通常比连接数更早成为日志流水线的瓶颈。优先使用具有稳定延迟和可靠 flush 语义的 SSD/NVMe 或云块存储。

如果 PostgreSQL 位于 Btrfs、ZFS 或其他 CoW 文件系统：

- 优先使用为数据库准备的独立卷或数据集。
- Btrfs NOCOW 必须在创建数据库文件之前设置；对已有文件执行 `chattr +C` 不会安全地完成迁移。
- NOCOW 会同时失去 Btrfs 数据 checksum 与压缩，需要结合 PostgreSQL checksum、备份和底层冗余评估。
- 继续使用 CoW 时，可分别 A/B 测试 `wal_init_zero=off` 与 `wal_recycle=off`，不要和其他大项同时切换。

spool 应位于本地持久磁盘，而不是容器临时层。必须监控剩余空间；磁盘写满是本地耐久入口的硬失败边界。

## PostgreSQL 18 异步 I/O

默认保留兼容性较高的：

```text
io_method=worker
io_workers=3
```

8 核以上、读取或 vacuum I/O 明显受限的机器可以测试 4–6 个 `io_workers`。`io_uring` 应作为独立实验项验证内核、容器安全策略、文件系统和镜像构建支持，不能仅因系统支持该枚举就直接作为生产默认值。

Compose 对未知或虚拟磁盘保留保守的 `random_page_cost=4.0` 与
`effective_io_concurrency=16`。确认使用 SSD/NVMe 后，可以分别测试：

```bash
AI_GATEWAY_POSTGRES_RANDOM_PAGE_COST=1.5 \
AI_GATEWAY_POSTGRES_EFFECTIVE_IO_CONCURRENCY=64 \
AI_GATEWAY_POSTGRES_MAINTENANCE_IO_CONCURRENCY=64 \
docker compose up -d
```

这些值影响优化器计划和并发预取，应根据 `EXPLAIN (ANALYZE, BUFFERS)` 与
真实 I/O 指标决定，不能仅按磁盘宣传规格设置。

## 备份与高可用边界

默认 Compose 是单节点部署基线，不自动提供：

- PostgreSQL 流复制或自动故障转移。
- WAL 归档与时间点恢复。
- 跨主机 spool 复制。
- 数据库、JWT 密钥和本地 spool 的备份。

正式生产应优先采用受管理 PostgreSQL，或独立设计 base backup、WAL 归档、恢复演练、监控告警和故障切换。备份必须覆盖 PostgreSQL；spool 主要用于数据库短暂不可用和进程恢复，不能替代数据库备份。

## 调优验证顺序

每次只切换一组参数：

1. 确认错误率、spool pending、ingress backlog 和未结算 backlog 为零或可控。
2. 记录 PostgreSQL WAL、checkpoint、autovacuum、I/O 和锁等待。
3. 运行符合真实响应时长与流式比例的负载。
4. 比较吞吐、p50/p99、日志耐久率和关闭排空时间。
5. 再决定是否保留调整。

仓库中的 Quick/Standard Harness 仍只在明确要求时运行，报告也不等同于生产容量承诺。
