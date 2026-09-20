# SQLite 单实例部署与恢复

> 状态：当前。Linux、本地文件系统、协作式单实例；不提供 PG 数据导入、HA 或在线备份。

SQLite 与 PostgreSQL 是完整的替代后端，不是混合存储。Console、API Key、路由、
Codex、拼车、请求日志、不可变计量事实与结算均使用选定的数据库。
公共协议、鉴权和金额规则不变；单写事务不适合通过增加实例数扩容。

## 构建与配置

官方 Docker/发行构建包含 `sqlite-backend`；从源码构建时显式选择：

```bash
pnpm --dir web/console install --frozen-lockfile
pnpm --dir web/console build
cargo build --locked --release --features embedded-console-ui,sqlite-backend
install -d -m 0700 ./data/sqlite
```

在普通配置模板中替换整个 `[database]` 段，其他设置仍沿用
[操作指南](operations.md)和[生产配置](production-configuration.md)：

```toml
[database]
url = "sqlite:///absolute/private/data/sqlite/gateway.sqlite"
max_connections = 5
connect_timeout_seconds = 5
```

- 路径必须绝对、无符号链接别名；URL 不接受 host、query、fragment、内存库或 `..`。
  不支持 SQLite `password_file`；不能把 PG 密码配置带到此段。
- 数据库父目录必须属于运行 UID，模式恰为 `0700`；主文件、身份 marker、WAL/SHM
  必须是同 UID、单硬链接的 `0600` 普通文件。已有权限错误不会自动修复。
- 每个数据库使用专属目录，spool 使用另一个目录；不要把两者嵌套。
- `max_connections` 是**整个数据库**的一条写连接加只读连接总上限，最小 2，
  建议先用 5。`connect_timeout_seconds` 限制连接池获取等待；SQLite busy timeout
  固定 5 秒。`request_logging.database_max_connections` 仅适用于 PG，在 SQLite
  下没有独立日志写池。系统负载页面报告实际共享池状态。
- 仅接受本地 ext 系列、XFS、Btrfs、overlay 和测试 tmpfs。生产不可使用 tmpfs；
  overlay 下层同样必须是本地持久存储。NFS/SMB、多实例共享或绕过应用直接写库不受支持。
- 二进制内置 SQLite 3.51.3，打开前核对实际 native 版本至少为该版本，避免使用
  未修复 WAL-reset 问题的旧链接库。

首次初始化和紧急密码重置仍为 stdin-only：

```bash
./target/release/ai-gateway bootstrap-admin --config ./config/config.toml \
  --email admin@example.com --display-name Admin --password-stdin < /secure/admin-password
./target/release/ai-gateway ./config/config.toml
```

管理员 CLI、迁移、备份与 serve 使用相同文件/进程所有权。
**运行中的 SQLite 实例不能同时执行管理员 CLI**，需先停机；Console 在线管理不受此限制。
`close()` 不释放进程级 lease，只有进程退出才允许另一个进程接手。

## 容器

`docker-compose.sqlite.yaml` 不启动 PostgreSQL，提供数据库、request spool 和
Images 临时上传目录的独立持久卷。先按[生产部署](production-deployment.md)生成 JWT
密钥，复制 `deploy/compose/config.example.toml` 到 `config/config.sqlite.toml`，
把数据库段换为：

```toml
[database]
url = "sqlite:///var/lib/ai-gateway/sqlite/gateway.sqlite"
max_connections = 5
connect_timeout_seconds = 5
```

删除该段的 `password_file`。SQLite stack 不挂载 PostgreSQL 密码；entrypoint 在降权到
UID/GID 10001 前准备 `0700` SQLite volume。不要通过多个容器共享此卷扩容，
不要绕过 entrypoint。默认本地镜像避免误用尚未包含 SQLite 的旧发布：

