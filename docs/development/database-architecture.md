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
| 模型与路由 | `models`、`model_routing_profiles`、`model_rules`、`model_rule_routing_tiers`、`model_rule_routing_candidates`、`channel_groups`、`channels`、`proxies`、`config_templates`、`system_settings` | 客户端模型价格、协议规则、候选级上游 wire 模型、路由层级/权重、格式隔离、Connector、网络/变换和数据库动态系统策略。 |
| Codex Connector | `connector_pools`、`codex_oauth_credentials`、`codex_oauth_credential_channels`、`codex_oauth_flows`、`codex_quota_window_periods`、`codex_quota_reset_events`、`user_group_codex_quota_visibility` | 共享逻辑凭证、Responses/Images 投影、OAuth、quota 历史和用户组可见性。 |
| 日志与统计 | `request_log_ingest`、`request_logs`、`spend_leaderboard_periods`、`spend_leaderboard_entries`、`audit_logs` | 耐久事件入口、日志/排行榜投影和控制面审计。 |
| 计量与结算 | `request_metering_facts`、`request_settlements`、`request_settlement_pending` | 不可变财务事实、唯一结算回执和可索引的未结算工作集合。 |

## 关键当前语义

### 独立上游凭证

PostgreSQL `0064` 与 SQLite `0004` 引入 `upstream_credentials`，普通渠道通过
`channels.credential_id` 引用唯一静态认证材料。旧认证列被清空且不可再写入；
运行时仓储在同一个读取事务中解析身份、目标范围、启用状态和渠道引用。
`src/persistence/upstream_credentials.rs` 拥有共享验证，SQLite 实现位于同名后端模块。
停用渠道也阻止删除或收窄目标范围。凭证 revision 与渠道 binding revision 独立推进，
使密钥轮换、禁用/恢复及 A → B → A 改绑均不会恢复旧连接身份。

Codex 保留历史 credential UUID，公共身份与两种 canonical 投影同步，Token/配额仍由
专属表维护；不得通过通用 CRUD 绕过生命周期或拼车保护。
升级预检也覆盖停用草稿；非法认证或作用域使整批迁移回滚。
使用说明见[上游凭证管理](../user/upstream-credentials.md)，后续阶段见
[身份与能力设计](upstream-identity-capabilities.md)。

### 格式、模型和路由

- `api_format` 当前包含 `open_ai_chat_completions`、`open_ai_responses` 和
  `open_ai_images`。
- `models.source_model_id` 是客户端请求使用的模型标识及计价身份。每个 `models` 行最多由一个
  `model_routing_profiles` 顶层规则引用；被引用后 `source_model_id` 不可修改。
- 活动模型的 `source_model_id` 由部分唯一索引约束。模型墓碑保留原标识和价格事实，但不进入
  Console、目录同步匹配或运行时快照；同名重建使用新 UUID。
- `model_rules` 现在是 profile 下按 `(model_routing_profile_id, api_format)` 唯一的协议规则。
  顶层 profile 不重复保存格式、启用状态或上游模型；协议格式创建后不可修改。
- 上游 wire model 属于具体 `model_rule_routing_candidates` 候选，不属于计价模型或协议父记录。
  协议写入会验证每条候选引用的 Channel 格式一致，且该 Channel 的 `available_models` 声明了
  对应模型；因此价格模型 A 可以路由到未单独计价的 wire model B。后续 Channel 能力变更仍可
  让既有规则进入 `disconnected`。
- `model_rule_routing_tiers` 保存规则拥有的非负 priority 和单一
  `weighted_random` / `weighted_round_robin` strategy；priority 数值越小越先选。
  `model_rule_routing_candidates` 为 tier 直接保存 `channel_id`、`upstream_model` 和正权重。
  权重只在同一 tier 的合格渠道/模型候选间比较；同一 Channel 可在同层使用多个不同模型，也可
  跨层重复，只有同层完全相同的 `(channel_id, upstream_model)` 组合被主键拒绝。
- Channel Group 不进入持久化路由图。Console 逐行添加显式渠道/模型候选，新行默认权重为 `1`；
  渠道组仅提供选项上下文，之后新增或移动组成员不会隐式改变已保存规则。
- 规则、routing candidate、渠道组和渠道必须保持格式一致。计价模型启用时，停用协议可无 tier 并
  显示为 `draft`；`model_disabled` 在计价模型停用后优先于其他状态。启用协议必须至少有一个
  非空 tier，但可以暂时没有模型兼容或活跃
  渠道；快照仍可发布，实际请求按普通路由错误失败。
- Channel Group 不再保存 priority 或 selection strategy，Channel 和 Codex credential 不再保存
  routing weight。它们仍分别保存资源池/Connector 设置和端点、鉴权、模型能力、网络、变换、
  计费与健康配置。API Key 的 group/channel 授权关系没有改变，只在规则候选之上继续取交集。
- `channel_groups.request_compression` 当前为 `default` 或 `zstd`；只有 Responses group 可以
  选择 `zstd`。
- `channels.health_check` 已在 migration `0017_remove_legacy_compatibility.sql` 删除。当前定时测试
  的上游请求模型来自 `test_model`，计价记录来自成对设置的 `test_pricing_model_id`；自动禁用和
  超时策略来自现有渠道列与 `system_settings.forwarding_policy`。

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

普通渠道和渠道组也使用不可恢复墓碑。删除预览对 child channels、模型协议规则、API Key、
API Key Policy 和 quota 可见性依赖生成确认 token；DELETE 在 `SERIALIZABLE` 事务中重新计算，
影响变化时以 `deletion_impact_changed` 失败。渠道墓碑会清除 secret、上游 URL、Transform、
proxy、超时、测试和模型能力配置；组墓碑会同时处理所有普通 child channels。删除事务解绑授权
引用，删除对应渠道/模型候选与空 tier，并在协议规则无 tier 时将其停用。运行时和普通管理查询仅加载
活动渠道/组，历史 request log 和 audit 仍通过墓碑 UUID 读取稳定名称。Codex OAuth managed
groups/channels 保持 connector pool 专用生命周期，普通删除接口返回
`provider_managed_resource`；数据库触发器拒绝渠道和渠道组的直接硬删除。

删除计价模型会先停用其 profile 下全部协议规则，并成对清空 Channel 的 `test_model` /
`test_pricing_model_id`，再停用模型并写入墓碑。Profile、协议规则、tier 和 candidate 继续保留，
但活动管理查询与完整运行时快照按父模型 `deleted_at IS NULL` 隐藏它们。模型自然标识可由新 UUID
复用；目录同步只匹配活动模型，不会刷新墓碑。数据库触发器拒绝模型恢复、墓碑修改、硬删除，以及
定时测试或新路由重新引用墓碑。

### 系统设置

`system_settings` 不是预留表。固定的 `forwarding_policy` 文档保存并热更新以下策略：

- 上游超时、按精确渠道/模型候选排除的请求重试、可重试 HTTP 状态码、被动健康、自动禁用和
  定时测试；
- Session affinity 与 Responses WebSocket；
- Codex 合成 workspace path、HTTPS Git remote 等转发元数据策略。

首次启动只在对应设置不存在时使用 TOML bootstrap 值。之后数据库记录是动态运行时来源。

### migration 0052 硬切换

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

### migration 0057 模型与协议层级硬切换

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

### migration 0062 扁平路由候选硬切换

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
