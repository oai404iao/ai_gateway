# 数据库与控制面架构

> 状态：当前。数据库结构以 `migrations/` 为唯一权威来源；本文只说明当前持久化边界、
> 主要实体组和运行时编译关系，不逐列复制 schema。

## 设计边界

- PostgreSQL 保存控制面配置、Console 身份与会话、Connector 状态、请求日志、统计投影和审计事实。
- TOML 只保存进程/bootstrap 配置，以及数据库系统设置缺失时的一次性初始值。
- 数据面从 `ArcSwap<CompiledRuntimeConfig>` 读取不可变快照，不在每个代理请求中查询数据库。
- Console 控制面写入在事务中完成候选配置校验和审计；提交成功后立即发布完整新快照，定时重载负责
  跨进程收敛。
- schema 只能通过新的有序 migration 演进；不得修改已经部署的 migration 来伪造当前结构。

应用通过 prepared control-plane/Codex 专属操作使用事务，不直接持有 PG 事务或连接池；
日志写入、日志查询、计量查询与结算已拆为窄句柄。SQLx 错误在存储内部分类，
提交前校验/审计与提交后发布顺序不变。详见[持久化操作接口](persistence-interfaces.md)。
费用来源与结算认领已由[独立计量事实和回执](independent-metering.md)拥有，不再依赖日志投影。

最初的 11 表方案及其当时的取舍已移入
[首版数据库设计归档](../archive/initial-database-design.md)。它不能作为当前列名、表数量或功能边界
的依据。

## 当前实体组

截至当前 migration，实体可按职责分为以下几组；维护者不应把表数量写成稳定产品契约，
新增 schema 时应直接阅读全部 migration：

| 领域 | 主要表 | 责任 |
| --- | --- | --- |
| 上游身份 | `upstream_credentials` | 可复用静态认证材料、精确接入范围、连接 revision 与 Codex 公共身份。 |
| 身份与授权 | `users`、`user_groups`、`user_sessions`、`user_invitations`、`registration_invitation_codes`、`api_key_policies`、`api_keys` | Console 身份、角色、生命周期、注册/邀请、用户可选路由边界和具体 Key 限制。 |
| 模型与路由 | `models`、`model_routing_profiles`、`model_operation_rules`、`model_capability_tiers`、`model_capability_candidates`、`routing_groups`、`upstream_accesses`、`upstream_channels`、`channel_capabilities`、`api_key_capability_grants`、`api_key_policy_capability_grants`、`proxies`、`config_templates`、`system_settings` | 客户端模型价格、协议规则、候选级上游 wire 模型、路由层级/权重、格式隔离、Connector、网络/变换和数据库动态系统策略。 |
| Codex Connector | `connector_pools`、`codex_oauth_credentials`、`codex_oauth_flows`、`codex_quota_window_periods`、`codex_quota_reset_events`、`user_group_codex_quota_visibility` | 共享逻辑凭证、独立操作能力、OAuth、quota 历史和用户组可见性。 |
| 日志与统计 | `request_log_ingest`、`request_logs`、`spend_leaderboard_periods`、`spend_leaderboard_entries`、`audit_logs` | 耐久事件入口、日志/排行榜投影和控制面审计。 |
| 计量与结算 | `request_metering_facts`、`request_settlements`、`request_settlement_pending` | 不可变财务事实、唯一结算回执和可索引的未结算工作集合。 |

## 关键当前语义

### 上游身份、能力与固定授权

PostgreSQL `0064` / SQLite `0004` 引入独立凭证；`0065` / `0005` 在启动事务内
完成能力转存、历史外键迁移、快照校验和旧配置表退役。实现位于
`src/persistence/capability_cutover/`，运行时仓储只读取新拓扑。
迁移失败整批回滚，包括非法停用草稿；SQLite 迁移专属连接关闭外键后不回池，
提交前执行完整 `foreign_key_check`。

- `upstream_credentials` 是静态认证材料与目标范围的唯一来源；Codex Token 仍由专属扩展持有。
- `upstream_accesses` 拥有连接器、Base URL、代理及超时；`upstream_channels` 绑定接入、
  nullable 凭证和格式中立的 `routing_groups`。
