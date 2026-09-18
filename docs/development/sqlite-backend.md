# SQLite 双后端实施

> 状态：部分实现。基于 `081d8c9`（第二阶段 PR #177）；S1 已提交，
> S2 已提交为 `65d8fee`；S3 已实现认证、会话、管理员操作和普通控制面双后端分派。
> 尚不能用 SQLite 启动 Gateway。最后核对：2026-09-18。

前置工作见[持久化边界](persistence-boundaries.md)、
[操作接口](persistence-interfaces.md)和[独立计量事实](independent-metering.md)。
本页是第三阶段实施入口，不将之前阶段的 PostgreSQL 验收当作双后端验收。

## 已确认范围

- 保留 PostgreSQL，SQLite 最终支持全部现有功能，包括 Console、普通渠道、Codex、
  拼车、计量结算及查询；不是“只有控制面使用 SQLite”的混合部署。
- SQLite 面向单实例、本地文件系统；不承诺 HA、网络文件系统或多实例共享文件。
- 分可验证切片实现。首版只支持新建部署及 SQLite 自身后续 schema 升级，
  不提供 PostgreSQL 数据导入，也不改写历史 PG migration。
- 保持协议、权限、ETag、金额精度、独立事实/回执、日志准入及 spool/WAL 保证。
  不引入 ORM、通用 CRUD/UoW、双结算入口或内存丢弃队列。
- 启用前所有业务链路必须完整；不支持的后端必须在配置验证时拒绝，
  不能先启动后在某个请求上发现仓储未实现。

## S1–S3 当前实现

`sqlite-backend` 是非默认 Cargo feature，仅编译
`src/persistence/sqlite/`、仓储分派、文件库契约和 PG 对照测试；不改变生产组合根、
TOML、配置模板或发行构建。即使启用该 feature，`AppConfig::validate` 仍拒绝 `sqlite:`。

`SqliteDatabase` 直接使用 SQLx SQLite 驱动，不包装现有 PG 仓储：

- 接受绝对文件路径；S2 增加 Linux 私有目录、文件身份和协作式进程锁验证，
  见[文件与迁移生命周期](sqlite-lifecycle.md)。测试使用独立临时文件库。
- 一个写连接、最多四个只读连接；参数是开发基础的固定策略，不是新增配置项。
- 每个新物理连接配置并核对 WAL、`synchronous=FULL`、外键、recursive triggers、
  关闭 read-uncommitted 及 5 秒 busy timeout。读连接另外以只读方式打开并启用 query-only。
- 写事务由 SQLx `BEGIN IMMEDIATE` 管理，持有唯一写连接；提交、显式回滚、drop/cancel
  后回滚均使用 SQLx 事务对象。没有手写 COMMIT 后归还“看似干净”的连接。
- 读写池是同一文件的访问策略，不是两份数据；WAL 读快照可以跨越写事务提交。
- 原始 SQL 句柄仅供持久化实现使用，不是新的应用层事务接口。

`SqliteDecimal` 将 `Decimal` 绑定为规范化十进制 TEXT，并只解码可无损、规范往返的 TEXT。
拒绝浮点、整数、BLOB、溢出、精度下溢和非规范表示；可选字段通过
`Option<SqliteDecimal>` 保留 NULL。规范化只去掉无意义尾零，不改变数值。

S2 的 `SqliteAmount` / `SqliteUnitPrice` / `SqliteSharingAmount` / `SqliteTokenRate`
按列精度检查并恢复 SQLx/PG 的显示 scale；数据库 CHECK 使用相同精度规则。
`SqliteDecimal` 仍是通用底层传输，不用于代替这些业务列类型。
这里不执行舍入、聚合或结算；S4 的操作实现仍须遵守既有金额规则。
TEXT 不能用于金额的字典序排序或 SQLite 原生 `SUM`、算术、NUMERIC/REAL `CAST`。

