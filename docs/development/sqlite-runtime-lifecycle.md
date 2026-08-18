# SQLite 数据库运行时生命周期技术路线

> 状态：提案。本文修订
> [数据库后端抽象与 SQLite 完成总计划](database-backend-completion-plan.md) 的 M2 / I-02
> 技术路线，不描述当前已交付行为。正常配置仍拒绝 `sqlite:` URL。
>
> 外部语义最近核对：2026-08-18。依据为 SQLite 与 Linux 官方文档，以及仓库锁定的
> `sqlx-sqlite` 0.8.6 源码。

## 1. 决策摘要

M2 采用 **Linux protected-path / cooperative-process** 边界，并继续使用 stock SQLx 0.8.6：

1. Gateway 先用 descriptor-relative、no-follow 的 Linux 文件 API 验证目录并安全创建或打开
   数据库，再持有一个独立的数据库 anchor descriptor。
2. anchor descriptor 只用于稳定文件身份、权限检查和进程生命周期 `flock`，**不是** SQLx 的
   数据 I/O descriptor。SQLx 仍按已验证 canonical path 调用 `sqlite3_open_v2`。
3. SQLx pathname reopen 的安全基础是完整祖先链的 DAC 稳定性、Gateway 专属 `0700` 数据库目录、
   已丢弃危险 capability、稳定 mount namespace，以及明确排除 root/相同有效 UID 恶意进程的威胁
   模型；不能把前后 path check 描述为 descriptor adoption。
4. 数据库 inode 使用 versioned canonical-path binding xattr。它用于在普通重启、崩溃恢复和
   path/bind alias 误配置时 fail closed，不是密钥或对恶意相同 UID 的认证机制。
5. `flock` 排除遵守同一协议的第二个 Gateway；SQLite 自己的 POSIX/WAL locks 继续负责数据库内部
   并发。`flock` 不阻止绕过协议的普通 SQLite client。
6. runtime 在创建 lifecycle connection/pool 和每次受控 acquire 前执行 path prevalidation；
   SQLx `after_connect` 对每个新建/重开的物理连接执行立即 post-open gate，`before_acquire` 与
   `after_release` 再覆盖 checkout/recycle。身份边界失败会 poison 整个 runtime，而不只是重试
   一条连接。
7. 不实现 custom SQLite VFS，不使用 `/proc/self/fd` 伪装 descriptor adoption，也不依赖 SQLite
   明确标为 application 不应使用的 `SQLITE_FCNTL_HAS_MOVED`。
8. M2 冻结候选 Linux/文件系统 capability contract 与可复用资格探针；I-12 才在预激活阶段冻结并
   验证准确的官方 image、UID/GID、mount path、volume 类型和 backing filesystem 组合。

这个选择修改了原计划的安全边界，但不修改首发单进程、本机持久文件拓扑，不降低 WAL、
`synchronous=FULL`、权限、link count、事务、日志、结算或激活门禁。

## 2. 原路线为何不可实施

仓库锁定的 `sqlx-sqlite` 0.8.6 最终把 filename 传给 `sqlite3_open_v2`。它可以选择 VFS，
但没有从调用方预打开 descriptor 构造 `SqliteConnection` 的受支持 API。以下方案都不能满足原
descriptor-adoption 描述：

- `/proc/self/fd/<n>` 或 `/dev/fd/<n>` 仍是 pathname reopen，并会引入 WAL/SHM 命名问题；
- `unix-excl` 仍由 SQLite 按 filename 打开，不接管 Gateway descriptor；
- SQLx 的 `vfs` 选项只选择已经注册的 VFS；
- `SQLITE_OPEN_NOFOLLOW` 仍是 filename open，而且 SQLx 0.8.6 没有公开该 flag；
- `SQLITE_FCNTL_HAS_MOVED` 不是 adoption，且 SQLite 官方文档明确不建议 application 使用。

严格保持“所有 SQLite I/O 都来自预打开 descriptor”需要自研或引入 descriptor-aware VFS。完整
VFS 还必须正确实现/委托 WAL shared memory、locking、sync、mmap 和错误边界，会在最敏感的耐久性
边界引入大型 `unsafe`/FFI 子系统。M2 不承担该风险。即使 custom VFS 消除 pathname reopen，也
不能阻止相同 UID 直接改写 DB/WAL 或 ptrace 进程；未来如要把该主体纳入边界，必须先设计专用 OS
principal/user namespace、LSM/ptrace/filesystem 隔离和独立安全里程碑，不能只增加 VFS。

## 3. 威胁模型与候选支持矩阵

### 3.1 受保护对象

本边界保证：

- 不同非特权 UID 不能通过 symlink、目录替换或不安全权限把 Gateway 引向另一文件；
- 同时启动且遵守协议的第二个 Gateway，即使从同路径、编码等价路径或同 inode bind alias 进入，
  也不能取得数据库 anchor `flock`；
- pre-existing hard link、错误 owner/mode、未知文件类型、错误 xattr、远程/未知文件系统或能力
  探测失败均 fail closed；
- 普通崩溃后只有相同 canonical path 可以重新绑定数据库与残留 WAL/SHM；
- runtime 持有锁期间，全部 SQLx pool、lifecycle connection 和 checkpoint 共享一个关闭次序。

### 3.2 明确排除的主体

以下主体已经能够直接改写数据库、WAL、spool 或进程内存，因此不属于本文件安全隔离边界：

