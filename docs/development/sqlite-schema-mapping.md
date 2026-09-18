# SQLite 完整 schema 与约束映射

> 状态：当前 S2 实现。业务 baseline 对齐 `081d8c9` 的 PostgreSQL
> 0001–0063 最终 schema；S3 开发仓储已接入，尚未开放生产启动配置。核对日期：2026-09-18。

整体进度见 [SQLite 双后端实施](sqlite-backend.md)，原子安装、文件身份和
进程独占见[文件与迁移生命周期](sqlite-lifecycle.md)。S2 完成不代表 S3–S6
的完整后端或生产开放门槛已经满足。

## 完整 baseline

`SqliteDatabase::install_schema()` 使用独立 SQLite 历史，在一个事务中安装：

- `migrations/sqlite/0001_baseline.sql`：34 个 STRICT 业务表、401 列、主键、
  唯一键、56 个外键、183 个原 CHECK 名称及额外存储检查、索引和默认组 seed。
- `migrations/sqlite/0002_guards.sql`：跨表保护、不可变性、派生投影和延迟路由约束。

| 范围 | 已实现表 |
| --- | --- |
| 身份 | `users`、`user_groups`、`api_key_policies`、`api_keys`、`user_sessions`、`user_invitations`、`registration_invitation_codes` |
| 普通控制面 | `proxies`、`config_templates`、`models`、`model_routing_profiles`、`model_rules`、`model_rule_routing_tiers`、`model_rule_routing_candidates`、`channel_groups`、`channels`、`system_settings` |
| Codex | `connector_pools`、`codex_oauth_credentials`、`codex_oauth_credential_channels`、`codex_oauth_flows`、`codex_quota_window_periods`、`codex_quota_reset_events`、`user_group_codex_quota_visibility` |
| 拼车 | `codex_sharing_groups`、`codex_sharing_ledger` |
| 日志与财务 | `request_log_ingest`、`request_logs`、`request_metering_facts`、`request_settlements`、`request_settlement_pending`、`spend_leaderboard_periods`、`spend_leaderboard_entries`、`audit_logs` |

PG 基线有 34 个主键、19 个 UNIQUE constraint 和 55 个非内部 trigger。
SQLite 保留约束语义，不要求相同的物理对象数；ingress 序列改为
`INTEGER PRIMARY KEY AUTOINCREMENT`。四个 `_gateway_*` 内部表不计入业务表：
身份、迁移历史、路由真值常量及路由 assertion。

`tests/fixtures/sqlite-schema-inventory.json` 固定当前 PG 列顺序、原类型与约束名。
`tests/contracts/sqlite_parity.rs` 在真正执行完 PG migration 的新库上对照该清单、
SQLite 表结构、外键列/目标/删除动作，以及默认组 UUID、名称、描述、角色。
后续 schema 修改必须同步两个后端、清单和测试，不能以旧本地库代替当前 migration。

不包含历史已删除表或 `request_logs.billed_at`，不重放 PG 数据回填或枚举提交屏障。
保留 0022 默认用户/管理员组；系统探测身份仍由仓储创建，不是静态 seed。

## 存储类型与连接函数

类型适配位于 `src/persistence/sqlite/{types,decimal,functions}.rs`，
不改变领域类型。每个物理连接注册 schema 函数与排序规则，未注册连接写入 fail closed。

| PG 类型/语义 | SQLite 编码与保护 |
| --- | --- |
| UUID | 规范小写连字符 TEXT；`SqliteUuid` 与 CHECK 拒绝非规范编码 |
| enum | TEXT 枚举 CHECK；`ag_api_format` 按领域枚举顺序排序，不按文字顺序 |
| boolean、smallint、integer、bigint | STRICT INTEGER，布尔 0/1、16/32 位范围和原 NULL/CHECK |
| timestamptz/date | `SqliteTimestamp` 固定 UTC 六位微秒加 `Z`；`SqliteDate` 为 `YYYY-MM-DD`，四位年份；时间量化匹配 SQLx PG 相对 2000 年纪元的微秒编码，含纪元前亚微秒与闰秒归一化 |
| varchar(n)、char(3)、text | 明确长度/币种检查；拒绝 NUL，不依赖 SQLite 声明长度 |
| bytea、cidr | BLOB/hash 长度检查；网络前缀解析校验 |
| JSONB | `ag_json_valid` 解析 JSON，递归拒绝 NUL/孤立 surrogate；保留 object/array、NULL 与空对象约束，空对象比较使用解析值 |
| UUID[]、text[]、api_format[] | JSON 数组，校验元素类型、枚举、NULL 成员、非空及子集；不以逗号分隔文本代替数组 |
| NUMERIC | 规范十进制 TEXT、精确 Decimal 比较及列精度/范围/符号检查；禁止 REAL 和 NUMERIC affinity |
| 正则、lower、ANY/ALL、array_position | 专属纯函数执行既有字段规则，保留 NULL/空数组语义 |

