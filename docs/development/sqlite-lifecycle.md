# SQLite 文件与迁移生命周期

> 状态：当前。适用于 serve、管理员 CLI、迁移及停机备份恢复的 SQLite 文件所有权。

数据库打开之前先验证目录、文件和进程所有权，再验证数据库身份与迁移历史。
关闭连接池不等于释放文件所有权：进程 lease 必须保留到进程退出。
本页用于判断某个打开、重开或恢复操作是否安全，不是通用 SQLite 文件操作指南。

## 目录

- [信任边界](#信任边界)
- [两层所有权](#两层所有权)
- [数据库身份与非破坏性拒绝](#数据库身份与非破坏性拒绝)
- [原子 migration runner](#原子-migration-runner)
- [验证](#验证)

实现位于 `src/persistence/sqlite/`；编码与直接 SQL 约束见
[约束映射](sqlite-schema-mapping.md)，部署命令见[用户指南](../user/sqlite.md)。

## 信任边界

当前只支持 Linux 下协作式单进程访问。每个数据库使用独立目录：

- 路径为绝对路径；目录及其父路径不能通过符号链接别名访问。
- 目录须属于当前 effective UID，模式恰为 `0700`。文件须为同 UID 的普通文件，
  单硬链接、模式 `0600`。不会自动 chmod/chown 修复已有数据。
- 明确认可 ext 系列、XFS、Btrfs、overlay 和开发测试用 tmpfs；其他文件系统拒绝。
  overlay 的下层也必须是本地存储。tmpfs 不持久化，不能用于生产财务数据。
- `.identity`、`-wal`、`-shm`、`-journal` 不得是 symlink/hardlink 或公共可访问文件。
- 文件权限不防同 UID/root 攻击；不支持不合作的直接 SQLite 写入、运行中复制/替换/
  移动/删除目录或数据库，也不声称消除了任意路径替换的 TOCTOU。

这不是自定义 VFS 或任意文件系统保证；部署须遵守[单实例边界](../user/sqlite.md)。

### 原生 SQLite 版本门槛

当前锁定 SQLx 0.9.0、`libsqlite3-sys 0.37.0` bundled SQLite 3.51.3。
打开文件前调用 `sqlite3_libversion_number`，
拒绝低于 3.51.3 的实际 native 库；不依赖宿主机 `sqlite3` CLI 版本作为证明。
该门槛采用 2026-09-20 核对的
[SQLite WAL-reset 修复说明](https://www.sqlite.org/wal.html)；运行时保证是实际 native 版本检查。
早期基础实现的 3.46.0 不再满足部署门槛；版本检查不替代连接、迁移或恢复契约。

## 两层所有权

`ownership.rs` 在打开驱动前锁定目录 inode 和数据库 inode，使用 `flock`：

1. **进程 lease**：首次认领后，目录、数据库和身份 marker 的描述符由进程级登记表保留，
   **直到进程退出**。`close()` 不释放跨进程锁，也不删除任何锁文件/目录/数据库。
   每个已认领路径保留三个文件描述符；生产组合根认领配置指定的数据库。
   测试进程可能认领多个隔离临时库，测试清理不改变此进程级描述符保留规则。
2. **逻辑 opener**：同一进程同一时刻只能有一个 `SqliteDatabase` opener；
   关闭全部池/句柄后可在该进程重开同一路径。另一个进程必须等待原进程退出。
   池关闭会等待借出的连接归还；丢弃数据库但仍持有连接时仍拒绝第二个 opener。
   Codex provider 操作同样保留逻辑 opener，关闭须等待其释放。

保留进程 lease 是有意的 fail-closed 选择。早期 SQLx 0.8.6 的调查发现 replacement connection
可在注册 native callback 前被取消，后台 SQLite worker 不一定已终止。
仅在 `after_connect` 中挂一个 Arc 不能证明其整个 native 生命周期都受保护。
此外，SQLite 使用 POSIX 文件锁，过早关闭同进程额外打开的数据库描述符也可能释放锁。
本实现不以“取消 acquire 已返回”推断底层文件已经关闭。

`_gateway_owner` collation 只用于 native 生命周期保留逻辑 opener，不是业务排序接口；
跨进程保证由更长寿命的进程 lease 提供。初始化在独立任务中完成；
调用者取消后，已创建的数据库会关闭而非交给调用方。退出 Tokio runtime 前须先关闭数据库。

`acquire_read` / `begin_write` 在操作边界检查目录、主文件、marker 身份和侧文件属性。
检测到替换后，该进程中的 lease 被永久 fence，恢复路径名称也不重新开放。
检查不能拦截已借出连接上的每个 syscall；同 UID 并发修改路径仍在不支持范围内。

## 数据库身份与非破坏性拒绝

驱动打开之前：

- 未认领的新库只能是空文件，且没有现存 WAL/SHM/journal。
- 创建并 fsync `<database>.identity`（规范 UUID），再同步目录；
  已有非空库缺少 marker 时拒绝，**不会先用 SQLite 打开它做探测**。
- 校验已有 marker 的模式、类型、硬链接数和 UUID 表示。

这个顺序避免把 `read_only=true` 错当成“不会触碰 WAL/SHM”：
SQLite 的只读主库打开仍可能创建或重建共享内存侧文件。
带合法 marker 的受管库才进入 SQLite 身份读取，支持从已提交 WAL 恢复 metadata。
marker 不是密码学认证；不防同 UID 伪造整套文件。

首次 SQLite 初始化在一个事务中创建：

- `PRAGMA application_id = 0x41494757`；
- `PRAGMA user_version = 1`，它是身份格式版本，**不是业务迁移编号**；
- `_gateway_sqlite_identity`：与 marker 相同的数据库 UUID；
- `_gateway_sqlite_migrations`：SQLite 专用的版本、description、SHA-256 checksum；
- 禁止修改/删除数据库身份的两个触发器。

重开核对 application ID、身份版本、metadata 表及两个保护触发器的定义、单行 UUID，
并与文件 marker 比对。不采用 `CREATE TABLE IF NOT EXISTS` 掩盖缺失/损坏。
不认识的版本、外部库、UUID 不符或缺失保护一律拒绝。
marker 不完整时不猜测、不重建；若中断发生在 marker fsync 前且数据库仍空，
需要人工确认后处理整套未初始化文件，不能据此删掉可能存在的业务数据。

备份包含 `.identity`、同一停写点数据库及完整 spool。不能只拷贝活跃 `.sqlite` 主文件。
[离线命令](../user/sqlite.md)验证完整历史、身份与清单，恢复到新的 staging 后才无覆盖发布；
不使用部分 staging、不移动已被当前进程认领的库，也不清除未知 Codex intent。
备份不自动升级来源，不提供 PG 导入或将重启当作财务清零。

## 原子 migration runner

`SqliteDatabase::migrate` 接收受信任的、已审查的 SQLite 专属 manifest。
`install_schema()` 使用 `schema.rs` 的完整 manifest，当前为 `0001–0012`：
baseline、guard、Codex intent、凭证/能力拓扑、六操作、固定渠道授权、独立凭证、
请求归属、连接器 ID、插件生命周期和拼车移除。前三项只是早期 baseline，不是当前 schema。
serve 和管理员 CLI 在读取业务记录前执行；备份仅核对当前完整历史，不自动升级来源。

规则：

1. 版本从 1 起连续递增，不接受空描述/SQL 或 `-- no-transaction`。
2. 取得唯一写连接、`BEGIN IMMEDIATE`，核对数据库身份和已应用历史。
3. 历史必须是当前 manifest 的完整前缀，版本、description、SQL checksum 都一致。
   缺号、未来版本、改写历史 SQL 均拒绝；不自动 repair 或跳过。
4. **所有 pending migration 的 DDL、DML、历史记录在同一个事务提交**。
   SQL 失败、外键/延迟约束提交失败或提交前取消，不能留下已应用的前半批。
5. commit hook 在最终提交前拒绝 COMMIT；rollback hook 检测脚本自行回滚/重启事务。
   禁止迁移脚本拆开 runner 的事务；没有 PG 0034/0046 屏障。
6. 结束前再次核对身份和完整历史，再允许唯一最终 COMMIT。
   使用过迁移 hook 的连接关闭而不复用，取消不能污染普通写连接。

需要重建表的指定迁移在专属连接 BEGIN 前关闭 FK；事务结束前执行完整
`foreign_key_check`，检查失败整体回滚。该连接随后关闭，不把关闭外键的状态带回普通池。
这不放宽普通连接的 `foreign_keys=ON`，也不允许迁移文本自行拆事务。

迁移 SQL 不是面向用户的沙箱：不得接收 Console 文本或任意文件执行，
不得 ATTACH 其他数据库、改变连接策略或修改 runner 的 metadata。
baseline/增量 SQL 均须逐项代码审查和真实库测试。

COMMIT 取消/回复丢失不是“明确回滚”；重开后按已提交历史判断。
当前崩溃测试证明已提交历史保留、未提交 DDL 消失，不证明突然断电或硬件损坏恢复。

## 验证

```bash
cargo test --locked --features sqlite-backend --test sqlite_foundation
cargo test --locked --features sqlite-backend --test control_plane_integration sqlite_parity
cargo clippy --locked --workspace --all-targets --features sqlite-backend
```

除连接/快照/Decimal 基础契约外，真实文件库和子进程测试覆盖：

- 两个进程独占、正常关闭后的进程 lease 保留、SIGKILL 后由新进程恢复；
- 借出连接存活、replacement acquire 取消、迁移在执行期间取消；
- 多个 pending migration 的 DDL/DML/history 回滚、延迟约束提交失败；
- 并发迁移只应用一次、重复执行、checksum/description/版本前缀漂移；
- 意外 COMMIT、ROLLBACK/BEGIN、`no-transaction` 拒绝；
- 空库初始化和 UUID 重开、缺失 metadata/身份保护触发器拒绝；
- 完整业务 baseline/guard 安装、重复安装、重开和后续语句失败的整批回滚；
- 真实 PG 对照、财务资格/不可变保护、Codex 派生、软删除和延迟路由约束；
- 无 marker 的 DELETE/WAL 外部库及侧文件内容不变；
- 权限、symlink/hardlink、侧文件别名和主文件替换拒绝。

其中历史 baseline 及 migration 1–4 对照使用冻结夹具，当前业务仓储运行完整 manifest；
不要把旧夹具数字误当成当前表清单。SQLite 生命周期变化还须跑
[系统 E2E](system-e2e.md)的真实 CLI/浏览器与成对恢复，并保留 PostgreSQL 回归。
SIGKILL 只验证进程崩溃，不验证设备断电、同 UID 攻击或硬件损坏。

## 外部依据

2026-09-18 核对：

- [SQLite 文件损坏边界](https://www.sqlite.org/howtocorrupt.html)：POSIX 锁、额外文件描述符、
  路径修改和运行中复制的风险，不可由简单 sidecar lock 自动消除。
- [SQLite WAL 只读边界](https://www.sqlite.org/wal.html#read_only_databases)：
  主文件只读不等于整个侧文件集合不会变化。
- [SQLite 延迟外键](https://www.sqlite.org/foreignkeys.html#fk_deferred)：
  约束可在 COMMIT 时失败，runner 必须覆盖这个失败点。