- root；
- 具有 `CAP_DAC_OVERRIDE`、`CAP_DAC_READ_SEARCH`、`CAP_FOWNER`、`CAP_CHOWN`、
  `CAP_SYS_ADMIN`、`CAP_SYS_PTRACE`、`CAP_SETUID` 或等价能力的主体；
- 恶意或已攻陷的 Gateway 相同有效 UID / fsuid 进程；
- 能改变 Gateway mount namespace 或底层块设备语义的主体；
- 不遵守 Gateway `flock` 协议的 SQLite client。

这些排除不意味着允许同 UID 启动第二个正常 Gateway。所有受支持启动入口都必须走同一 runtime
owner、path lock、anchor lock 和 canonical-path binding 协议。

### 3.3 候选矩阵

| 项目 | M2 候选能力 |
| --- | --- |
| OS | Linux |
| Kernel | 提供可验证 `openat2` resolve 语义；初始最低为 5.6 |
| 进程身份 | 非 root；effective UID 与 fsuid 一致；危险 capability 已丢弃 |
| 文件系统 | ext4、XFS、Btrfs 的候选集合；每个实际组合仍须通过资格探针 |
| 文件身份 | `st_dev + st_ino`，另记录 mount ID 供诊断与复验 |
| path binding | 支持并持久化 `user.*` xattr |
| active exclusion | Linux nonblocking BSD `flock`，经真实双进程探针验证 |
| live 文件 | 同一主机的本地持久文件系统 |

overlayfs、tmpfs、NFS、SMB/CIFS、FUSE、9p、virtiofs、未知 filesystem magic 和无法验证
xattr/lock/link-count/fsync 语义的组合默认拒绝。Linux 5.6 的可用 syscall 不能可靠辨认所有
idmapped mount；因此 M2 runtime 不虚构自动检测，I-12 必须在准确部署声明并验证非 idmapped mount
（或另行评审一个明确通过的 idmapped 组合）。测试可通过 crate-private policy 使用临时文件系统，
但不能把测试豁免写成生产支持。

M2 的候选集合不是发布声明。I-12 必须在准确的官方 image 与部署示例 volume 中重新运行同一个
探针；实际 backing filesystem 不在通过集合内时不得激活。

## 4. URL、目录与 canonical path

### 4.1 URL grammar

生产 parser 只接受：

```text
sqlite:///absolute/path/to/database.db
```

并执行：

1. 拒绝非空 authority、host、userinfo、port、query、fragment、`:memory:`、空路径和临时数据库；
2. 验证每个 percent escape 后只 decode 一次；
3. 拒绝非法 escape、非 UTF-8、NUL、解码后的 `.`/`..`、重复分隔符和非规范绝对路径；
4. 原始 URL 只用于 parsing；交给 SQLx 的是已验证 canonical filesystem path，不再让 SQLite
   解释客户端 query parameter；
5. `password_file` 与 SQLite 组合直接报配置错误；
6. 未编译 `sqlite-backend` 时返回明确 feature-disabled 错误；
7. 普通 `AppConfig` 在 I-12 前继续先行拒绝 SQLite；M2 只提供 crate-private qualification/test
   入口。

### 4.2 祖先链

从 `/` 的 directory descriptor 开始逐段使用 `openat2`：

- `RESOLVE_NO_SYMLINKS`；
- `RESOLVE_NO_MAGICLINKS`；
- 每一段要求是 directory；
- 每一段 owner 只能是 root 或 Gateway effective UID；
- 任一祖先有 group/world write bit 即拒绝，不提供 sticky-directory 生产例外；
- 最终数据库父目录必须由 Gateway effective UID 拥有且 mode 精确为 `0700`；
- 记录父目录 descriptor，并在 runtime 全生命周期保留。

SQLx 后续仍使用 pathname，因此长期稳定性来自上述 DAC/mount 前提，而不是早先一次 path lookup。
runtime 启动后禁止更改 effective UID、fsuid、mount namespace 或 canonical path。

## 5. 文件生命周期协议

### 5.1 路径创建锁

在保留的父目录 descriptor 下安全打开或创建固定 sibling lock：

```text
<database-leaf>.ai-gateway.lock
```

要求：

- `O_NOFOLLOW | O_CLOEXEC`，创建时使用 `O_CREAT | O_EXCL` 和 `0600`；
- existing 文件必须是相同 UID 拥有的普通文件、mode `0600`、`nlink == 1`；
- filesystem 与父目录相同并通过候选探针；
- 取得 nonblocking exclusive `flock`；
- lock 文件保存定长、版本化的 initialization record。新建数据库前先写入
  `creating(generation, path_hash)` 并 `fsync` lock 与父目录；数据库 label 完成并同步后推进为
  `labeled`，任何 migration 前先同步 `migrating(from,target)`，migration/SQLite reopen 校验完成
  后才推进为 `initialized(target)`。该 record 不是数据库内容备份，只用于区分首次创建/升级阶段
  与未知删除/截断；
- record 使用双 slot、单调 sequence 与 CRC；只接受最高的完整 valid slot，更新 inactive slot
  后同步，避免 power-loss torn write 被误判为授权初始化；
- 文件不在正常关闭时删除，避免 create/unlink race；
- 从 path lock 到 database anchor 的锁顺序固定，关闭时反序释放。