- `channel_capabilities` 拥有操作、传输、模型目录、独立健康、探测、压缩、变换、倍率和统计开关。
  能力所属逻辑渠道与操作不可改绑；Codex 一个逻辑凭证有四个能力，Images 初始停用。
- 凭证 revision、接入 revision、渠道 binding revision 与能力 revision 纳入连接身份失效；
  轮换、禁用、范围收窄及 A → B → A 改绑不能恢复旧 WebSocket continuation。
- `models.source_model_id` 是不可变客户端计价身份；每个模型最多一个 `model_routing_profiles`。
  `model_operation_rules` 按 `(profile, operation)` 唯一，启用规则必须有非空 tier。
- `model_capability_tiers` 保存 priority/strategy；`model_capability_candidates` 保存
  capability/wire-model/weight，同层完全相同组合不得重复，不随组成员变化自动扩展。
- Key/Policy 保留组/逻辑渠道选择数组，同时在 `api_key_capability_grants` /
  `api_key_policy_capability_grants` 固定实际能力及授权来源。只有新增来源才展开，
  保留来源即使与新增来源混合编辑也不重新扩权；自助写入还与 Policy 固定范围取交集。
- `group_identity_registry`、`channel_identity_registry`、`model_rule_identity_registry`
  保留旧 UUID/名称及新能力身份，供日志、财务和 spool 重放引用，不参与管理或授权。

完整迁移及安全约束见[身份与能力设计](upstream-identity-capabilities.md)，配置步骤见
[运维接口](../user/operations.md)和[上游凭证管理](../user/upstream-credentials.md)。

### 用户组与 Fast 过滤

`user_groups.filter_fast_mode` 会编译进每个 `CompiledApiKey`。启用时，数据面在客户端白名单之后
删除顶层 `service_tier`，因此后续日志元数据、请求计费倍率、Session affinity、Transform 和
Connector 都只观察过滤后的请求。

### 控制面资源软删除

用户、用户组、API Key 和计价模型使用不可恢复的墓碑式软删除。活动数据查询必须同时过滤
`deleted_at IS NULL`；请求日志、结算和审计查询仍按原 UUID 读取墓碑。删除用户会匿名化身份并
软删除其 Key；删除自定义用户组会把成员迁移到按角色选择的内置默认组、禁用关联注册码并移除
Codex quota 可见性；直接删除 Key 会覆盖其明文 secret。活动记录使用部分唯一索引，因此删除后
可以用相同邮箱或自然名称创建新 UUID。完整阶段边界见[控制面软删除](soft-deletion.md)。

上游资源删除不再隐式级联路由或固定授权：先撤销候选，再按能力、逻辑渠道、组的顺序退役。
仍在用的接入或凭证不能删除；Codex 只能走专属生命周期。墓碑及历史身份保留，原 UUID
不可恢复，同名新资源不继承授权。

删除计价模型会停用操作规则、撤销其 tier/candidate、成对清空能力的探测模型引用，然后
写入墓碑。规则和历史身份保留；撤销操作配置避免不可编辑的墓碑永久阻止能力删除。
数据库触发器拒绝模型恢复、硬删除及探测/路由重新引用墓碑，双后端行为一致。

### 系统设置

`system_settings` 不是预留表。固定的 `forwarding_policy` 文档保存并热更新以下策略：

- 上游超时、按精确渠道/模型候选排除的请求重试、可重试 HTTP 状态码、被动健康、自动禁用和
  定时测试；
- Session affinity 与 Responses WebSocket；
- Codex 合成 workspace path、HTTPS Git remote 等转发元数据策略。

首次启动只在对应设置不存在时使用 TOML bootstrap 值。之后数据库记录是动态运行时来源。

### 历史 migration 0052 硬切换

