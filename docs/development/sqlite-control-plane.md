# SQLite 身份与控制面

> 状态：当前 S3 实现。仅在 `sqlite-backend` feature 下供开发验证；
> 生产配置仍只支持 PostgreSQL。完整后端进度见 [SQLite 双后端实施](sqlite-backend.md)。

## 分派与范围

`AuthRepository` 和 `ControlPlaneRepository` 保持原来的操作签名与
`new(PgPool)` 构造器。开发测试可以通过 `from_sqlite(Arc<SqliteDatabase>)`
选择 SQLite；调用前须完成 S2 的 `install_schema()`。
应用仍只持有仓储和 prepared change，不接触驱动、原始事务或方言。

| 文件 | 职责 |
| --- | --- |
| `src/persistence/backend_auth.rs` | 认证仓储的封闭后端枚举分派 |
| `src/persistence/backend_control_plane.rs` | 控制面仓储与 prepared change 的后端分派 |
| `src/persistence/auth.rs`、`control_plane_write.rs`、`postgres_control_plane.rs` | 既有 PostgreSQL 实现；公共 DTO 继续由 `persistence` 导出 |
| `src/persistence/sqlite/auth.rs` | SQLite 认证、会话、邀请、注册和管理员恢复 |
| `src/persistence/sqlite/control_plane.rs` | SQLite 完整配置读取、普通控制面变更、审计、删除影响和自助 Key |

未引入 SQLx Any、运行时 SQL 翻译器、通用 CRUD/UoW 或应用层 SQL 分支。
原 `persistence/mod.rs` 中的 PostgreSQL SQL 与记录类型位于
`postgres_control_plane.rs`；`mod.rs` 仅作模块/公共接口入口。

### 身份与管理员操作

24 个认证操作均实现双后端：

- 登录身份和活跃 Console 身份检查，normal/password-change 会话用途隔离；
- 刷新轮换、重放撤销、本人/其他/全部会话撤销，以及过期状态列表；
- profile、显示名和密码变更，auth-version 推进与会话失效；
- 临时密码发放/完成、到期拒绝、不能重置自己、管理员资格检查；
- 邀请、重发、接受；注册码新增/更新/ETag、使用次数、余额、启停与到期；
- 首个管理员 bootstrap 和现有活跃管理员密码 reset。

`bootstrap-admin` / `reset-admin-password` 的底层仓储操作已支持 SQLite；
本阶段**不开放**命令行的 SQLite 配置路径。serve、两个 CLI 的文件生命周期组合、
S6 已接通 stdin/配置/容器路径，仍由 `AppConfig::validate` 统一检查。
密码哈希、token 和 JWT 的应用层策略不变。

### 普通控制面

完整支持用户/组/策略/Key、模型/profile/protocol tiers/candidates、
渠道/组、模板、代理、系统设置、用户设置的读写，以及用户/渠道批量操作、
模型目录同步、自动禁用/恢复和手动 reload。
保留资源详情、审计投影、秘密字段脱敏、删除影响/确认 token、软删除与引用修剪、
本人 Key 所有权/Policy/固定席位授权、版本冲突和撤销不可逆规则。

完整 runtime snapshot 在同一个读事务中加载，包含 Codex credential projection、
拼车 canonical/protected channels、身份别名、完整 quota windows 和 sharing-only 限制。
这些读取由 S3 引入；专属 OAuth/额度/WAL 操作见 [S5](sqlite-codex.md)。

## 事务与金额

读操作使用 SQLite 只读池；一致性快照使用读事务。所有写入使用唯一写池的
`BEGIN IMMEDIATE`，通过 S2 列适配器绑定 UUID、微秒时间、精确金额及 JSON 数组。
实现遵守[归一化写入契约](sqlite-schema-mapping.md)：显式 pool/标志归一化，
更新带时间列的表时使用 `updated_at=ag_now()`。

prepared change 保持既有顺序：

```text
身份/权限/ETag 检查及业务修改
  → 读取完整候选 runtime_records
  → application 编译验证
  → 在同一事务写审计并 commit
  → 发布完整 ArcSwap 快照
```

编译失败、审计失败或提交前丢弃均不得提交修改或发布候选。调用方仍负责在 commit
之前验证候选；prepared 对象不反向依赖 application 编译器。数据库提交后崩溃仍靠
启动/重载恢复，不宣称数据库与内存发布构成分布式事务。

NUMERIC 输入按现有 PG 列规则量化后用 TEXT 保存；批量余额从 `SqliteAmount` 读取，
checked Decimal 运算之后才量化最终结果，不能先舍入增量，也不能从 audit JSON
或 `f64` 恢复账户金额。SQL 构造的 PG audit 数字与 Rust DTO 的 Decimal 字符串
分别保持既有展示形状，不将审计数据作为财务计算来源。

S3 双后端契约同时发现并修复 PostgreSQL 代理列表/审计的正则替换转义：
含 URL userinfo 时仍去除凭据/query/fragment，但保留原 scheme，不产生控制字符。
这不改变上游代理实际使用的连接 URL。

## 后续切片边界

SQLite Codex credential 管理/视图、OAuth flow、token refresh、quota 更新/history/reset、
sharing ledger 认领和独立 sharing 管理列表已在 [S5](sqlite-codex.md) 接通，
不再经过 PG-only 拒绝占位。

S4 的 ingress、计量/结算、统计见[计量与结算](sqlite-metering.md)。S6 已开放 Linux 部署；
文件限制与[原生 SQLite 版本门槛](sqlite-lifecycle.md#原生-sqlite-版本门槛)仍强制执行。

## 验证

```bash
cargo test --locked --features sqlite-backend --test sqlite_foundation
cargo test --locked --features sqlite-backend --test control_plane_integration sqlite_s3_parity
cargo clippy --locked --workspace --all-targets --features sqlite-backend
cargo test --locked --workspace
cargo test --locked --workspace --features sqlite-backend
```

`tests/contracts/sqlite_auth.rs` / `sqlite_control_plane.rs` 使用真实文件 schema，
覆盖仓储和故障点；包括耗尽 reader 时正确分类过期会话、不同 group/credential UUID
下保留拼车保护、两个 format projection 与 alias、编译失败/审计失败回滚。

`tests/contracts/sqlite_s3_parity.rs` 使用**同一套 case 函数和断言**，
分别经公共仓储运行于新 PostgreSQL 库和 SQLite 文件库，不使用空仓储替身：

- 认证、会话重放、管理员操作、邀请/注册码；
- 全部 31 个普通 `ControlPlaneMutation` 变体；`SaveCodexSharing` 属于 S5；
- 批量操作、路由重建、删除影响、设置、目录和资源生命周期；
- 权限、ETag、软删除、所有权及无副作用拒绝；
- 真实 coordinator 的 compile/commit/publish 与 rollback；
- 大额整数精确加法，以及负余额加半个最小单位时只舍入最终结果。

CI 在既有 PG gate 之外运行 SQLite 文件库契约及 `control_plane_integration sqlite_`。
这些仓储测试与 S6 的真实浏览器/CLI 双后端系统验收互补；不运行付费模型或转发压测。