```bash
docker compose -f docker-compose.sqlite.yaml build gateway
docker compose -f docker-compose.sqlite.yaml run --rm --no-deps gateway \
  bootstrap-admin --config /run/ai-gateway/config.toml \
  --email admin@example.com --display-name Admin --password-stdin < /secure/admin-password
docker compose -f docker-compose.sqlite.yaml up -d
```

已有文件必须保持 UID/GID 10001；entrypoint 不递归修复恢复数据的错误所有权。
已有 PostgreSQL 部署继续使用 `docker-compose.prd.yaml`，其连接和事务语义不变。

## 停机成对备份

数据库中有客户端及上游秘密，spool 中有计量和未决记录；备份同样是敏感数据。
维护前停止全部相关 Gateway 进程，禁止复制后并行运行两个账本副本。
使用相同 UID 和当前工作目录执行：

```bash
ai-gateway backup-sqlite --config /absolute/config.toml \
  --destination /absolute/backups/new-snapshot
ai-gateway restore-sqlite --source /absolute/backups/new-snapshot \
  --destination /absolute/recovery/new-instance
```

目的目录必须不存在，父目录需预先创建、同 UID、不可被其他用户写入。
备份来源为配置中的数据库和**完整** `request_logging.spool_directory`；
后者的相对路径仍按当前工作目录解析。工具：

1. 取得数据库进程/目录锁，拒绝活跃 Gateway；验证身份、完整 migration checksum、
   integrity、外键，然后执行 checkpoint 并关闭 native 连接。
2. 保留文件所有权，取得所有 spool/拼车 writer 锁；复制主库、`.identity`、仍存在的
   SQLite sidecars 及整个 spool（含拼车账本、未知请求、检查点和终结槽）。
3. 写带数据库 UUID、相对路径、文件大小和 SHA-256 的 manifest，fsync 文件与目录。
   manifest 最后落盘；没有完整 manifest 的目录不是可用备份。
4. 恢复先验证清单、文件集合与 checksum，写入新的私有 staging tree，再由同一个二进制
   的短生命周期子进程验证数据库。子进程退出后才原子发布目标目录，避免移动仍被
   native lease 认领的文件。已有目标、损坏文件或未完成拷贝都不会覆盖/发布目标。

恢复后配置：

- 数据库 URL 指向 `new-instance/database/gateway.sqlite`；
- spool 指向 `new-instance/spool`；
- 外部配置、JWT 私钥和代理/网络环境需另行安全保存并配套恢复，不包含在 snapshot 内；
- Images 临时上传目录不属于财务备份，不恢复在途 HTTP 请求。

失败备份目录或 `.sqlite-restore-*` staging 保留用于诊断，**不要用它们启动服务**。
确认失败且无进程占用后由操作者单独处置；工具不自动删除已有数据。
校验和检测损坏，不证明来自可信操作者；仅恢复受保护、可信来源的快照。
这不是在线一致性备份，也不保证硬件突然断电、恶意同 UID/root 或跨机器并行副本的安全。

## 升级与未知结果

首次仅支持新部署和 SQLite 自身连续 schema 升级，不提供 PG → SQLite 导入。
升级前停机成对备份，禁止混跑新旧二进制；SQLite migration 整批原子提交，
未知/超前历史或 checksum 变化拒绝启动，不提供跳过校验开关。

Codex refresh/reset 在 HTTP 前持久化 intent；取消、超时或结果提交失败不自动重试。
放弃的 refresh 可由管理员重新导入正确凭证；活跃操作不允许抢占。
未知 quota-reset **持续拒绝受保护操作**，没有人工确认结果的恢复 API。
不要删除 intent、重试兑换或凭借重启宣称已经退款；保留 provider 证据。
拼车未知费用仍保持 pending，换座、重导或恢复不能补发金额。
详见[Codex 与拼车协议](../development/sqlite-codex.md)。

验收与故障场景见[双后端系统 E2E](../development/system-e2e.md)、
[文件生命周期](../development/sqlite-lifecycle.md)及[独立计量](../development/sqlite-metering.md)。