`0052_model_rule_routing_tiers.sql` 把旧 `model_rules` group/channel 数组、Channel Group
priority/strategy 和 Channel weight 回填到上述三张规则级关系表，然后删除这些旧列和不再适用的
Codex flow weight。
旧的 group target 回填为 `all`、默认权重 `100`；只有所属 group 未同时被规则整体引用的旧直接
Channel target 才回填为 `selected`。若规则已经整体引用该 group，重叠的直接 Channel 仍属于
`all`，其非 `100` 旧权重保存为逐渠道覆盖；其他 group-selected Channel 的非 `100` 旧权重也用
相同方式保留。

这是不能由新旧二进制同时解释的硬切换。多实例部署必须先停止所有旧 Gateway，再让单个新版本实例
应用 migration，验证启动和快照编译后才恢复同版本实例与流量，不能滚动混跑。migration 会检查
禁用或暂时不可用的 latent targets；如果同一 model rule 的同一旧 priority 下存在不同
selection strategy，会明确中止并要求先在旧 schema 上统一这些 group strategy，不能静默选择
一种策略。缺失引用和跨格式引用也会 fail closed。

### 历史 migration 0057 模型与协议层级硬切换

`0057_model_rule_hierarchy.sql` 新建 `model_routing_profiles`，把旧 `model_rules` 行保留为
协议规则，并把旧的 `models.source_model_id` 回填到每个 group/channel target。协议规则 ID
不变，因此历史 `request_logs.model_rule_id` 外键和观测关联继续有效。migration 同时把
Channel 定时探测的 wire `test_model` 与 `test_pricing_model_id` 分离。

该 migration 不支持客户端模型别名兼容。如果任一旧规则的 `client_model` 不等于其
`upstream_model_id` 所指 `models.source_model_id`，migration 会在改表前中止并列出首个冲突；
管理员必须在旧 schema 上清理该别名后重试。成功后客户端模型身份只来自 profile 绑定的价格模型，
旧 `model_rules.client_model` 与 `upstream_model_id` 列会被删除。该步骤同样是停机硬切换，
不能让 0056 及更早版本的进程与新 schema 滚动混跑。migration 还会拒绝无法映射到价格模型的
旧 `test_model`，以及 Images/provider-managed Channel 上不再支持的定时测试配置，避免迁移后
第一次快照编译才暴露不兼容数据。

Gateway 在持有 SQLx 数据库 advisory lock 期间，把连续待执行 migration 放入同一个 PostgreSQL
外层事务。任一步失败会回滚同批先前已经执行的 migration 及其 `_sqlx_migrations` 记录；
更早批次或启动中已经提交的版本不会被追溯回滚。历史 `0034`、`0046` 使用
`ALTER TYPE ... ADD VALUE`，PostgreSQL 要求提交新增枚举值后才能由后续 migration 引用，
因此它们是仅有的既存事务提交屏障；`0053–0063` 属于同一原子批次。禁止新增
`-- no-transaction` migration；新的提交屏障必须作为显式架构例外审查。

### migration 0061 失败请求零费用

`0061_zero_failed_and_cancelled_costs.sql` 把已有 `failed`/`cancelled` 请求费用统一为零。
已经写入 `billed_at` 的旧正费用会按原日志用户和 API Key 聚合，退回用户余额并扣回 Key 已用额度；
成功请求不变。没有价格快照的失败记录也可以用零费用完成幂等结算，供 Codex 拼车恢复旧 pending。

### 历史 migration 0062 扁平路由候选硬切换

`0062_flat_model_route_candidates.sql` 把 group target 与其 Channel 明细合并为
`model_rule_routing_candidates`。旧 `selected` target 原样迁移每条渠道、模型和权重；旧
`all` target 在 migration 时一次性展开当前未删除组成员，并保留默认权重或逐渠道覆盖。即使成员
当时已不再声明该模型，也保留显式候选，使原有 `disconnected` 状态可在能力恢复后重新连通。
空 `all` target 无法转换为候选，因此删除对应空 tier；规则最终无 tier 时自动停用。

该 migration 删除旧的 `model_rule_routing_groups` 和 `model_rule_routing_channels`，旧二进制无法
继续写入，必须按停机硬切换部署。迁移后 Channel Group 只保留资源、Connector 和授权语义，不再
动态决定模型路由成员。