path lock 只串行化同 canonical path 的创建和检查，不是 bind/hard-link alias 排除机制。

### 5.2 数据库 anchor 与稳定身份

持有 path lock 后：

1. existing leaf 用 `O_RDWR | O_NOFOLLOW | O_CLOEXEC` 打开；
2. absent leaf 用 `O_CREAT | O_EXCL`、`0600` 原子创建；
3. 要求普通文件、相同 UID、mode `0600`、`nlink == 1`；
4. 通过 `fstat`/`fstatfs` 取得 `st_dev`、`st_ino`、mount ID 和文件系统能力；
5. 在该 descriptor 上取得 nonblocking exclusive `flock`；
6. 再从父目录 descriptor no-follow 打开 leaf，并确认身份仍与 anchor 一致；
7. **只有取得并复验 anchor `flock` 后**才读取、验证或初始化数据库 xattr；
8. anchor descriptor 与 `flock` 保持到 pools、lifecycle connection 和 checkpoint 全部结束。

同 inode 的并发 bind alias 会竞争同一 `flock`。pre-existing hard link 先因 `nlink != 1`
失败；低层双 descriptor 测试仍要证明候选文件系统上的 inode lock 语义。

### 5.3 Canonical-path binding label

数据库 anchor 使用固定名称的 versioned `user.*` xattr，例如：

```text
user.ai_gateway.sqlite_path.v1
```

payload 是定长、版本化的非 secret 二进制结构：

```text
magic | version | database_generation_uuid | sha256(domain || canonical_utf8_path)
```

规则：

- 新建文件在 SQLx 第一次打开前用 create-only 语义写入 label，并 `fsync` 数据库 descriptor 与父
  目录，再把 path-lock initialization record 从 `creating` 推进为 `labeled` 并同步；
- existing 非空数据库缺失、损坏、超长、未知版本或 path hash 不匹配时 fail closed；
- existing 零长度 unlabeled 文件**不会仅凭长度自动初始化**。只有 path lock 中存在完全匹配的
  `creating(generation, path_hash)` record、文件 metadata 正确且无任何 sidecar 时，才可恢复
  “intent 已同步、label 未完成”的首次创建；其他 unlabeled 文件一律要求 offline
  recovery/import，避免把被截断的生产数据库误认成新库；
- UUID 和 hash 不是身份认证或秘密；它们用于发现操作错误和 path drift；
- 不给 WAL/SHM 写该 xattr；
- ordinary `cp`、rename、bind alias 或缺失 xattr 的 backup output 不是受支持恢复方式；
- I-12 的 offline restore/install 流程负责校验加密 manifest 与数据库/spool generation，在空的
  受保护目标目录安装文件，并为目标 canonical path 写入新 label；不得把 source path hash 原样
  搬到新路径。

label 防止进程退出后从另一 canonical path 错配旧 WAL namespace；运行期间的 active exclusion
仍由 anchor `flock` 提供。

### 5.4 Initialization state table

只有下表列出的状态可自动推进；其他组合全部 fail closed：

| Path-lock record | Database leaf | 动作 |
| --- | --- | --- |
| missing | missing 且全部 SQLite sidecar absent | 创建 lock，写/同步 `creating`，进入首次创建 |
| missing | any existing file | 拒绝；只能走 offline enrollment/restore |
| malformed/unknown | any | 拒绝 |
| `creating` | missing 且全部 SQLite sidecar absent | 用同一 generation/path hash 创建 DB |
| `creating` | zero-length unlabeled、metadata 正确且无 sidecar | 写 label 并推进 `labeled` |
| `creating` | matching labeled DB 且全部 sidecar absent | 复验后推进 `labeled` |
| `creating` | nonzero unlabeled 或 label mismatch | 拒绝 |
| `labeled` | matching labeled DB 且全部 sidecar absent | 先同步 `migrating(none, target)`，再打开 SQLite 并运行幂等 migration |
| `labeled` | missing/unlabeled/mismatch | 拒绝 |
| `initialized(expected schema generation)` | matching labeled、nonzero DB | 正常启动 |
| `initialized(older schema generation)` | matching labeled、nonzero DB | 先同步 `migrating(from, target)`，再运行 forward migrator |
| `initialized(newer schema generation)` | any | 在任何 migration/write 前拒绝 binary downgrade |
| `initialized` | missing/zero-length/unlabeled/mismatch | 拒绝，不能重建空库 |
| `migrating(from, target <= binary generation)` | matching labeled DB | 幂等恢复/继续 migration；成功后同步 `initialized(target)` |
| `migrating(_, target > binary generation)` | any | 在打开 SQLite 前拒绝旧 binary |
| `migrating` | missing/unlabeled/mismatch | 拒绝 |

所有 record transition 都写 inactive CRC slot 并同步。`initialized` 至少记录 generation/path hash
与 SQLite schema/migration generation。missing/zero/unlabeled DB 的任一 `-wal`、`-shm` 或
`-journal` 都使状态 fail closed，绝不在残留 sidecar 旁创建空 DB。`migrating` 必须在任何 forward
migration SQL 前写入并同步；因此 crash 发生在 migration commit 与 record update 之间时，同版/
新版 binary 会安全重跑幂等 migrator，旧版 binary 在打开前被 target generation fence 拒绝。
offline restore/install 必须在同一个 path lock 下按
`creating → labeled → migrating → initialized` 重新建立目标 record/label，不能混用 source
lock 文件。