`ag_lower` 使用每个 Unicode 标量的简单小写映射；测试与基线 PostgreSQL
`en_US.utf8` 的 ASCII、重音字母、土耳其 `İ`、希腊词尾等案例对照。
这不是任意 PG locale/ICU 排序规则的兼容承诺。JSON 存储不承诺保留 JSONB
的展示空格或对象键顺序；未来仓储仍按类型解码。

### 精确金额

25 个金额/倍率列以及一个吞吐率列按实际精度区分：

| 类型适配器 | PG 列类型 | 用途 |
| --- | --- | --- |
| `SqliteAmount` | `numeric(24,8)` | 余额、费用、额度、排行榜和邀请码金额 |
| `SqliteUnitPrice` | `numeric(24,12)` | 四项单价和渠道倍率 |
| `SqliteSharingAmount` | `numeric(20,8)` | 拼车三个金额限制 |
| `SqliteTokenRate` | `numeric(14,4)` | `request_logs.output_tokens_per_second` |

构造、绑定和解码均校验精度，不舍入；解码恢复 PG/SQLx 的列 scale，
零值保持 SQLx PG 的 scale 0。`SqliteDecimal` 只是通用规范 TEXT 传输，不替代列适配器。
`users.balance_amount` 允许负数以保留软配额透支。价格快照的全有/全无、符号、
币种和费用一致性仍在数据库检查。

不能使用 TEXT 字典序或 SQLite `SUM`、浮点运算、NUMERIC/REAL CAST 计算金额。
S4 仓储尚须落实 checked Decimal 更新与精确聚合；S2 没有添加第二套结算业务逻辑。

## 数据库保护与归一化写入契约

| 职责 | PG 来源 | SQLite 当前实现 |
| --- | --- | --- |
| 事务时间 / `updated_at` | 0001 及后续安装 | `BEGIN IMMEDIATE` 后设置 `ag_now()` 事务时钟；更新校验触发器拒绝未使用当前事务时间的行 |
| audit、事实、回执不可变 | 0001 / 0063 | 数据库触发器禁止 UPDATE/DELETE |
| 查询日志不可变 | 0063 | 禁止 UPDATE，允许 DELETE，不联动删除财务事实/回执 |
| 回执资格、金额、币种、账户 | 0063 | 插入时验证事实状态、精确金额/币种及 API Key 归属 |
| pending / ingress ack | 0063 | pending 禁止 UPDATE，无回执不得 DELETE；ingress 删除要求 metered、事实和查询投影均存在 |
| 提交时路由 shape | 0052 / 0062 | 受保护的延迟外键 assertion，允许合法事务中间态 |
| 路由父锁、profile touch、priced identity | 0052 / 0057 / 0062 | 单写事务、profile 时间推进、已路由模型身份保护 |
| 用户/组/Key 硬删 | 0058 | 禁止硬删，保留 soft-delete CHECK |
| 渠道/组/模型墓碑与引用 | 0059 / 0060 | 墓碑不可恢复/改写；子渠道、探测、启用协议和已删除模型引用保护 |
| Codex pool 一致性及双投影 | 0036 / 0052 | pool/format 检查；数据库派生禁用 Images 组和 credential projections，保留 Responses 身份与 MD5 UUID |
| Images 同步/墓碑 | 0036 / 0052 | 同步共享字段，不传播独立健康/WS 状态；凭证墓碑禁用 Images |
| sharing-only | 0055 | 插入须符合 pool 已有标志，更新在两投影传播 |
| 拼车 binding/identity | 0054 / 0056 | 保护已绑定凭证、provider identity 和已有席位编号 |
| 请求 operation | 0035 | NOT NULL 与 format 一致性 CHECK；仓储在绑定前归一化 |