S1 提交为 `5668ea4`。S2 的 `install_schema()` 通过独立的 `0001_baseline.sql`
和 `0002_guards.sql` 一次性安装完整业务 schema。另有身份 metadata、原子 runner 和进程所有权。
S3 仓储分派与双后端契约见[身份与控制面](sqlite-control-plane.md)；
尚无备份命令或完整业务端到端支持。SQLx 仍按路径打开文件，不使用自定义 VFS；
调用方不得移动、替换或删除进程已认领的目录/数据库文件，直到该进程退出。
完整 schema 和归一化写入契约见[约束映射](sqlite-schema-mapping.md)。
当前 bundled SQLite 还需处理[原生版本门槛](sqlite-lifecycle.md#原生-sqlite-版本门槛)，
开发测试通过不能作为生产开放许可。

## 后端语义与实施决策

### 存储和金额

已采用 SQLite STRICT 表：UUID/时间/枚举/JSON 明确编码，布尔和计数用 INTEGER，
金额用 TEXT；所有适配位于持久化内部，业务继续使用既有类型。
S2 已固定这些编码、NULL、默认值和数据库校验；未来仓储必须使用列类型适配器，
不能依赖 PG 自动 cast 或 SQLite affinity，也不能遗漏归一化写入字段。

金额计算保持既有 Decimal 舍入顺序；8/12 位小数、拼车 ToZero 和越界整笔回滚
见[契约基线](persistence-contracts.md)。SQLite 账户更新在独占写事务内读取、
checked Decimal 运算、验证列精度后写回；PG 保留其原生 NUMERIC SQL。
统计采用精确 Decimal 聚合，按有界批次扫描/投影，不把历史全表一次加载到内存。
实现前须对齐 PG 聚合中间精度、最终范围与溢出失败规则；不能以 Rust 单值上限
悄悄替代现有 SQL 聚合语义，或转为浮点绕过越界。

数据库级保护没有迁出数据库。S2 保留 CHECK/FK/唯一键、不可变与跨表触发器，
通过受保护的延迟 FK assertion 保留路由提交时校验。
金额/文本/JSON 函数在每个物理连接注册，未注册连接写入 fail closed。
不能用 TEXT 字符串比较冒充大小比较，也不能把删除 financial facts/receipts 的保护放宽。

### 并发和耐久

所有 SQLite 写路径最终共享同一个写池，不能给认证、日志和 Codex 各建独立写者。
读仓储使用事务快照，prepared change 保持“修改→完整候选→编译→审计提交→发布”；
应用仍只接触[专属操作对象](persistence-interfaces.md)。

PG 行锁/advisory lock 与 SQLite 单写事务不机械对应：

- 账户结算原子包含回执、余额、额度和 pending 删除，UUID 重放不重复扣款。
- Codex refresh/reset 现有锁跨越外部 HTTP；SQLite 全局写锁不能照搬成长网络事务。
  S5 必须先设计 generation/专属操作锁及必要的 lease/fencing，覆盖取消和外部结果不确定，
  不自动重试 token 轮换/兑换，不在此基础切片偷偷改变 PG 行为。
- 拼车单实例所有权须覆盖启动、CLI 管理命令、全部池及恢复。
  进程内单写连接不是进程独占证明；busy timeout 也不是重试授权。
- SQLite 的 durable ingress 使用事务批量 INSERT，不模拟 COPY；
  成功提交才允许 checkpoint，事实/ready 同事务，投影后 ack，
  回执与事实不依赖 `request_logs`。
- FULL/WAL 是拟定最低持久化策略，不代表已经验证断电或任意硬件损坏。
  保留未决 intent、未知费用和冲突证据，不按重启时的当前价格重新计价。

### 文件、迁移和配置

S2 先明确支持的平台/文件系统、私有目录权限、进程独占锁、
数据库文件/侧文件的身份与路径替换风险，再接入 serve/bootstrap/reset。
禁止先宣称一个 sidecar flock 能覆盖任意别名或替换攻击；也不预先承诺自定义 VFS。
旧实验分支不作为当前 schema 或安全保证的来源。

SQLite 使用独立 migration 历史：新库 baseline 对齐当前完整 schema（包括 0063），
不执行 PG SQL、不借用 PG 版本号假装已经跑过 PG 升级。
在写锁内检查历史/checksum，连续待执行 migration 在同一事务提交；
失败、取消、并发启动、未来版本和外部数据库误选必须有拒绝/回滚测试。
后续每次 schema 变更同时维护两种后端的迁移与契约。

S6 才确定并同步 TOML、两个配置模板、compose/容器目录、CLI 和运维文档；
不静默把现有 PG 的连接数或密码配置解释成 SQLite 参数。

## 实施切片和退出条件

| 切片 | 交付与必须通过的验收 | 状态 |
| --- | --- | --- |
| S1 基础 | 可选驱动、文件读写连接、drop/cancel 回滚、TEXT Decimal 无损往返、生产配置仍拒绝 SQLite、CI 收集测试 | 已实现，测试范围见下 |
| S2 schema 与生命周期 | 完整约束映射和 baseline、原子迁移、类型编码、单实例所有权、错误分类、路径/身份/关闭恢复负向测试 | 已实现；包括真实 PG schema/编码对照和完整 baseline 批次回滚 |
| S3 身份与控制面 | 认证/会话/管理员操作、完整配置读写、编译失败和审计失败回滚、权限/版本/软删除/路由约束双后端契约 | 已实现；CLI 底层 bootstrap/reset 操作已双后端，SQLite CLI 配置/组合根开放仍属 S6 |
| S4 事实与结算 | ingress/计量/独立投影/回执/pending、精确聚合、重复与冲突重放、未知费用、取消/崩溃与提交回复丢失 | 待实现 |
| S5 Codex 与拼车 | OAuth/配额/paired projections、长事务替代协议、单实例 WAL 恢复、窗口费用及授权隔离 | 待实现 |
| S6 可部署验收 | 组合根与配置开放、真实浏览器/CLI 系统链路双后端矩阵、SQLite 故障/备份恢复、运维说明与发行构建 | 待实现 |

S3–S5 的公共仓储使用显式后端分派和后端私有行映射，保留既有窄操作接口；
不要求业务理解 SQLx Any、方言或原始 executor。不以空 SQLite 分支或 Mock 仓储宣称完成。

S3 的共享门面已接线：`AuthRepository::new(PgPool)` 与 `ControlPlaneRepository::new(PgPool)`
保持原签名，另加仅 `sqlite-backend` 可用的 `from_sqlite(Arc<SqliteDatabase>)` 开发构造器；
两者按显式封闭后端枚举分派，保留 `PreparedControlPlaneChange` 等既有应用接口。
PostgreSQL 实现改名为 `PostgresAuthRepository` / `PostgresControlPlaneRepository`，行为不变。
认证全部 24 个方法已完整分派。普通控制面读写、预提交变更、用户设置、自助 API Key、
批量更新、目录同步和审计读取同样已分派。

S5 专属的 Codex 方法当前仍只有 PostgreSQL 实现：`codex_credentials`、`codex_credential`、
`codex_credential_view`、`load_codex_credentials`、`export_codex_credentials`、
`create_codex_oauth_flow`、`codex_oauth_flow`、`cleanup_codex_oauth_flows`、
`set_codex_user_id_if_missing`、`codex_quota_window_history`、`self_codex_quota_credentials`、
`self_codex_quota_window_history`、`persist_codex_quota`、`record_codex_quota_reset`、
`mark_codex_credential_error`、`lock_codex_refresh`、`lock_codex_quota_reset`、
`prepare_codex_credential_{create,update,delete}`、`prepare_codex_credentials_batch`、
`claim_sharing_ledger`、`sharing_groups`。它们统一经单一 PostgreSQL-only 访问器返回
`UnsupportedBackendOperation`（经 `RepositoryError::Storage`/内部错误类别映射），
不返回空成功、不伪装成业务 `Validation`，也不改变任何应用签名。
生产组合根仍拒绝 SQLite 配置，因此该门面不对生产暴露。

正式启用必须覆盖同一套行为契约：金额逐位相等、权限与快照一致、
软删除历史身份可追溯、一份事实/回执/账户效果、日志阻塞时仍结算、未知不记零、
迁移失败回滚、重启/重复重放、配置与所有 CLI 生命周期一致。
PG 的 COPY/锁/升级测试和 SQLite 的 busy/WAL/文件锁/迁移测试分别保留；
现有 PG raw SQL、dump/restore 和容器故障脚本不能直接改名充当 SQLite 测试。

## 当前验证入口

```bash
cargo test --locked --features sqlite-backend --test sqlite_foundation
cargo test --locked --features sqlite-backend --test control_plane_integration sqlite_parity
cargo test --locked --features sqlite-backend --test control_plane_integration sqlite_s3_parity
cargo clippy --locked --workspace --all-targets --features sqlite-backend
```

测试使用真实文件库，覆盖 S1 的连接、快照、Decimal 和配置关闭契约，
以及 S2 的协作式跨进程独占、kill/restart、全部 pending 迁移回滚、checksum/history
漂移、延迟约束提交失败、迁移取消、身份保护和文件别名拒绝。
同时覆盖完整业务 baseline 的安装、重开、整批回滚及直接 SQL 负向约束。
PG 对照验证当前 34 表/401 列、类型、约束名、外键、seed、枚举排序、金额显示和时间量化；
生命周期故障测试还使用最小测试表隔离故障点。SIGKILL 不等于断电测试。
详细边界见[生命周期验收](sqlite-lifecycle.md)。
常规 Rust CI 在既有 PostgreSQL gate 之外执行上述 feature 检查；
没有运行转发压测或付费上游。

## 外部依据

2026-09-18 核对，以下是外部语义，不等于本项目已具备完整后端保证：

- [SQLite 类型与 affinity](https://www.sqlite.org/datatype3.html)：NUMERIC affinity
  可把十进制文本转为 INTEGER/REAL，故本实现使用 TEXT 编解码。
- [SQLite 事务](https://www.sqlite.org/lang_transaction.html)：IMMEDIATE 提前取得写事务，
  仍可能遇到 BUSY；不等同于跨外部服务的原子性。
- [SQLite synchronous](https://www.sqlite.org/pragma.html#pragma_synchronous)：
  WAL/FULL 的同步边界是本实现连接策略依据，实际可靠性仍依赖 OS/硬件。