### 5.5 Stock SQLx pathname open

完成上述检查后，SQLx 使用：

- 已验证 canonical path；
- `create_if_missing(false)`；
- private cache；
- built-in default VFS；
- `min_connections(0)`；
- 不接受 URL query、custom VFS、`immutable`、`nolock` 或 path indirection。

SQLx 没有 per-physical-connection pre-open hook，因此 contract 明确分为：

1. runtime 创建 lifecycle connection/pool 前，以及每次调用受控 acquire wrapper 前，先 no-follow
   读取 path leaf，并与 anchor 的 `st_dev + st_ino`、owner、mode、link count 和 path label
   比较；
2. `after_connect` 在每个物理 open 后立即执行同一 identity/file gate；
3. 连接通过 `PRAGMA database_list` 确认 SQLite 报告的 `main` filename 与 canonical path 一致；
4. connect options 在 post-open gate 前不得执行 journal-mode、migration 或其他持久 mutation；
   connection-scoped policy 只在 gate 内设置。

这是一项 protected-path 推论：在全部祖先不可由边界内攻击者替换的前提下，前后身份一致说明
SQLx pathname open 进入预期文件。它不声称读取了 SQLx 内部 file descriptor，也不隔离排除范围内
的 root/相同 UID 恶意 race。

任何 identity/path/label 失败都会 poison `DatabaseRuntime`、拒绝后续 checkout 并触发受控关闭；
不能只丢弃一条连接后无限重试。

### 5.6 WAL、SHM 与 rollback journal

主数据库同目录下至少检查：

- `<db>-wal`；
- `<db>-shm`；
- `<db>-journal`；
- Gateway path lock。

existing sidecar 必须是相同 UID 拥有的普通文件、mode `0600`、`nlink == 1`，并位于同一候选文件
系统。Gateway 不删除未知、损坏或 hot WAL/journal 来“修复”启动。

启动顺序允许 SQLite 在 lifecycle connection 打开时恢复合法 hot rollback journal，再切换/确认
WAL。lifecycle connection 保持打开后，runtime 记录本次生命周期中出现的 WAL/SHM 身份；连接 gate
在不干扰 SQLite 合法首次创建的前提下验证其后没有替换。sidecar identity 异常 poison runtime。

checkpoint 失败、进程崩溃或强制终止都保留可恢复 WAL。恢复时必须继续使用 path label 对应的同一
canonical path。

## 6. 唯一 `DatabaseRuntime`

### 6.1 Owner 与 profile

`DatabaseConnectOptions` 不再 `Clone`，只能被消费来创建一个 non-cloneable
`DatabaseRuntime`。runtime profile 是闭合枚举：

- `Serve`：lifecycle connection、control pool、request-log pool、migration、metrics 和完整关闭；
- `ManagementCommand`：bootstrap/reset 使用 lifecycle connection 与一个 control pool，不启动
  request-log pool 或后台 worker；
- `Qualification`：crate-private 文件型 SQLite 测试；
- `InMemoryTest`：crate-private，不能从 TOML 构造且不声称具备文件安全保证。

`main`、serve、bootstrap、reset 和 worker 不得再各自调用 `connect_pool` 或 migration。

### 6.2 闭合枚举

以下 database lifecycle 类型内部改为私有 `Postgres | Sqlite`：

- `DatabaseConnectOptions`；
- `DatabasePool`；
- `RepositoryTransaction`。

以下 public repository 名称先成为中立 facade：

- `AuthRepository`；
- `ControlPlaneRepository`；
- `RequestLogRepository`。

pool、repository 和 transaction 都携带：

- backend；
- runtime identity；
- logical pool identity；
- `ControlPlane | RequestLog` role；
- transaction intent。

错误 backend/runtime/pool/intent 配对在执行 SQL 前返回 `RepositoryError::BackendMismatch`。
SQLite write transaction 还持有 shared writer permit，直至 commit/rollback 已被确认或物理连接
已完成 graceful close。

M2 保持 M1 的 104 个公开 repository 签名和全部 PostgreSQL 行为。三个 facade inner 在所属
里程碑完成完整 SQLite contract 前**不提供可构造的 SQLite repository variant**，而不是加入
capability-unavailable method stub。现有 `SqliteRuntimeConfigRepository` 和
`SqliteAuthRepository` 暂时作为 feature-gated test foundation 保留，但 architecture gate 禁止
生产 application/HTTP/MCP/worker 导入；M3/M4 将其正式接入 facade，M10 删除残余直接导出。

logical role 不是按 repository 名称机械限制：Console request-log 查询可显式使用 control pool；
ingest/projection/settlement mutation 只能使用 request-log pool。方法到允许 role/intent 的表由
M2 固定，未列出的 cross-role 组合返回 `BackendMismatch`，从而保持当前 PostgreSQL Console 查询
压力域而不放宽写入。

## 7. SQLite 连接与 writer policy

### 7.1 连接 gate

lifecycle、control 和 request-log 每个物理连接在 `after_connect` 执行同一个 post-establish
gate。stock SQLx 0.8.6 在 hook 前会执行其内建 `PRAGMA foreign_keys=ON`；这是 audited、
connection-scoped 且不写 schema/data 的唯一 pre-hook SQL 例外。M2 禁止 extension、collation 和
其他 connect-option pragma，并以精确版本/source guard 固定该 initializer：

