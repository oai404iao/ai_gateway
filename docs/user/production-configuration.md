# 配置与容量调优

> 状态：当前。字段和默认值以[源码模板](../../config.example.toml)、[容器模板](../../deploy/compose/config.example.toml)和配置反序列化实现为准。

本页帮助你选择配置入口、设置请求与存储边界，并调优单实例 Gateway 与单节点 PostgreSQL。默认值是 4–8 核、4–8 GiB、低延迟数据库连接和可靠 fsync 本地磁盘的起点，不是吞吐承诺。SQLite 的连接与文件生命周期不同，见[专用指南](sqlite.md)。

## 首次启动

新部署按[快速开始](quickstart.md)生成秘密、创建管理员并建立路由；容器使用[生产部署](production-deployment.md)。不要在已有部署重复生成数据库密码或覆盖 TOML。

## 选择配置入口

| 内容 | 修改位置 | 生效方式 |
| --- | --- | --- |
| listener、数据库、插件目录、请求体上限、日志 spool/批次、观测 | 进程 TOML | 重启 |
| 上游超时、重试、被动健康、自动禁用、定时测试、Session affinity | 首次创建 `forwarding_policy` 前可用 TOML 初始化，之后在 Console“系统设置”修改 | 当前进程发布快照，其他实例周期重载 |
| 凭证、接入、渠道、操作能力、模型路由、价格、Key 与授权 | Console / Console API，持久化在数据库 | 完整快照校验与发布 |
| Codex provider 身份、合成工作区、隐私设置 | Console“插件管理”的插件设置 | 插件配置快照，不放回系统设置 |
| `AI_GATEWAY_*` 部署变量 | Compose 的受控 env 文件或部署系统 | 只展开 Compose，不是 Gateway 配置来源 |

serve 默认读取当前工作目录的 `./config/config.toml`，可传 `ai-gateway /absolute/config.toml`。没有 XDG 查找、dotenv 或自动 local override；相对秘密/spool 路径也按工作目录解析。不要把动态 `[[api_keys]]`、`[[channels]]` 或 `[[model_rules]]` 写进 TOML，这些字段会被拒绝。

## 监听器与请求体限制

```toml
[server]
host = "127.0.0.1"
port = 3000

[request_limits]
proxy_body_bytes = 1_048_576
image_edit_body_bytes = 67_108_864
image_edit_file_bytes = 52_428_800
image_edit_memory_bytes = 1_048_576
image_edit_spool_directory = "./data/image-edit-spool"
console_body_bytes = 262_144
auth_body_bytes = 16_384

[console]
enabled = true
host = "127.0.0.1"
port = 3001
allowed_origins = ["https://console.example.com"]

[auth]
issuer = "ai-gateway"
audience = "ai-gateway-console"
access_token_ttl_seconds = 900
refresh_token_ttl_seconds = 2_592_000
key_id = "primary-2026"
signing_key_path = "./config/console-jwt-private.pem"
verification_key_path = "./config/console-jwt-public.pem"
```

- 公共数据面默认监听 `127.0.0.1:3000`。
- Console 是独立监听器；应仅通过 HTTPS 反向代理对外暴露。
- `proxy_body_bytes` 限制 JSON OpenAI 代理请求，包括 Images generation。
- `image_edit_body_bytes` 与 `image_edit_file_bytes` 分别限制 multipart edit 总 body 和单个
  image/mask part；`image_edit_memory_bytes` 是转为匿名临时文件前的内存阈值。
- `image_edit_spool_directory` 必须位于容量足够的本地文件系统。Unix 上目录和临时文件分别使用
  `0700` 与 `0600`；图片字节不会进入请求日志。
- `console_body_bytes` 限制已认证 Console 写操作；`auth_body_bytes` 限制登录、注册、刷新和邀请激活请求。

## 上游超时

```toml
[upstream]
connect_timeout_seconds = 10
response_header_timeout_seconds = 30
images_response_header_timeout_seconds = 300
standalone_web_search_response_header_timeout_seconds = 300
stream_idle_timeout_seconds = 90
```

这些 TOML 值只在数据库 `forwarding_policy` 系统设置不存在时用于首次初始化；之后应在 Console
的“系统设置”分类页面修改。按页面分类编辑上游超时、重试与健康、定时测试、会话亲和和 WebSocket 等策略；各类有独立地址。
保存只提交当前页面的编辑，其他分类保留读取时的值；整份配置仍使用同一个 ETag 防止覆盖
并发修改。切换分类前请先保存，未保存编辑不会带到另一个页面。Images generation/edit 使用独立的
`images_response_header_timeout_seconds`，因为上游通常要完成图片处理后才返回响应头。Chat
Completions、Responses 和其他辅助上游请求继续使用 `response_header_timeout_seconds`。
非流式 `/v1/alpha/search` 使用
`standalone_web_search_response_header_timeout_seconds`，因为 provider 可能在返回响应头前执行
多个 Search command。
渠道显式 `response_header_timeout_ms` 始终优先于对应的系统默认值；Images 仍与其他格式共享建连
超时和流空闲超时。所有响应头超时都必须大于有效建连超时。

## 轮换已有数据库密码

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

下面的日志参数位于 `[request_logging]`，请求体参数位于 `[request_limits]`；绝对 Images 目录为容器模板值，源码模板使用 `./data/image-edit-spool`。

默认生产模板使用：

