# 数据库与控制面架构

> 状态：当前。schema 以两后端的有序 migration 为准，本文说明数据所有权和事务边界。

数据库保存配置、身份与会话、Connector 状态、财务事实、查询投影和审计。
数据面使用不可变运行时快照；请求路径不逐次查库。PostgreSQL 和 Linux SQLite
实现相同业务契约，但连接、锁、金额编码和备份方式不同。

## 目录

- [存储与运行时](#存储与运行时)
- [实体与所有权](#实体与所有权)
- [控制面提交与发布](#控制面提交与发布)
- [财务事实与查询投影](#财务事实与查询投影)
- [迁移与升级](#迁移与升级)
- [修改数据库的流程](#修改数据库的流程)

## 存储与运行时

| 层 | 所有权与边界 |
| --- | --- |
| TOML | 监听器、数据库、spool、秘密文件等进程配置；数据库动态设置缺失时的一次性 bootstrap 值 |
| 控制面数据库 | 身份、授权、路由、上游拓扑、动态策略及审计 |
| `ArcSwap<CompiledRuntimeConfig>` | 完整编译后的不可变配置；数据面选路和策略读取来源 |
| 独立 Codex 凭证状态 | Token、刷新 generation、额度观测及维护生命周期 |
| 本地 spool / admission | 数据库接收前的事件与未决证据；每进程独占目录 |
| 数据库财务与投影 | 不可变计量事实、唯一结算回执、可恢复工作项及日志/排行榜投影 |

PostgreSQL 保留控制面池与日志流水线池的资源分隔。SQLite 的所有仓储共享一个
数据库 owner、一个 writer 和只读池；日志连接数配置不另建写者。
平台、权限和文件身份要求见[SQLite 生命周期](sqlite-lifecycle.md)。

应用通过 prepared change 和专属 Codex guard 操作存储，不接触 PG 事务、连接池、
SQLSTATE 或任意 executor。接口定位见[持久化操作接口](persistence-interfaces.md)。

## 实体与所有权

不把表数量作为产品契约。当前表、列、约束和种子数据直接查看
`migrations/`、`migrations/sqlite/` 及当前 schema inventory。

| 领域 | 主要实体 | 所有权 |
| --- | --- | --- |
| 身份与授权 | `users`、`user_groups`、会话、邀请、注册码、`api_keys`、`api_key_policies` | Console 身份、角色、生命周期、具体 Key 及自助授权范围 |
| 上游拓扑 | `upstream_credentials`、`upstream_accesses`、`routing_groups`、`upstream_channels`、`channel_capabilities` | 认证、接入、组织、绑定和操作能力分别配置 |
| 模型路由 | `models`、`model_routing_profiles`、`model_operation_rules`、`model_capability_tiers`、`model_capability_candidates` | 客户端计价身份、操作规则、priority/strategy、显式能力/wire-model/weight |
| 授权目标 | `api_keys`、`api_key_policies` 的 `allowed_group_ids` / `allowed_channel_ids` | 分别保存所选组与直接渠道 UUID，不把组成员复制为直接选择 |
| Codex | `codex_oauth_credentials`、OAuth flow、额度窗口、重置事件、组可见性 | OAuth 材料与提供方状态；不拥有渠道拓扑 |
| 辅助配置 | `proxies`、`config_templates`、`system_settings`、插件生命周期与设置记录 | 网络、变换、动态通用策略及管理员安装的插件状态 |
| 财务 | `request_metering_facts`、`request_settlements`、`request_settlement_pending` | 不可变终态事实（含可空凭证 ID）、唯一扣款权及未结算工作 |
| 查询与审计 | `request_log_ingest`、`request_logs`、排行榜周期/条目、`audit_logs` | 耐久入口、可查询投影、统计快照及控制面审计 |
| 派发身份 | group/channel/model-rule identity registry | 保留当前账本中派发、模型与渠道的稳定引用，用于查询，不是授权表，也不是将旧库转换到新库的映射工具 |

### 上游身份、能力与渠道授权

- 静态凭证是认证材料与精确目标范围的唯一来源；OAuth Token 由 Codex 扩展持有。
  凭证不属于组或 pool，导入凭证不创建接入、渠道、能力或候选。
- 接入拥有连接器 ID、Base URL、代理与超时；逻辑渠道绑定接入、nullable 凭证和管理组。
- 能力拥有操作、模型目录、独立健康、变换、压缩、探测、倍率及状态统计开关。
  六种操作决定传输；能力不能改绑所属渠道或操作。
- 凭证、接入、渠道绑定和能力各有连接 revision。轮换、禁用、范围变更及
  A → B → A 改绑均不能恢复旧 WebSocket continuation。
- `models.source_model_id` 是不可变客户端计价身份，每模型至多一个 profile。
  操作规则按 `(profile, operation)` 唯一，启用规则必须有非空层。
  候选显式保存能力/wire-model/weight；同层只禁止完全相同组合，组不参与动态展开路由。
- 快照从 Key 所选组的当前成员和直接渠道授权记录推导有效授权，组选择不生成逐渠道授权记录。
  已授权渠道的后续能力共享授权，但仍需路由、目录、
  开关、健康和准入全部满足。
- 删除能力在同一事务撤销其全部候选、清理空层、停用无候选规则并推进相关规则 ETag。
  删除逻辑渠道要求先退役能力，删除管理组要求先清空成员。
- 请求的凭证归属在选路快照中捕获；渠道改绑不改变历史或在途费用，无认证请求不能借用当前绑定推算身份。

完整配置约束见[上游身份、能力与路由](upstream-identity-capabilities.md)。

### 墓碑与动态策略

用户、组、Key、模型及上游资源按各自生命周期保留墓碑。活动读取过滤
`deleted_at IS NULL`；历史查询和结算仍按原 UUID 读取。自然名称可重用，UUID 不可恢复或复用。
删除模型会停用操作规则、撤销候选并成对清空探测模型引用，不删除历史事实。
用户匿名化、组成员迁移和秘密擦除见[控制面软删除](soft-deletion.md)。

`user_groups.filter_fast_mode` 编译到 Key，在客户端 body 白名单之后删除顶层
`service_tier`；日志、倍率、Session affinity、Transform 和 Connector 只能看见过滤后请求。

`system_settings.forwarding_policy` 保存超时、精确候选重试、被动健康、自动禁用、
定时测试、Session affinity 和 Responses WebSocket 等通用运行时策略。
插件设置与提供方隐私策略由插件独立拥有，不作为 host 通用系统设置解释。
TOML 只在对应设置缺失时初始化，之后以数据库为准。

## 控制面提交与发布

```text
取得进程内 serial 门
  → 存储事务：身份/权限/版本检查、业务变更、读取完整候选记录
  → 应用编译完整快照并验证策略
  → 同事务写审计并提交
  → 发布完整 ArcSwap 快照
```

编译、审计或提交失败不得产生部分配置或新快照。取消/丢弃未提交对象回滚。
PostgreSQL 使用 SERIALIZABLE；SQLite 使用唯一 writer 的 BEGIN IMMEDIATE。
定时重载负责其他进程的收敛；数据库与 ArcSwap 不是一个原子事务。
提交后退出由启动/重载恢复，不宣称多实例同时发布。

## 财务事实与查询投影

```text
intent / terminal slot → process-unique spool
  → durable ingress → immutable fact + pending + readiness
     ├─ unique settlement receipt + balance/quota + pending deletion
     └─ indexed request_logs projection → ingress ack
```

- PostgreSQL 入口使用 COPY，SQLite 使用整批事务 INSERT；接收提交后才推进 checkpoint。
- 事实和回执不得外键依赖日志；日志投影失败不阻塞已验证费用的结算。
- 结算回执、余额、Key 已用额度和 pending 删除同事务，UUID 重放不重复扣款。
- Console `billed_at` 由回执派生。usage、费用统计、排行榜与 Codex 窗口费用读取事实；
  成本非 NULL 不等于已结算，也不等于提供方账单。
- 日志保存请求时模型/渠道身份、最终 wire model、usage、有效价格、费用、Peak 和有界诊断，
  不保存 prompt、成功响应正文、完整 Header、Cookie 或秘密。
- 每个逻辑请求只有一条最终日志；逐次尝试只在进程日志中记录。
  Images、Codex 与发送后的 WebSocket 仍受各自不可重试规则约束。

金额、资格、未知终态、恢复与保留要求见[独立计量事实](independent-metering.md)；
本地同步和容量边界见[日志耐久化](request-log-durability.md)。

## 迁移与升级

PostgreSQL 和 SQLite 各有一份当前 v1 基线，用于初始化空库。PG 在同一事务持有 advisory lock，初始化 schema 和记录基线；重开只接受唯一匹配且成功的迁移记录。版本号相同不代表校验和与初始化历史相符。SQLite 文件身份与所有权检查独立于业务基线，见[生命周期](sqlite-lifecycle.md)。

旧版本数据库不原地升级，也没有数据导入、账本回填或旧 spool 重放路径。切换需新建数据库及独立空 spool，保留旧制品和停写点成对备份；这不是已有余额、Key、配置或计量历史的迁移。回退须停止新写者，由匹配的旧制品恢复原备份，并核对切换后的使用。见[升级与回滚](../user/upgrades.md)。

## 修改数据库的流程

1. 同时核对两后端的 schema、约束、种子与数据库初始化契约。
2. 同步持久化 DTO/查询、领域校验、完整快照编译器及 mutation。
3. API 变化先改 OpenAPI，再生成前端类型；数据库结构不在 Markdown 逐列复制。
4. 使用隔离的数据库/文件库验证空库初始化、当前基线重开、旧历史拒绝、失败回滚和直接 SQL 约束；不操作共享业务库。
5. 运行对应 PostgreSQL/SQLite 契约、workspace、Console spec 和系统 E2E。
   SQLite 生命周期/恢复变化同时验证真实 CLI 与成对备份恢复。
6. 同步操作说明并运行文档检查。测试通过不替代真实规模的发布前恢复演练。

## 来源

- `src/persistence/`：仓储、错误翻译、migration runner。
- `src/runtime_config/`：完整候选编译与原子快照。
- [Console OpenAPI](../openapi/console-v1.yaml)：管理接口权威契约。
- [当前架构](architecture.md)、[运维接口](../user/README.md)。