SQLite 不支持 PG BEFORE trigger 给 `NEW.column` 赋值。这里不模拟“重发整行
INSERT/UPDATE，再 `RAISE(IGNORE)`”：这种方案会破坏 `ON CONFLICT`、行数和幂等语义。
后端私有 SQL 必须满足下列写入前置条件，数据库拒绝不满足条件的最终状态：

1. 新建 Codex Responses group 前，在同一写事务创建/解析 `connector_pools`；
   显式绑定 pool ID。credential 也绑定一致的 pool ID，不依赖数据库补空值。
2. 插入同 pool 的投影时显式继承已有 `sharing_only`。数据库仍负责 Images
   派生与后续共享字段传播；不能自动启用 Images 或扩大授权。
3. 请求日志 upsert 显式绑定 operation。沿用已有 `ApiOperation::legacy_default`
   / journal 归一化处理旧事件，不在插入后绕过 NOT NULL 修补。
4. 更新带 `updated_at` 的表时显式 `SET updated_at=ag_now()`；派生更新触发器也遵守
   此规则。不得使用连接外系统时间或 SQL `CURRENT_TIMESTAMP` 混入其他编码。

这是 S3 已遵守、S5 必须继续遵守的内部写入契约，不是公共 API 变更，也不承诺 PG 原始 SQL
可以逐字执行。数据库仍独立保护跨表业务不变量；没有将保护替换成应用层断言。

### 延迟路由约束

`_gateway_routing_shape` view 计算 enabled rule / tier 是否完整；
每个 rule 有一条 `_gateway_routing_assertions`，其 `valid` 通过
`DEFERRABLE INITIALLY DEFERRED` 外键引用唯一且不可修改的 `_gateway_true(value=1)`。
rule、tier、candidate 变动立即刷新 assertion，提交时才要求全部有效。

helper 行禁止伪造或提前删除；父 rule 的删除和 ID 更新可以合法级联。
tier/candidate 不能跨 rule 移动。测试覆盖合法删旧建新、空 enabled rule/空 tier
提交失败、helper 绕过、级联和精确 candidate 去重，不以迁移器的通用延迟 FK 测试代替。

## 索引与错误分类

baseline 有 57 个显式索引，另有主键/UNIQUE 内部索引。保留 active-name/source-model
部分唯一键、pool/format 唯一键、同 tier 精确 channel/model 唯一键以及 UUID 幂等键。
Codex active identity 的 `NULLS NOT DISTINCT` 用表达式唯一索引实现：
空 identity 已由 CHECK 禁止，故可以用空串表示 NULL 而不合并合法空串值。

`storage_error.rs` 根据实际 `SqliteError` 分类，不对 SQLite 套用 SQLSTATE：
BUSY/LOCKED 为 Conflict；约束、类型/长度与 schema 函数拒绝为 InvalidInput；
四个已有路由依赖名称通过受限触发器错误映射到 RoutingDependency。未知故障仍为 Internal，
不会把任意 trigger 文本误判为可向客户端暴露的约束名。

## S2 验收

`tests/contracts/sqlite_schema.rs` 覆盖完整安装、重开、真实 baseline 加失败后续版本
的整批回滚、合法写入与直接 SQL 负向约束、类型边界、幂等 upsert、金融保护、
路由提交、墓碑、paired projection 和拼车保护。
`tests/contracts/sqlite_parity.rs` 对照当前 PG schema、seed、枚举顺序、金额显示、
时间量化及派生 UUID。文件/进程与取消/重启测试见[生命周期验收](sqlite-lifecycle.md)。

S2 的 schema、编码、约束、错误分类、原子迁移和所有权已实现。
生产配置保持关闭；S3 身份/普通控制面仓储见[当前实现](sqlite-control-plane.md)。
其余业务仓储、完整双后端系统验收、备份恢复与原生 SQLite 版本升级门槛仍属于
后续切片，不能以 S2 测试代替。