| 参数 | 默认值 | 说明 |
| --- | ---: | --- |
| `[database].max_connections` | 10 | migration、Console、控制面与重载 |
| `request_logging.database_max_connections` | 4 | COPY、投影、结算和健康采样的连接余量 |
| `ingest_batch_size` | 4096 | 本地 spool 到入口表的 COPY 批次 |
| `projection_batch_size` | 2048 | 单 Worker 投影最终宽表 |
| `settlement_batch_size` | 4096 | 一次 claim/聚合的结算行数 |
| `settlement_interval_milliseconds` | 500 | 默认软额度更新延迟 |
| `spool_sync_interval_milliseconds` | 10 | 未准入的拒绝/probe 日志 group-sync 窗口；已准入请求逐条同步 |
| `spool_compaction_threshold_bytes` | 256 MiB | 降低高流量下的截断与同步频率 |
| `spool_max_bytes` | 1 GiB | 本地追加文件、在途预占/追加余量和待核对证据预算 |
| `spool_min_free_bytes` | 64 MiB | 准入所需的文件系统保留空间，另计在途追加余量 |
| `metrics_interval_seconds` | 0 | 默认关闭完整 INFO 心跳；非零值至少为 10 秒 |
| `request_limits.image_edit_body_bytes` | 64 MiB | 单个 multipart edit 总 body 上限 |
| `request_limits.image_edit_file_bytes` | 50 MiB | 单个 image/mask part 上限 |
| `request_limits.image_edit_memory_bytes` | 1 MiB | edit 转入匿名临时文件前的内存阈值 |
| `request_limits.image_edit_spool_directory` | `/var/lib/ai-gateway/image-edit-spool` | Compose 的独立本地 scratch volume |

PostgreSQL 的连接预算至少应满足：

```text
Gateway 实例数
  × (database.max_connections + request_logging.database_max_connections)
  + migration / 管理 / 监控余量
```

默认 PostgreSQL `max_connections=50` 从连接数上可容纳三个默认 Gateway 实例并保留少量管理余量，但这不是多实例部署、插件协调或吞吐能力保证。同一主机上的多个 Gateway 实例必须使用不同的 spool 目录；同一 spool 目录不能被多个进程共享。

不要仅通过增加日志连接数扩大吞吐。当前日志流水线的投影与结算并发受到刻意限制；连接过多会增加 WAL、索引、CPU 和热账户行锁竞争。

日志准入不可写或容量不足时，客户端新请求在派发前返回
`503 request_log_unavailable`。DB 暂时不可用但本地仍有预算时可以继续服务；
容量恢复可自动恢复准入，写/同步失败锁定需保留原 spool 并重启。
存储必须支持预分配及文件/目录同步。逐请求同步增加存储开销，应在实际部署上验证容量。
重启后只有 intent、没有完整终态的请求保留待核对，不自动记零，也不单独冻结用户。
恢复及回滚步骤见[日志耐久化](../development/request-log-durability.md)。

### 日志级别

默认过滤器保留生命周期、控制面、异常和恢复事件，同时关闭逐请求完成日志：

```toml
[observability]
filter = "ai_gateway=info,ai_gateway::application::proxy=warn,tower_http=warn"
```

请求日志流水线的完整 `request_log_metrics` INFO 心跳默认关闭。管理员应优先使用 Console
“系统负载”页面查看当前实例；无 Console 的部署如需周期日志，可将
`request_logging.metrics_interval_seconds` 设置为 `300`–`900`。无论是否启用完整心跳，
backlog 停滞、日志数据库池持续饱和、健康查询不可用及其恢复都会按状态变化输出一次日志。
已有 TOML 的显式值不会随默认值改变；要关闭完整心跳，将该值改为 `0` 后重启。

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

### 区分两类 spool

request-log spool 应位于本地持久磁盘，而不是容器临时层。必须监控剩余空间；磁盘写满是本地耐久
入口的硬失败边界。

Images edit body spool 是独立的短生命周期 scratch 存储，不需要备份，也不能与 request-log
durability 混为一谈。它仍应位于本地磁盘而不是 RAM-backed `/tmp`，否则大 multipart 会重新变成
内存压力。每个文件在打开后保持匿名并随请求引用释放；Console 系统负载的
`image_body_spool.available_bytes`、`active_bytes` 和 `storage_failures_total` 应纳入容量告警。
模型别名重建会短暂同时保留原始与重建 body，Codex data URL 适配还会发生 base64 膨胀；因此应
保守地至少预留并发 edit 上限乘以三倍 `image_edit_body_bytes` 的空间，并额外保留文件系统和
其他服务余量。

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
- 数据库、配置、JWT 密钥、插件目录和 request-log spool 的备份。Images edit scratch spool 不需要备份。

正式生产应优先采用受管理 PostgreSQL，或独立设计 base backup、WAL 归档、恢复演练、监控告警和故障切换。备份必须覆盖 PostgreSQL；spool 主要用于数据库短暂不可用和进程恢复，不能替代数据库备份。

## 调优验证顺序

每次只切换一组参数：

1. 确认错误率、spool pending、ingress backlog 和未结算 backlog 为零或可控。
2. 记录 PostgreSQL WAL、checkpoint、autovacuum、I/O 和锁等待。
3. 运行符合真实响应时长与流式比例的负载。
4. 比较吞吐、p50/p99、日志耐久率和关闭排空时间。
5. 再决定是否保留调整。

仓库中的 Quick/Standard Harness 仍只在明确要求时运行，报告也不等同于生产容量承诺。

## 相关任务

[部署和停止容器](production-deployment.md) · [日志故障与监控](observability.md) · [升级与恢复](upgrades.md) · [保护秘密](security.md)