1. 作为第一个 **Gateway-controlled** post-establish operation、在任何其他 hook SQL 或可能失败的
   文件 gate 前，通过 pinned `libsqlite3-sys` 的小型封装设置并 read-back
   `SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE=1`，防止 pool close 绕过显式 checkpoint admission；
2. 验证 runtime 尚未 poisoned；
3. 复验 path/anchor/label 与适用 sidecar；
4. 设置 `foreign_keys=ON`；
5. 设置 `synchronous=FULL`；
6. 设置固定 `busy_timeout`；
7. lifecycle 已初始化 WAL 后，确认 `journal_mode=WAL`；
8. 确认 `locking_mode=NORMAL`；
9. 独立 read-back 每一项；
10. 再次复验 path/anchor/label。

若第 1 步无法设置并确认，runtime 进入 non-unwinding fatal termination；不能让该 SQLite handle
走可能触发 automatic close-checkpoint 的 destructor 后继续同一进程。若 SQLx establish 在 hook
前失败，startup 失败且 fatal lease 保持到 SQLx worker/close 真正结束或 process termination；
该窄 pre-gate 路径可能执行 driver close-checkpoint，因此只记录为 startup recovery outcome，
不宣称受显式 WAL-size admission 控制，也绝不继续 serve。

M2 对该 FFI 边界精确固定 `sqlx`/`sqlx-sqlite` 0.8.6 与 `libsqlite3-sys` 0.30.1；升级必须作为
显式审查改动，同时验证 workspace 只有一份 SQLite linkage、常量/API 可用且 runtime read-back
通过。直接 FFI 只允许
`SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE` 这类公开、稳定且有窄 safe wrapper 的调用；不读取 private VFS
layout。SQLx pool 必须设置 `min_connections(0)` 且关闭 `test_before_acquire`。所有 repository
acquire 都经过 `DatabasePool` wrapper，禁止 raw `SqlitePool` executor、`try_acquire`、
直接 `begin`、`detach` 或 `leak`。

SQLite controlled acquire 由 runtime-owned acquisition task 执行，而不是直接在调用方可取消的
future 中调用 `pool.acquire()`：

- caller 通过 bounded channel 请求 connection；
- acquisition task 完成 prevalidation 与 SQLx acquire，不因 caller future 被取消而中断；
- caller 仍在等待且 runtime 未 poisoned 时才通过 one-shot 交付 `PoolConnection`；
- caller 已取消或 poison 已发生时，task 对取得的 connection 执行 `close().await`，确认 graceful
  close 后才继续；
- shutdown 先停止接收 acquire request，并 join 全部 acquisition tasks，再进入 pool close。

固定检查点：

- controlled acquire wrapper：调用 SQLx 前做 path prevalidation，返回连接后再次读取 poison；
- `after_connect`：每个新建或替换的物理连接做 post-open gate；
- `before_acquire`：每个 idle connection checkout 前重复 gate；
- `after_release`：每次普通 recycle 前重复 gate。

新连接不经过 `before_acquire`，内部连接也不保证经过 `after_release`，所以不能互相替代。hook
内部捕获 gate failure 并记录 poison，不把 failure 直接返回给 SQLx 的 `close_hard` 路径；
controlled wrapper 在把连接交给 repository 前观察 poison，并显式 `close().await`。`after_release`
发现 failure 时让连接留在 poisoned pool，shutdown supervisor 随即 graceful-close 整个 pool。
任何无法证明 worker 已停止的异常路径都保留/leak anchor lease 到进程退出。

### 7.2 公平 writer coordinator

两个 SQLite logical pool 共享一个 runtime-wide FIFO writer coordinator：

- `ConsistentRead` 不取得 writer permit；
- `ManagementWrite`、`RequestLogWrite` 和 `Settlement` 在 checkout 与
  `BEGIN IMMEDIATE` 前排队；
- permit 只有在 commit/rollback 已 await 成功，或 physical connection graceful-close 已确认后
  才释放；
- begin/commit/rollback future cancellation、terminal failure 或 active transaction 的
  unexpected drop 会 poison runtime、关闭 writer admission，并把 permit 转移到 shutdown
  supervisor；不能因 SQLx `Transaction::drop` 只是排队 rollback 就立即释放；
- 若 graceful cleanup 也不能确认，permit 与 anchor lease 保留到进程退出，外部 supervisor
  forced kill 后由 OS 释放；
- request-log batch 不能预订多个未来 permit 或在 release 前重新排队；
- fairness 测试必须同时证明持续日志负载下 control writer 有界前进，以及反向负载下日志 writer
  有界前进。

coordinator 表达单 writer 容量；两个 pool 的 max connection 不能在指标或文档中被解释为两个
并行 writer。

## 8. Migration、worker 与关闭

### 8.1 启动

文件型 SQLite 的固定顺序：

1. 验证 process identity/capability 与候选 filesystem；
2. no-follow 验证祖先和最终父目录；
3. 取得 path lock；
4. 新建时先同步 path-lock `creating` record；
5. 创建/打开 anchor 并取得 inode `flock`；
6. 锁后复验 path identity，再初始化/验证 label；
7. 比较 record 与 binary schema generation；需要 migration 时先同步 `migrating(from,target)`，
   newer target 则在 SQLite open 前拒绝；
