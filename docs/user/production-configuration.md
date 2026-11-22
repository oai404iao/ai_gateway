# 配置与容量调优

> 状态：当前。

先从[源码 TOML 模板](../../config.example.toml)或[容器模板](../../deploy/compose/config.example.toml)开始。下文说明哪些值要重启、哪些应在 Console 修改，以及如何给连接池和磁盘留余量。[SQLite](sqlite.md) 使用不同的连接与存储模型。

## 选择配置入口

| 设置 | 修改位置与生效方式 |
| --- | --- |
| 监听端口、数据库、插件目录、请求体上限、日志 spool/批次、观测 | 进程 TOML，重启生效。默认读取当前工作目录的 `./config/config.toml`，可传 `ai-gateway /absolute/config.toml`。相对秘密文件和 spool 路径也按工作目录解析。 |
| 上游超时、重试、被动健康、自动禁用、定时测试、Session 亲和 | TOML 只在数据库尚无 `forwarding_policy` 时初始化。之后改 Console「系统设置」。当前实例保存后发布，其他实例周期重载。 |
| 凭证、接入、渠道、能力、路由、价格和 Key | 在 Console 或其 API 修改数据库配置，随运行时快照生效。 |
| Codex 客户端身份、合成工作区 | Console「插件管理」中编辑[插件设置](connector-plugins.md#3-调整插件设置)。 |
| `AI_GATEWAY_*` 部署变量 | 由 Compose 在创建容器时展开，不是二进制配置。长期设置保存在部署系统或受保护的 Compose env 文件中。 |

## 监听器与请求体限制

数据面默认监听 `127.0.0.1:3000`。Console 使用独立监听器和 JWT 密钥，生产浏览器经 HTTPS 反向代理访问；来源配置见[部署指南](production-deployment.md)。在 TOML `[request_limits]` 按用途设置大小：

| 参数 | 模板值与用途 |
| --- | --- |
| `proxy_body_bytes` | `1 MiB`，限制 JSON 代理请求，包括 Images generation。 |
| `image_edit_body_bytes`、`image_edit_file_bytes` | `64 MiB` 总 body、`50 MiB` 单个 image/mask；调整图片上限无需调高所有 JSON 请求的内存上限。 |
| `image_edit_memory_bytes`、`image_edit_spool_directory` | 超过 `1 MiB` 写入本地匿名临时文件；Unix 目录 `0700`、文件 `0600`。 |
| `console_body_bytes`、`auth_body_bytes` | `256 KiB` 管理请求，`16 KiB` 认证请求；插件包上传有自己的限制。 |

## 上游超时

在「系统设置 → 连接与协议」设置建连、等待响应头与流空闲的超时。默认建连 `10 s`，流空闲 `90 s`。普通响应头等待 `30 s`；图片和独立搜索可能在发响应头前完成主要计算，各自有独立的 `300 s` 默认值（`images_response_header_timeout_seconds`、`standalone_web_search_response_header_timeout_seconds`）。

接入上的显式 `response_header_timeout_ms` 优先，注意它使用毫秒，且有效响应头超时必须长于有效建连超时。系统设置的多个分类共用一个 `ETag`；切换页面前保存，遇版本冲突先载入最新值。

## 轮换已有数据库密码

`POSTGRES_PASSWORD_FILE` 只在初始化空数据卷时设置角色密码。轮换前安排维护窗口，用受保护的 `psql` 会话执行 `\password ai_gateway`，同步更新 Gateway 的 `database.password_file` 指向的文件，再重启依赖该密码的实例。仅替换文件不会更改数据库角色密码；删除卷则会删除数据库。

## 连接与日志容量

每个 PostgreSQL Gateway 有控制面池和独立日志池。模板分别是 `database.max_connections = 10` 和 `request_logging.database_max_connections = 4`。服务端连接预算至少为两者之和乘以实例数，另留迁移、监控和管理员连接。每个 Gateway 的日志 spool 目录必须独占。

日志积压时，先区分入口、投影还是结算变慢。在 `[request_logging]` 中，`ingest_batch_size`、`projection_batch_size`、`settlement_batch_size` 分别对应三个阶段，模板为 `4096 / 2048 / 4096`。增大批次可减少往返，但会拉长事务和锁持有时间。`settlement_interval_milliseconds` 是结算调度间隔（默认 `500 ms`），不是软额度更新时限。先测 I/O、WAL、索引和热账户锁，再考虑增加数据库连接。

### 区分两类 spool

| 目录 | 如何规划 |
| --- | --- |
| `request_logging.spool_directory` | 持久财务入口，使用支持预分配及文件/目录同步的本地存储，与数据库成对备份。`spool_max_bytes`（默认 `1 GiB`）包括在途及待核对余量；`spool_min_free_bytes`（默认 `64 MiB`）是文件系统保留空间。容量不足会阻止新请求派发，见[日志入口故障](observability.md#处理日志入口故障)。 |
| `request_limits.image_edit_spool_directory` | 存在单次请求期间的图片 scratch，不备份。使用本地磁盘而非内存盘。改模型时可能同时保留原 body 和重建 body，Codex 转 data URL 还会膨胀。可先按最大并发 edit 数 × `3 × image_edit_body_bytes` 预留，再核对可用容量与实际峰值。 |

`request_logging.spool_directory` 指向根目录，不是其 `journal-v2/` 子目录。`spool_max_bytes` 只约束当前布局；[旧布局](upgrades.md#请求日志目录切换)的文件仍占磁盘空间，规划可用容量时要另计。

### 日志级别

默认 `[observability].filter` 为 `ai_gateway=info,ai_gateway::application::proxy=warn,tower_http=warn`，保留异常和恢复事件，平时不逐条输出请求完成日志。排障时限时调整相关模块，配置容器或服务管理器的日志轮转。

优先在[系统负载](observability.md#观察当前实例)查看实时压力。无 Console 时，可将 `request_logging.metrics_interval_seconds` 设为 `300`–`900` 以启用周期 INFO 快照；`0` 关闭完整快照，不影响异常状态变化日志。

## PostgreSQL 调优

默认值针对单主机 4–8 GiB 的起步部署。实际值及其 `AI_GATEWAY_POSTGRES_*` 覆盖项见[生产 Compose](../../docker-compose.prd.yaml)。先按内存预算决定数据库缓存与应用批次：

| 主机内存 | shared_buffers / effective_cache_size | work_mem / maintenance_work_mem | ingress / projection / settlement |
| --- | --- | --- | --- |
| 2–4 GiB | 256–512 MB / 1–2 GB | 4 MB / 128 MB | 2048 / 1024 / 2048 |
| 4–8 GiB | 1 GB / 4 GB | 8 MB / 256 MB | 4096 / 2048 / 4096 |
| 16–32 GiB | 4–8 GB / 12–24 GB | 8–16 MB / 512 MB–1 GB | 测试 8192 / 4096 / 8192 |

`work_mem` 可能被同一查询的多个执行节点和并行 worker 同时分配。调优时保持 `fsync`、`synchronous_commit`、`full_page_writes` 与数据校验和开启，以保留提交与恢复语义。

日志表高频写入。如果 dead tuple 持续增加，查 autovacuum 上次运行时间、锁等待与 I/O，再考虑调整阈值：

```sql
SELECT relname, n_live_tup, n_dead_tup, last_autovacuum, last_autoanalyze
FROM pg_stat_user_tables
WHERE relname IN ('request_logs', 'request_log_ingest');
```

新 Compose 数据卷会启用 `pg_stat_statements`；已有卷在数据库启用了 preload 后，可用有权限的数据库角色运行 `CREATE EXTENSION IF NOT EXISTS pg_stat_statements`。检查 `pg_stat_statements` 的耗时与调用次数，并结合 `pg_stat_wal`、`pg_stat_checkpointer` 和 `pg_stat_io` 区分慢查询、checkpoint 与存储瓶颈。

优先使用可靠 flush、低延迟的持久存储。Btrfs NOCOW 应在创建数据库文件前决定，使用后会失去该层数据校验和与压缩。PostgreSQL 18 模板采用 `io_method=worker`、`io_workers=3`；需要调整并发 I/O 或 `random_page_cost` 时，分别测量查询计划、真负载下的延迟、spool/结算积压和停机排空时间，每次只改一组参数。

默认 Compose 是单节点，不包含复制、故障切换、WAL 归档或时间点恢复。容量调整不能替代[备份与恢复演练](upgrades.md)。
