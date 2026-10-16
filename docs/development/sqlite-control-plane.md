# SQLite 身份与控制面

> 状态：当前。Linux sqlite-backend 部署已接入 serve、bootstrap-admin、
> reset-admin-password；不是仅开发验证的仓储。

本页说明身份/会话与控制面怎样在 SQLite 上保持相同权限、ETag 和完整快照事务。
文件 owner、schema 安装和恢复由[生命周期](sqlite-lifecycle.md)负责；
OAuth 外部操作见[Codex 仓储](sqlite-codex.md)。

## 目录

- [分派与范围](#分派与范围)
- [身份、会话与管理员](#身份会话与管理员)
- [完整配置变更](#完整配置变更)
- [事务与金额](#事务与金额)
- [验证](#验证)

## 分派与范围

AuthRepository 和 ControlPlaneRepository 保留 new(PgPool)，
在 sqlite-backend 下通过 from_sqlite(Arc<SqliteDatabase>) 选择文件后端。
生产组合根先验证配置、认领数据库并安装完整 schema，再构造仓储。
应用只接触仓储及 prepared change，不接触驱动、事务或方言。

| 位置 | 责任 |
| --- | --- |
| backend_auth.rs / backend_control_plane.rs | 封闭后端枚举和 prepared-change 分派 |
| auth.rs / control_plane_write.rs / postgres_control_plane.rs | 既有 PG 实现及共享 DTO |
| sqlite/auth.rs | 登录、会话、邀请、注册和管理员恢复 |
| sqlite/control_plane.rs | 完整配置、普通 mutation、审计、自助 Key |
| upstream_topology/ / upstream_credentials/ | 新上游拓扑及身份的双后端操作 |

没有 SQLx Any、SQL 翻译器、通用 CRUD/UoW 或应用 SQL 分支。

## 身份、会话与管理员

同一公共仓储覆盖：

- 活跃 Console 身份、登录和 normal/password-change 会话用途隔离；
- refresh 轮换、重放撤销、本人/其他/全部会话撤销及过期状态；
- profile、显示名/密码变更、auth-version 和会话失效；
- 临时密码、到期拒绝、不能重置自己及管理员资格；
- 邀请/重发/接受，注册码版本、次数、余额、启停和到期；
- 首个管理员 bootstrap 与现有活跃管理员密码 reset。

两个管理员 CLI 已使用与 serve 相同的配置/owner/迁移入口，
密码仍只从 stdin 读取，AppConfig::validate 统一检查平台、feature 和文件要求。
密码哈希、JWT、Token 及权限策略不因后端改变。

## 完整配置变更

用户、组、Policy、Key、模型/profile/操作规则/tier/candidate、
接入/凭证/逻辑渠道/能力/管理组、代理、模板、动态设置及用户设置均走现行控制面。
批量操作、目录同步、自动禁用/恢复和 reload 也不是逐行非事务入口。

完整 runtime records 在同一读事务加载，包括 Codex 状态和只读历史身份。
新拓扑写入使用[当前资源契约](upstream-identity-capabilities.md)，不保留旧渠道 CRUD、
删除影响 token、pool 或自动投影作为第二套配置来源。

权限/版本/所有权在存储检查；保留秘密脱敏、自助 Key 的 Policy 固定渠道交集、
墓碑和不可逆撤销。能力删除自动修剪候选、空层及停用规则；
逻辑渠道/组按依赖顺序删除，不使用旧级联影响确认流程。

## 事务与金额

读仓储使用只读池，一致快照使用读事务；全部写入共享唯一 BEGIN IMMEDIATE。
UUID、UTC 微秒时间、JSON、金额通过后端私有列适配器绑定。
updated_at 使用当前事务 ag_now()，不能混入连接外时钟或 SQLite CURRENT_TIMESTAMP。

```text
身份/权限/ETag → 修改 → 完整候选
  → 应用编译 → 同事务审计/commit → 发布 ArcSwap
```

编译/审计失败或提交前丢弃不得提交/发布。
调用者先编译，prepared 对象不反向依赖应用；
提交后退出由启动/重载恢复，不宣称数据库与内存是分布式事务。

NUMERIC 输入按原 PG 列规则量化后以 TEXT 保存。
批量余额从 SqliteAmount 读取，checked Decimal 运算后才量化最终结果；
不能先舍入增量、从 audit JSON 或 f64 还原金额。
PG audit 数字与 Rust Decimal 字符串保留各自既有展示形状，审计不是财务运算来源。
归一化与 SQL 保护见[约束映射](sqlite-schema-mapping.md)。

代理列表/审计必须去除 URL userinfo、query、fragment，保留 scheme，
不得产生正则替换控制字符；脱敏不改变实际代理连接 URL。

## 验证

```bash
cargo test --locked --features sqlite-backend --test sqlite_foundation
cargo test --locked --features sqlite-backend --test control_plane_integration sqlite_s3_parity
cargo clippy --locked --workspace --all-targets --features sqlite-backend
cargo test --locked --workspace
cargo test --locked --workspace --features sqlite-backend
```

tests/contracts/sqlite_auth.rs、sqlite_control_plane.rs 验证文件仓储及故障；
sqlite_s3_parity.rs 以相同 case/断言调用真实 PG 与 SQLite：

- 会话重放、身份/管理员、邀请和注册码，耗尽 reader 时仍正确分类过期会话；
- 普通 mutation、批量、设置、目录、拓扑/路由和资源生命周期；
- 所有权、权限、ETag、固定渠道授权、软删除及拒绝无副作用；
- 真实 coordinator 的 compile/commit/publish、编译/审计失败回滚；
- 大额整数、负余额加半个最小单位仅舍入最终结果。

当前契约跑完整 migration，历史升级夹具独立保留，不用旧投影占位行掩盖新拓扑。
仓储测试不替代[系统 E2E](system-e2e.md)真实浏览器/CLI 双后端矩阵，
也不运行付费调用或性能负载。完整部署条件见[SQLite 指南](../user/sqlite.md)。