8. 打开并 gate lifecycle connection，设置/验证 `NO_CKPT_ON_CLOSE`；
9. 自动恢复合法 journal，设置/确认 WAL；
10. 在 lifecycle connection 上只运行一次 `SQLITE_MIGRATOR`；
11. 验证 DB/WAL/SHM 与 label，并同步 `initialized(target)`；
12. 创建 control/request-log pools；
13. 第二次完成全部 gate 后才开放 repositories、启动 worker 或 publish snapshot。

PostgreSQL 由同一 enum runtime 执行一次 `POSTGRES_MIGRATOR`，保持现有 pool 容量、application
name、isolation 和行为。

### 8.2 Owned background tasks

任何持有 repository/pool 的 task 都必须有 runtime owner 可见的 cancellation/join handle。M2
至少覆盖当前 detached control-plane reload 与 Connector report 路径，并用架构测试禁止新增
持库的裸 `tokio::spawn`。listener 应在 worker 启动前完成 bind，或由 startup rollback guard
保证任一后续 early return 都进入数据库关闭流程。

`DatabaseRuntime` 创建时同时建立 owned shutdown supervisor；supervisor 持有 lifecycle connection、
pools 和 lock lease，application 只取得受控 handle。正常 serve/CLI 每个 return path 都显式请求并
join supervisor。owner 在未完成 shutdown 时被 drop 会 poison runtime 并让 supervisor 继续关闭，
不能直接 drop locks。

构造 cancellation 也必须封闭：同步取得 parent/path/anchor leases 并完成 label state 检查后，在
**第一次 SQLx await 前**启动 supervisor 并把 leases 移交给它；lifecycle connection 和 pools 由
supervisor 内部创建/登记，不在 caller future 中短暂持有。`DatabaseRuntime::open` caller 被取消
或结果 receiver 被丢弃时，supervisor 仍按逆序完成已登记资源的 graceful cleanup；每个构造 await
point 都有 cancellation/fault test。

所有 SQLite lock leases 还登记到 process-lifetime fatal lease vault；只有 supervisor 证明 pools、
lifecycle connection、acquisition/cleanup tasks 和 checkpoint 全部结束后才能从 vault 移除。任何
supervisor/acquisition task panic 或 `JoinError` 都 poison runtime、保留 vault leases 并进入
nonzero process termination；panic unwind 不能成为释放 `flock` 的路径。

### 8.3 关闭与 checkpoint

固定顺序：

1. 停止新 HTTP/WebSocket upgrade 与请求；
2. drain 当前请求；
3. 停止并 join control-plane、Connector、probe、Codex 和其他 producer；
4. 按 spool → ingest → final → settlement 次序 drain request-log worker；
5. 阻止新 checkout；fatal identity/path/label/sidecar poison 时立即停止新应用工作，只允许已取得
   连接 rollback 或 graceful-close；已经 dispatch 且不能撤回的 commit 只等待其真实结束，不再发起
   任何后续 SQL；
6. 关闭 request-log pool，再关闭 control pool；
7. 在仍持锁的 lifecycle connection 上执行 checkpoint；
8. 关闭 lifecycle connection；
9. 复验文件状态，释放 database anchor lock，再释放 path lock 和 parent descriptor。

identity/path/label/sidecar poison 使用更严格的 fatal transition：立即停止 request admission 和
全部 producer，不再等待普通业务 drain；尚未 dispatch terminal 的 transaction 只允许 rollback/
connection close，不得再 commit 或开始新 SQL；已经 dispatch 且不能撤回的 terminal 只等待真实
结束。若在 deadline 内不能证明 quiescent，进程进入
nonzero forced-termination 路径，lock lease 保持到 OS 终止。

pool 或 lifecycle close 超过 deadline 表示“shutdown 仍 pending”，不是允许继续 checkpoint 或
释放锁。supervisor 保留（必要时故意 leak）全部 permits/anchor/path locks 到进程退出；调用方不得
在同一进程恢复服务。

每条 SQLite connection 在 `after_connect` 通过 pinned、窄 FFI wrapper 设置并验证
`SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE=1`，避免 pool/lifecycle close 自动执行未纳入 admission 的最后
checkpoint。stock SQLx future 被 timeout/drop 也不代表其 worker thread 已停止，因此 M2 不虚构
“任意 kernel I/O 都可硬取消”的保证。可实施的 bounded checkpoint 定义为：

- pool 全部关闭后执行；
- checkpoint connection 使用零 lock-wait；
- 先运行 `PASSIVE` 并记录 busy/log/checkpointed frame；
- 只有 `busy == 0 && checkpointed == log` 且 dispatch 前剩余预算允许时才尝试 `TRUNCATE`；
- 超过签入的 WAL-size admission 上限时跳过 shutdown checkpoint，保留 WAL 并报告 typed 状态；
- budget 只在调用前决定是否 dispatch；一旦 SQLite checkpoint 已开始，锁保持到调用真实结束；
  外部 supervisor 的 forced kill 是最终
  硬 deadline，OS 关闭 descriptor 后由下次同路径启动恢复 WAL；
- 任何路径都不得为满足 deadline 手工删除 WAL/SHM、提前释放锁或声称 checkpoint 成功。

## 9. Errors 与 metrics

生命周期增加 typed、后端中立分类：