### 请求日志与结算

```text
terminal RequestLogEvent
  -> process-unique local durable spool
  -> request_log_ingest
  -> immutable request_metering_facts + pending + ingress readiness
     -> unique settlement receipt + balance/quota update
     -> indexed request_logs projection
```

- 本地 spool 覆盖数据库写入前的进程崩溃恢复；入口和最终表都依赖请求 UUID 幂等。
- `request_logs` 保存最终协议规则 ID、渠道、实际发送的 candidate 上游模型、顶层 profile 的
  计价模型 ID、usage、有效价格快照、成本、请求开始时是否命中高峰时段，以及有界错误诊断；
  不保存 prompt、completion、完整 Header、Cookie 或密钥。
- Codex quota 当前与历史窗口的凭证总花费按逻辑凭证的 Responses/Images projection、周期边界和
  `cost_amount IS NOT NULL` 从 `request_metering_facts` 聚合；独立渠道/时间索引支撑该查询。
- 普通 Connector 的 Chat Completions、Responses 和 standalone web search 可按数据库策略重试
  响应头前传输失败，也可在向客户端发送前按显式 4xx/5xx 状态码切换到未尝试的渠道/模型候选；
  Images 不自动重试，Codex Connector 与 Responses WebSocket 发送上游请求后也不重试。
- 当前每个逻辑请求仍只写一条最终 `request_logs` 记录。数据库的旧 `attempts` JSONB 列不承载
  当前重试详情；尝试次数只进入完成 tracing。
- 结算插入唯一回执，在同一事务更新余额/额度并删除 pending；Console `billed_at` 由回执派生。

`0063_independent_metering_facts.sql` 回填历史费用与已结算回执，不再次扣款；删除日志物理
`billed_at`，不能与旧 worker 混跑。详见[停机切换与回退](independent-metering.md)。

完整耐久性和故障边界见[请求日志耐久化流水线](request-log-durability.md)。

## 修改数据库的流程

`0054_codex_sharing.sql` 新增固定席位车队与单实例账本身份。绑定和上游身份保持不可变。
`0055_codex_sharing_only_groups.sql` 只增加 Codex 渠道组的整池访问模式与同步触发器，
不改写这些绑定、席位、配额窗口或账本身份。
`0056_codex_sharing_direct_seats.sql` 删除车队的 `user_group_id`，成员直接来自原有
`seats` JSON；同时把旧 Key 经 group target 可达的现有拼车投影回填为显式 channel target，
不改写席位顺序、金额、窗口、账本身份或在途预占。
`0057_model_rule_hierarchy.sql` 的停机升级和别名预检见上文；不得绕过预检手工删除旧列。
`0060_model_soft_deletion.sql` 为模型增加不可恢复墓碑、活动标识部分唯一索引和引用保护；该
migration 不删除 profile、协议规则或历史外键。
`0062_flat_model_route_candidates.sql` 是路由图停机硬切换；不得在新旧 Gateway 混跑时应用。
金额预占不写入余额实体，而由本地耐久 WAL 拥有；后台使用独立计量事实对账。
详见 [Codex 拼车实现](codex-sharing.md)。

1. 新增有序 migration，不修改已发布 migration。
2. 同步 `src/persistence/` DTO/查询、领域类型、运行时编译器和 Console mutation。
3. 若 Console API 形状变化，先改 `docs/openapi/console-v1.yaml`，再生成并提交 TypeScript 类型。
4. 更新本文件或相应专题设计文档，但不要复制可从 migration 直接读取的完整列清单。
5. 使用任务专用数据库或隔离 stack 验证 migration；不要把未发布 schema 应用到其他 worktree
   共用的开发数据库。
6. 运行 PostgreSQL 相关 Rust 门禁和 Console 契约测试。

## 来源

- schema：`migrations/`
- 持久化记录与仓储：`src/persistence/`
- 快照编译：`src/runtime_config/mod.rs`
- 当前请求链路：[当前架构](architecture.md)
- Console API：`docs/openapi/console-v1.yaml`
- 用户可观察行为：[运行与接口说明](../user/operations.md)
