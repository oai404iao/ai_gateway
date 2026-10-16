# SQLite 双后端实现

> 状态：当前。Linux 的 sqlite-backend feature 提供完整后端；官方 Docker/发行构建包含它，
> Cargo 默认仍为 PostgreSQL。部署入口见[SQLite 用户指南](../user/sqlite.md)。

SQLite 是单实例、私有本地文件部署选择，不是只替换控制面的混合模式。
身份、Console、上游拓扑、Codex、独立计量、结算、查询及管理 CLI 都通过同一数据库 owner
工作。它保留 PostgreSQL 的业务契约，不复制 PG 的连接、锁、COPY 或 SQL 方言。

## 目录

- [选择与边界](#选择与边界)
- [连接与事务](#连接与事务)
- [金额与数据库保护](#金额与数据库保护)
- [生命周期、升级与恢复](#生命周期升级与恢复)
- [仓储地图](#仓储地图)
- [验证](#验证)
- [设计与历史验收](#设计与历史验收)

## 选择与边界

- Linux 编译启用 sqlite-backend 后，database.url 接受 sqlite:///absolute/private/gateway.sqlite。
  非 Linux 或未编译该 feature 的构建在配置验证时拒绝 SQLite。
- 私有目录由运行 UID 拥有，权限严格 0700；文件 0600，无别名，
  文件系统及身份检查见[生命周期](sqlite-lifecycle.md)。
- 单实例本地文件；不承诺 HA、网络文件系统或多进程共享文件。
  busy timeout 不是多写者协调或自动重试许可。
- database.password_file 不用于 SQLite，配置不得提供。
  database.max_connections 是 writer + readers 总数；日志独立池配置只对 PG 生效。
- 支持新建部署及 SQLite 自身有序升级，不提供 PostgreSQL 导入。
  不执行历史 PG SQL，也不借用 PG migration 编号。
- 没有 ORM、通用 CRUD/UoW、SQLx Any、运行时 SQL 翻译器或第二套结算权。
  不以空 SQLite 分支、Mock 仓储或先启动后报“未实现”代替完整后端。

## 连接与事务

SqliteDatabase 使用 SQLx SQLite 驱动。所有仓储共享一个 Arc<SqliteDatabase>，
一个写连接，其余只读；生产 open_with_limits 至少两个总连接，
测试 open 保留 1 writer + 4 readers 默认。

每个新物理连接注册/核对：

| 策略 | 契约 |
| --- | --- |
| WAL + synchronous=FULL | 最低持久化策略，不是硬件掉电认证 |
| foreign_keys / recursive_triggers | 约束及派生保护生效 |
| read_uncommitted 关闭 | 不读取未提交状态 |
| busy_timeout=5s | 有界等待，不自动重试外部副作用 |
| reader 只读 + query_only | 读路径不能绕开 writer |
| temp_store=FILE | 排序/staging 使用 SQLite 管理的临时文件 |
| schema 函数/排序规则 | 统一精确类型、金额、JSON、事务时钟和枚举语义 |

写入使用 SQLx BEGIN IMMEDIATE 事务对象；commit、rollback、drop/cancel 回滚由驱动管理，
不手写 COMMIT 后归还“看似干净”的连接。
读仓储需要一致性时使用读事务，WAL 快照可跨越写入提交。
读写池是同一文件的访问策略，不是两份数据，也不向应用暴露原始 executor。

prepared change 保持“变更 → 完整候选 → 应用编译 → 审计提交 → 发布”。
PG 行锁不能机械翻译为长时间 SQLite 写锁：
Codex HTTP 使用短事务 CAS 和耐久 intent fencing，不在全局 writer 下等待网络。
详见[Codex 操作](sqlite-codex.md)。

## 金额与数据库保护

业务继续用既有 UUID、时间、枚举、JSON 和 Decimal。
SQLite STRICT 表显式编码类型；布尔/计数 INTEGER，金额 TEXT，不依赖 affinity。

SqliteDecimal 只接受可无损、规范往返的十进制 TEXT，
拒绝 INTEGER/REAL/BLOB、溢出、精度下溢和非规范表示；
Option 保留 NULL，去无意义尾零不改数值。
业务金额使用专属列适配器：

- SqliteAmount：numeric(24,8)；
- SqliteUnitPrice：numeric(24,12)；
- SqliteTokenRate：numeric(14,4)。

适配器不负责计费舍入；写入按既有规则量化，再校验列精度。
禁止金额 TEXT 字典序、原生 SUM、浮点运算及 NUMERIC/REAL CAST。
账户在唯一写事务读取、checked Decimal 运算、验证精度后写回，越界整批回滚。
统计使用八位最小单位 BigInt 精确中间态，最终进入既有 PG/SQLx Decimal 结果边界；
不静默降低精度或扩张旧 API 金额类型。见[计量与查询](sqlite-metering.md)。

CHECK/FK/唯一键、不可变/跨表触发器及延迟路由提交保护仍在数据库，
未注册函数的连接写入 fail closed。
不以应用断言取代保护，也不弱化事实/回执不可删改。
完整编码和归一化写入契约见[约束映射](sqlite-schema-mapping.md)。

## 生命周期、升级与恢复

当前 SQLx 0.9.0、libsqlite3-sys 0.37.0 bundled SQLite 3.51.3；
打开前核对实际 native 版本，不以系统 sqlite3 CLI 版本代替。
最低门槛和支持文件系统见[原生版本门槛](sqlite-lifecycle.md#原生-sqlite-版本门槛)。

目录/主库 inode 的协作式进程 lease 保留到进程退出，
close 只结束逻辑 opener，不释放跨进程锁。不能移动、删除或替换已认领文件/目录。
这不是自定义 VFS，不防同 UID/root，不宣称消除任意路径替换 TOCTOU。

SQLite 独立 manifest 当前为 0001–0012：baseline/guard、Codex fencing、
身份/能力拓扑、六操作、渠道授权、凭证/归属、原生连接器、插件生命周期和拼车移除。
install_schema 运行完整 manifest，不停在最初的三项。
所有 pending DDL/DML/history 同事务；checksum/版本前缀漂移、未来版本、外部库均拒绝。
重建表的 migration 使用专属连接并检查全部 FK，不向普通池泄露关闭外键的连接。

backup-sqlite / restore-sqlite 是离线数据库、identity 与完整 spool 成对操作。
恢复先验证新 staging，再无覆盖发布；不使用部分 staging、不移动活跃 owner 的库。
未知 Codex intent 和普通未知 usage 均保留，恢复/重启不是财务清零。
配置、权限、CLI 和容器步骤见[用户指南](../user/sqlite.md)，不要复制活跃 .sqlite 主文件当备份。

## 仓储地图

| 主题 | 文档 / 实现 |
| --- | --- |
| 文件、owner、连接、migration | [生命周期](sqlite-lifecycle.md)；sqlite/{mod,ownership,migrations,schema}.rs |
| 类型与直接 SQL 保护 | [约束映射](sqlite-schema-mapping.md)；sqlite/{types,decimal,functions}.rs |
| 身份与控制面 | [控制面](sqlite-control-plane.md)；backend_auth/backend_control_plane、sqlite/auth/control_plane |
| ingress、事实、回执与报告 | [计量](sqlite-metering.md)；backend_pipeline/backend_queries、sqlite/pipeline/queries/aggregate |
| OAuth、quota、外部调用 | [Codex](sqlite-codex.md)；backend_codex、sqlite/codex |
| 离线备份恢复 | sqlite/backup.rs、main.rs CLI、系统 E2E |

公共 new(PgPool) 与 feature-gated from_sqlite 保留同样业务接口；
SQLite 构造器已被生产组合根使用，不再是“仅开发验证”。

## 验证

```bash
cargo test --locked --features sqlite-backend --test sqlite_foundation
cargo test --locked --features sqlite-backend --test control_plane_integration sqlite_parity
cargo test --locked --features sqlite-backend --test control_plane_integration sqlite_s3_parity
cargo test --locked --features sqlite-backend --test control_plane_integration sqlite_s4_parity
cargo test --locked --features sqlite-backend --test codex_capability_integration
cargo test --locked --features sqlite-backend --lib application::codex::sqlite_tests
cargo test --locked --workspace
cargo test --locked --workspace --features sqlite-backend
cargo clippy --locked --workspace --all-targets --features sqlite-backend
```

当前结构对照使用 tests/fixtures/canonical-schema-inventory.json；
历史 migration 1–4 测试继续用 sqlite-schema-inventory.json，不覆盖旧夹具。
当前 schema 改动后，在隔离 PG 管理连接下重新生成并审阅：

```bash
cargo test --locked --features sqlite-backend --test control_plane_integration \
  regenerate_canonical_schema_inventory -- --ignored
```

生成器只操作随机测试库。原始 34 表/401 列等数字是早期 baseline，不是当前 schema 总量。

同一 case/断言分别跑真实 PG 和 SQLite 文件库，验证金额逐位、权限/ETag、完整候选、
软删除历史、一份事实/回执/账户效果、日志故障下结算、未知不记零及恢复。
PG COPY/锁/升级与 SQLite busy/WAL/alias/单写/取消测试各自保留。
SQLite 变化还要跑[系统 E2E](system-e2e.md)的 --backend sqlite 与 PG 回归，
包括真实浏览器、CLI、离线成对恢复、损坏/已有目标拒绝与 staging 故障。

## 设计与历史验收

实现原按基础、schema/生命周期、身份控制面、财务、Codex、部署六个切片交付。
这些切片均已接线，不再作为“尚不可部署”的依据。
2026-09-20 的交付记录包含双后端 workspace、Clippy、嵌入 UI、Docker/release 二进制、
浏览器/CLI 系统矩阵及已授权八项真实上游回归；只是当时版本/合成数据的证据，
不是当前重跑结果、性能负载或所有硬件断电证明。

SQLx 升级曾暴露 COPY stream 建立前取消的 PG 连接回池问题；
实现以专属 guard 关闭未完成 COPY 的连接，正常完成继续复用。
这类真实后端回归不能被文件库测试替代。

## 外部依据

外部语义核对日期：2026-09-18；它们解释实现选择，不替代本项目业务契约：

- [SQLite 类型与 affinity](https://www.sqlite.org/datatype3.html)：解释为何金额用 TEXT。
- [SQLite 事务](https://www.sqlite.org/lang_transaction.html)：IMMEDIATE 仍可能 BUSY。
- [SQLite synchronous](https://www.sqlite.org/pragma.html#pragma_synchronous)：FULL/WAL 依赖 OS/硬件正确执行。