- invalid/unsafe path；
- unsupported filesystem/capability；
- path lock held；
- database identity lock held；
- owner/mode/link-count violation；
- canonical label missing/malformed/mismatch；
- identity/path/sidecar changed；
- connection policy mismatch；
- migration failure；
- writer busy/timeout；
- pool/lifecycle shutdown pending/fatal；
- checkpoint complete/partial/busy/skipped/failure。

SQLx extended result code 只在 adapter 分类；application/HTTP 不解析错误字符串。错误、日志和 metrics
不输出数据库完整 path、inode、xattr payload、凭据或数据库内容。

M2 提供：

- backend 与 logical pool role；
- size/idle/capacity；controlled acquire wrapper 记录 queue depth、wait duration 与 timeout，
  `acquire pressure` 明确定义为 queued acquires 与 utilization，不声称来自 SQLx 隐藏 callback；
- connection gate/reconnect/recycle 结果；
- SQLite extended `BUSY`/`LOCKED` 计数，以及 `BEGIN IMMEDIATE` 的 elapsed wait；不声称
  `busy_timeout` 能报告真实 busy-handler wait；
- writer queue depth、wait、hold 与 transaction duration；
- migration 结果；
- checkpoint attempt、mode、frame count、duration 与结果；
- runtime poisoned/shutdown 状态。

M6 再把 request-log backlog/health 与这些指标一起接入 `/system/load`。

## 10. M2 测试与 fault matrix

### 10.1 Parser 与 feature

- PostgreSQL URL 回归；
- SQLite 严格 grammar、单次 decode、所有 authority/query/fragment/memory/temp 反例；
- feature-disabled 与 `password_file` 矩阵；
- normal `AppConfig` 继续拒绝 SQLite；
- crate-private file/in-memory constructor 可见性。

### 10.2 文件与 capability

- 每级 ancestor symlink/magic-link、错误 owner/mode、writable ancestor；
- parent、path lock、DB、WAL、SHM、journal 的错误类型/owner/mode/link count；
- ext4/XFS/Btrfs classifier 与实际 capability probe；
- xattr create/read/reopen persistence、`ENOTSUP`、missing/malformed/version/path mismatch；
- path-lock 双 slot/sequence/CRC 的 torn-write recovery，以及无 matching `creating` record 的
  零长度 unlabeled 文件 fail closed；
- initialization state table 每个 valid/invalid 组合，包括 `initialized + missing/zero DB`、
  labeled DB + missing/corrupt/generation-mismatched lock record 和 restore 新 generation；
- `migrating(from,target)` 在 migration SQL 前持久化；migration commit 后 record update 前 kill
  可由同版/新版恢复，而旧版 binary 在 SQLite open 前拒绝；
- fresh-file 初始化每个 fsync cut point 的 kill/restart；
- residual WAL、SHM、hot journal、checkpoint failure 和只读/满盘；
- unknown/remote/overlay/tmpfs/capability-unverifiable fail closed；idmapped mount 由 I-12 准确
  deployment attestation/qualification 处理，M2 不假装仅靠 Linux 5.6 runtime 自动辨认。

文件系统 suite 分层执行：纯 classifier/metadata/fault-injection；当前已证明 filesystem 上的
mandatory real tests；需要 privilege 的 mount/owner/idmap qualification job。无法构造的
wrong-owner/disk-full/mount case 必须输出 capability 结果并运行命名的 syscall fault 或
`SQLITE_FULL` fallback，不能 early-return 当作通过。

### 10.3 双进程与 alias

- 同 canonical path；
- percent-encoded 同路径；
- pre-existing hard link rejection；
- 同 inode bind alias 的 concurrent `flock` contention；
- path label 阻止进程退出后的 alternate canonical path；
- lock 从 startup 一直保持到 pool/checkpoint/lifecycle 全部关闭；
- `SIGKILL` 后 descriptor 自动释放，随后同路径依靠残留 WAL 恢复；
- CI 缺少 bind mount capability 时，必须输出 capability 判定并运行真实双进程、同 inode
  descriptor 的 `flock` fallback；禁止 silent skip。

### 10.4 连接、transaction 与关闭

对 lifecycle/control/request-log 分别覆盖：

- 初始连接；
- 强制关闭后的 replacement；
- caller 在 acquire 每个阶段取消后，runtime-owned task 仍 graceful-close 并可 join；
- construction/acquisition/shutdown supervisor panic 后 fatal lease vault 保持 locks，子进程以
  nonzero termination 结束；
- 故意改变每项 pragma 后 recycle/recheckout；
- hook set/read-back 失败；
- path/label/sidecar mismatch 导致全 runtime poison；
- hook failure 不进入 SQLx `close_hard`，controlled wrapper/graceful pool close 确认 worker 结束；
- wrong backend/runtime/pool transaction pairing；
- shared writer 双向公平；begin/terminal cancellation、failure 和 transaction drop 都在 cleanup
  确认前保留 permit，无法确认时 poison 并保持到进程退出；
- migration exactly once；
- `DatabaseRuntime::open` 每个 SQLx await point 被取消后 supervisor 仍逆序关闭且保持 locks；
- 所有持库 task 可停止并 join；
- 精确 pin/source guard 证明 SQLx pre-hook initializer 只有 audited `foreign_keys=ON`；
  `NO_CKPT_ON_CLOSE` 是每连接第一个 Gateway-controlled post-establish operation；设置失败走
  non-unwinding fatal termination，pre-hook establish failure 不继续 serve 且 lock 保持到
  worker/process 结束；另测 checkpoint complete/partial/busy/skipped/failure；
