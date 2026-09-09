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

最初的 11 表方案及其当时的取舍已移入
[首版数据库设计归档](../archive/initial-database-design.md)。它不能作为当前列名、表数量或功能边界
的依据。

## 当前实体组

截至 migration `0052_model_rule_routing_tiers.sql`，migration 历史创建了 30 张表。维护者不应
把这个数量写成稳定产品契约；新增 schema 时应直接阅读全部 migration。当前实体可按职责分为：

| 领域 | 主要表 | 责任 |
| --- | --- | --- |
| 身份与授权 | `users`、`user_groups`、`user_sessions`、`user_invitations`、`registration_invitation_codes`、`api_key_policies`、`api_keys` | Console 身份、角色、生命周期、注册/邀请、用户可选路由边界和具体 Key 限制。 |
| 模型与路由 | `models`、`model_rules`、`model_rule_routing_tiers`、`model_rule_routing_groups`、`model_rule_routing_channels`、`channel_groups`、`channels`、`proxies`、`config_templates`、`system_settings` | 价格、上游 wire 模型、规则级路由层级/目标/权重、格式隔离、Connector、网络/变换和数据库动态系统策略。 |
| Codex Connector | `connector_pools`、`codex_oauth_credentials`、`codex_oauth_credential_channels`、`codex_oauth_flows`、`codex_quota_window_periods`、`codex_quota_reset_events`、`user_group_codex_quota_visibility` | 共享逻辑凭证、Responses/Images 投影、OAuth、quota 历史和用户组可见性。 |
| 日志与统计 | `request_log_ingest`、`request_logs`、`spend_leaderboard_periods`、`spend_leaderboard_entries`、`audit_logs` | 耐久日志入口、查询/结算事实、排行榜投影和控制面审计。 |

## 关键当前语义

### 格式、模型和路由

- `api_format` 当前包含 `open_ai_chat_completions`、`open_ai_responses` 和
  `open_ai_images`。
- `model_rules` 以 `(client_model, api_format)` 唯一；`upstream_model_id` 指向一个
  `models` 行。
- 被引用 `models.source_model_id` 同时是发往上游的 wire 模型名和该请求的价格来源。migration
  `0006_simplify_model_routes_and_request_log_filters.sql` 已删除旧的独立
  `model_rules.upstream_model` 列。
- `model_rule_routing_tiers` 保存规则拥有的非负 priority 和单一
  `weighted_random` / `weighted_round_robin` strategy；priority 数值越小越先选。
  `model_rule_routing_groups` 保存 tier 内 group target：`all` 使用正数默认权重并允许
  `model_rule_routing_channels` 提供逐渠道覆盖，`selected` 则要求后者明确列出至少一个正权重
  渠道。权重只在同一 tier 的合格渠道间比较。
- `all` 在每次完整快照编译时展开 group 当前全部渠道，所以以后加入该组的渠道自动继承规则默认
  权重；`selected` 不随 group 新成员扩展。Console 新建规则时把 `all` 默认权重和新选择的显式
  Channel 权重都初始化为 `100`。
- 规则、routing target、渠道组和渠道必须保持格式一致。启用规则可以暂时没有模型兼容或活跃
  渠道；快照仍可发布，实际请求按普通路由错误失败。
- Channel Group 不再保存 priority 或 selection strategy，Channel 和 Codex credential 不再保存
  routing weight。它们仍分别保存资源池/Connector 设置和端点、鉴权、模型能力、网络、变换、
  计费与健康配置。API Key 的 group/channel 授权关系没有改变，只在规则候选之上继续取交集。
- `channel_groups.request_compression` 当前为 `default` 或 `zstd`；只有 Responses group 可以
  选择 `zstd`。
- `channels.health_check` 已在 migration `0017_remove_legacy_compatibility.sql` 删除。当前定时测试、
  自动禁用和超时策略来自现有渠道列与 `system_settings.forwarding_policy`。

### 用户组与 Fast 过滤

`user_groups.filter_fast_mode` 会编译进每个 `CompiledApiKey`。启用时，数据面在客户端白名单之后
删除顶层 `service_tier`，因此后续日志元数据、请求计费倍率、Session affinity、Transform 和
Connector 都只观察过滤后的请求。

### 系统设置

`system_settings` 不是预留表。固定的 `forwarding_policy` 文档保存并热更新以下策略：

- 上游超时、请求重试、被动健康、自动禁用和定时测试；
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

### 请求日志与结算

```text
terminal RequestLogEvent
  -> process-unique local durable spool
  -> request_log_ingest
  -> indexed request_logs
  -> idempotent settlement and statistics projection
```

- 本地 spool 覆盖数据库写入前的进程崩溃恢复；入口和最终表都依赖请求 UUID 幂等。
- `request_logs` 保存最终选中路由、usage、有效价格快照、成本、请求开始时是否命中高峰时段，
  以及有界错误诊断；不保存 prompt、completion、完整 Header、Cookie 或密钥。
- Codex quota 当前与历史窗口的凭证总花费按逻辑凭证的 Responses/Images projection、周期边界和
  `cost_amount IS NOT NULL` 从该表聚合；现有 `(channel_id, started_at)` 索引支撑该只读查询。
- Chat Completions、Responses 和 standalone web search 可按数据库策略在收到上游响应头前自动
  重试不同渠道；Images 不自动重试，Responses WebSocket 发送上游消息后也不重试。
- 当前每个逻辑请求仍只写一条最终 `request_logs` 记录。数据库的旧 `attempts` JSONB 列不承载
  当前重试详情；尝试次数只进入完成 tracing。
- 结算以 `billed_at IS NULL` 取得唯一处理权，在同一事务更新用户余额和 API Key 已用额度。

完整耐久性和故障边界见[请求日志耐久化流水线](request-log-durability.md)。

## 修改数据库的流程

`0054_codex_sharing.sql` 新增固定席位车队与单实例账本身份。绑定和上游身份保持不可变，
金额预占不写入余额实体，而由本地耐久 WAL 拥有；后台仅使用既有请求日志对账。
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