- 精确 `sqlx` 0.8.6 / `libsqlite3-sys` 0.30.1 单一 linkage 与 FFI compile/runtime gate；
- pool/lifecycle close deadline 败者不能释放 locks，同一进程不能重新 serve；
- lock 只在最后连接和 checkpoint 结束后释放。

### 10.5 Gate

M2 运行默认 PostgreSQL、全部 SQLite suites、`mcp-server` 受影响组合、Rust 1.92 MSRV、
架构/compile-fail、两个真实进程和候选 capability probe。除非实际修改转发路径，否则不触发付费
真实上游 smoke。

## 11. I-02 实施切片

I-02 仍是一个 implementation PR，但按以下可评审 commit/slice 推进：

1. pin reviewed SQLx 0.8.6 / matching `libsqlite3-sys` pair；实现 enum-backed
   options/pool/transaction、runtime profile、typed lifecycle errors 与 PostgreSQL 等价路径；
2. Linux protected-path、capability probe、path label、path/anchor locks 与双进程 harness；
3. lifecycle connection、SQLite pool hooks、writer coordinator、migration 与 metrics；
4. 三个 repository facade 骨架、104 方法 PostgreSQL dispatch、backend/runtime/pool mismatch；
5. serve/bootstrap/reset/worker 唯一 owner、owned task 与有序 shutdown/checkpoint；
6. 全矩阵、architecture gate、MSRV 与当前架构文档。

实现前本设计与总计划修订必须先合并。I-02 完成仍不开放 SQLite 配置，也不宣称官方 image/volume
已经可部署。

## 12. I-12 handoff

M2 签入版本化 capability contract/probe。I-12 预激活阶段必须用**最终候选 artifact**验证：

- 二进制实际编译 `sqlite-backend`；
- runtime 使用不与其他服务共享的专用 principal；容器 UID/GID 到 host owner 的映射、
  fsuid 与 capability set 均明确；
- 数据库 mount source、destination、type、options 和 backing filesystem；
- mount namespace 稳定且不使用未单独通过评审的 idmapped mount；
- entrypoint 创建的 `0700` parent 与 `0600` 文件；
- xattr/flock/link-count/fsync/canonical-path restart；
- 同 volume 的第二个 conforming process/container contention；
- backup API + 加密 manifest + spool/checkpoint generation；
- offline restore/install 在相同和新 canonical path 的 label 行为；
- 容器 recreate、主机重启、崩溃恢复和负面 mount 示例。

只有准确组合通过预激活 gate 后，I-12 才能落最小 URL activation 改动并原样重跑。

## 13. 明确拒绝的捷径

- custom/descriptor-aware VFS 作为 I-02 隐式子任务；
- `/proc/self/fd`、`unix-excl` 或 URI 技巧冒充 descriptor adoption；
- `SQLITE_FCNTL_HAS_MOVED` 或读取 private SQLite VFS struct；
- 把 xattr/UUID/hash 描述为防恶意相同 UID 的认证；
- 仅用 canonical path 或 path lock 声称 active alias exclusion；
- 连接 gate 失败后无限重试而不 poison runtime；
- checkpoint future 被 drop 后提前释放 `flock`；
- silent skip alias/capability tests；
- M2 通过即宣传官方 container/volume 可部署。

## 14. 权威外部依据

- SQLite [`sqlite3_open_v2` 与 open flags](https://www.sqlite.org/c3ref/open.html)
- SQLite [`sqlite3_db_filename`](https://www.sqlite.org/c3ref/db_filename.html)
- SQLite [`sqlite3_db_config` options](https://www.sqlite.org/c3ref/c_dbconfig_defensive.html)
- SQLite [standard file-control opcodes](https://www.sqlite.org/c3ref/c_fcntl_begin_atomic_write.html)
- SQLite [WAL 模式与 checkpoint](https://www.sqlite.org/wal.html)
- SQLite [数据库损坏边界：rename、hard link、filesystem lock](https://www.sqlite.org/howtocorrupt.html)
- SQLite [PRAGMA 语义](https://www.sqlite.org/pragma.html)
- Linux [`openat2(2)`](https://man7.org/linux/man-pages/man2/openat2.2.html)
- Linux [`flock(2)`](https://man7.org/linux/man-pages/man2/flock.2.html)
- Linux [`inode(7)`](https://man7.org/linux/man-pages/man7/inode.7.html)
- Linux [`xattr(7)`](https://man7.org/linux/man-pages/man7/xattr.7.html)
- Linux [`fsync(2)`](https://man7.org/linux/man-pages/man2/fsync.2.html)
- SQLx 0.8.6 [`SqliteConnectOptions`](https://docs.rs/sqlx/0.8.6/sqlx/sqlite/struct.SqliteConnectOptions.html)
- SQLx 0.8.6 [`PoolOptions`](https://docs.rs/sqlx/0.8.6/sqlx/pool/struct.PoolOptions.html)

## 15. 相关文档

- [数据库后端抽象与 SQLite 完成总计划](database-backend-completion-plan.md)
- [数据库与控制面架构](database-architecture.md)
- [数据库 Repository 契约与 M1 方法台账](database-repository-contracts.md)
- [请求日志耐久化流水线](request-log-durability.md)
- [生产配置与容量调优](../user/production-configuration.md)
