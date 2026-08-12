# 数据库设计

> 状态：已完成设计记录。数据库 schema 以 `migrations/` 为准；当前运行时范围以
> [运行与接口说明](../user/operations.md) 为准，Console/JWT 背景见
> [Console 认证设计记录](console-auth.md)。
>
> **历史说明：** 下文的“首版 11 张表”描述的是 Console 登录重构前的简化基线。migration `0005_console_auth_and_policies.sql` 已新增用户角色、登录凭据、session、邀请和 API Key Policy，并将 `users.name` 迁移为 `users.display_name`。migration `0010_api_key_target_selection.sql` 又将 Policy 收敛为用户可选渠道组/渠道的授权边界，并把实际目标、RPM、并发和额度保留在具体 API Key 上。migration `0024_registration_invitation_codes.sql` 进一步新增只保存哈希、可限次数、可过期并绑定用户组与初始余额的可复用注册邀请码。涉及用户认证或 Console 权限时，以新 migration、`src/persistence/auth.rs` 和 `src/http/console.rs` 为准。

## 1. 设计原则

- 数据库保存**当前控制面配置**与**不可丢失的请求/审计事实**。
- 一个 API Key 的归属、格式限制、权限、可用分组、限流与额度都在 `api_keys` 一行中。
- 一个模型的当前价格都在 `models` 一行中。价格更新覆盖该行；`request_logs` 保存实际使用的完整单价快照，历史费用不依赖当前价格。
- 一个规则的多个渠道组/渠道目标使用数组字段，不建立关联表。由控制面编译器验证数组中 ID 的存在性、格式与启用状态。
- 单实例的健康、熔断、in-flight 和分钟级限流计数仅在内存中维护；数据库不承担高频运行状态。
- 所有请求链路不在每次请求中查询数据库：静态路由配置读取 `ArcSwap` 中的已编译快照，限流/额度等可变计数读取同进程内存状态。

这不是抽象能力预留：当确实需要“不同渠道有不同上游模型名”“价格历史回溯”“多成员租户”或“跨实例运行状态”时，再按实际需求拆表。首版不为这些假设提前付出复杂度。

## 2. 总览

```mermaid
erDiagram
    users ||--o{ api_keys : owns
    models ||--o{ model_rules : prices
    channel_groups ||--o{ channels : contains
    proxies ||--o{ channels : used_by
    config_templates ||--o{ channels : used_by
    users ||--o{ request_logs : makes
    api_keys ||--o{ request_logs : authenticates
    channels ||--o{ request_logs : handles
```

首版固定为以下 11 张表：

| 表 | 职责 |
| --- | --- |
| `users` | 控制台用户或租户，以及当前余额。 |
| `api_keys` | 客户端 Key、归属、格式/权限/分组范围、限流与额度。 |
| `models` | 系统维护的模型目录与当前四类单价；可由管理员创建或从 models.dev 导入。 |
| `model_rules` | `(client_model, api_format)` 到多个渠道组/渠道及一个上游模型名的路由。 |
| `channel_groups` | 同格式负载均衡池和优先级。 |
| `channels` | 上游地址、鉴权、权重、超时、变换覆盖、可用模型和预留健康检查字段。 |
| `proxies` | HTTP/SOCKS 出口代理。 |
| `config_templates` | 可复用的请求/响应变换。 |
| `request_logs` | 一次逻辑请求的最终结果、用量、费用与价格快照；`attempts` 预留给未来重试审计。 |
| `audit_logs` | 控制面变更记录。 |
| `system_settings` | 预留的 JSONB 系统设置；当前运行时未加载。 |

## 3. 基础约定

### 3.1 类型和公共字段

```sql
CREATE TYPE api_format AS ENUM (
    'open_ai_chat_completions',
    'open_ai_responses'
);
```

这两个值必须与 Rust `ApiFormat` 的 `snake_case` serde 值一致。除此之外，状态和策略使用受限 `text` + `CHECK` 或 Rust 枚举校验，避免为少量稳定字段创建大量数据库 enum。

控制面表使用：

```text
id uuid primary key
created_at timestamptz not null
updated_at timestamptz not null
```

`audit_logs` 是只追加事实表。`request_logs` 不使用 `updated_at`，最终请求结果写入后只允许一次从 `billed_at IS NULL` 到非空的结算状态更新。金额使用 `numeric(24, 8)`，单价使用 `numeric(24, 12)`，Token 使用 `bigint`，所有时间均为 UTC `timestamptz`。

控制面记录通过 `enabled` / `status` 停用，不物理删除；被日志引用的记录使用 `ON DELETE RESTRICT`。`audit_logs` 禁止更新和删除；`request_logs` 只允许一次结算标记更新。当前二进制没有日志保留 worker；未来若增加删除逻辑，只能作为明确的保留任务实现。

### 3.2 数组与 JSONB 的约定

为避免关系表，首版有三类有意使用数组/JSONB 的字段：

| 字段 | 语义 |
| --- | --- |
| `api_keys.allowed_group_ids uuid[]` / `allowed_channel_ids uuid[]` | Key 的实际路由白名单；允许整个渠道组或单独渠道，两数组合计至少一个目标。 |
| `model_rules.channel_group_ids uuid[]` / `channel_ids uuid[]` | 路由可选目标；两者可同时存在，合计至少一个元素。 |
| `channels.available_models text[]` | 渠道实际支持的上游模型名。 |
| `permissions`、变换、健康检查和系统策略 JSONB | 使用固定 schema，由 Rust 反序列化/编译，拒绝未知字段。 |

数组内 UUID 无法使用普通外键。保存或发布配置时，控制面编译器必须一次性验证所有引用；验证失败则整笔事务回滚，旧 `ArcSwap` 快照继续服务。这个完整性边界是为减少表数量而作出的明确取舍。

### 3.3 密钥和敏感数据

按 PRD，客户端 API Key 与上游 API Key 原始保存，以便所属用户和管理员查看。首版直接使用 `secret_value text UNIQUE` 查询 Key，不额外保存摘要列。

明文 Key、代理密码、`Authorization`、Cookie、请求正文、成功响应正文和完整 Header 绝不能写入 `request_logs`、`audit_logs` 或 tracing。失败请求的 `error_summary` 可以保存最长 16KiB、已清理控制字符的上游文本错误响应、结构化 SSE/WebSocket 错误事件，以及网关或传输错误诊断；上游可能在错误中回显 prompt、标识符或其他业务数据，因此 Console、备份和只读副本都必须按敏感数据边界管理。

## 4. 表结构

### 4.1 `users`

| 列 | 类型 | 说明 |
| --- | --- | --- |
| `id` | `uuid` | 主键，由应用生成。 |
| `name` | `varchar(200)` | 非空、唯一的展示/租户名称。 |
| `status` | `text` | `active`、`suspended` 或 `disabled`，默认 `active`。 |
| `balance_amount` | `numeric(24,8)` | 非空，默认 `0`；当前 USD 余额投影。 |
| `created_at` / `updated_at` | `timestamptz` | 通用时间列。 |

`balance_amount` 是余额投影列。可结算终态日志由后台 worker 在同一事务中以 `billed_at` 条件更新取得唯一结算权，并扣减该列；本阶段没有硬余额预留，余额可以为负。管理接口仍不提供充值、退款或人工修正；若未来保留无独立账本的方案，这些操作必须写入 `audit_logs`，需要严格财务账本时再新增相应实体。

`users.websocket_enabled` 保存用户级 Responses WebSocket 偏好。用户本人通过
`/console/v1/me/settings` 修改，管理员也可以通过版本化用户资源修改；两条路径都会重新编译并发布
该用户所有 API Key。

`user_groups.filter_fast_mode` 保存组级静默 Fast 过滤策略。运行时加载 API Key 时联表读取该值并
写入不可变 `CompiledApiKey`；启用后，数据面在客户端白名单之后删除顶层 `service_tier`，因此该
字段既不进入上游，也不参与 `fast_mode` 日志标记或模型 `request_multipliers` 匹配。

### 4.2 `api_keys`

一行包含 API Key 的全部授权与限制，不建立 scope、permission 或 quota 关联表。

| 列 | 类型 | 说明 |
| --- | --- | --- |
| `id` | `uuid` | 主键。 |
| `user_id` | `uuid` | 非空，外键至 `users(id)`。 |
| `name` | `varchar(100)` | Key 的控制台别名；同一 `user_id` 下唯一。 |
| `secret_value` | `text` | 非空、全局唯一；原始 Bearer Key。 |
| `status` | `text` | `active`、`disabled`、`revoked` 或 `expired`。 |
| `expires_at` | `timestamptz` | 可空；非空时必须晚于 `created_at`。 |
| `allowed_api_formats` | `api_format[]` | 非空且至少一个元素；允许调用的 API 格式。 |
| `permissions` | `text[]` | 非空；首版仅允许 `proxy`、`models.read`。 |
| `allowed_group_ids` | `uuid[]` | `NOT NULL` 数组列；选择整个渠道组时，该组当前及后续渠道均可用。 |
| `allowed_channel_ids` | `uuid[]` | `NOT NULL` 数组列；用于只允许某些具体渠道。管理/自助写入要求两数组合计至少一个目标。 |
| `requests_per_minute` | `integer` | 可空、正数；分钟请求限流。 |
| `max_concurrent_requests` | `integer` | 可空、正数。 |
| `quota_limit_amount` | `numeric(24,8)` | 可空、非负；该 Key 生命周期总额度上限。 |
| `quota_used_amount` | `numeric(24,8)` | 非空，默认 `0`；已结算的累计费用。 |
| `last_used_at` | `timestamptz` | 可空，异步低频更新。 |
| `created_at` / `updated_at` | `timestamptz` | 通用时间列。 |

建议约束：

```sql
UNIQUE (secret_value);
UNIQUE (user_id, name);
CHECK (cardinality(allowed_api_formats) > 0);
CHECK (array_position(allowed_group_ids, NULL::uuid) IS NULL);
CHECK (array_position(allowed_channel_ids, NULL::uuid) IS NULL);
CHECK (permissions <@ ARRAY['proxy', 'models.read']::text[]);
CHECK (requests_per_minute IS NULL OR requests_per_minute > 0);
CHECK (max_concurrent_requests IS NULL OR max_concurrent_requests > 0);
CHECK (quota_limit_amount IS NULL OR quota_limit_amount >= 0);
```

分钟级限流和并发数在单实例内存中维护；每次请求只读取 `ArcSwap` 中的已编译快照，不查询数据库。额度是**软预检查**：存在 `quota_limit_amount` 时，已结算的 `quota_used_amount >= quota_limit_amount` 才拒绝。结算 worker 成功后会立即把数据库返回的已用额度发布到同进程准入状态，同时定期重载仍是持久化快照的恢复路径。它不估算本次费用或预留金额，因此一笔在额度内开始、随后结算到上限之外的请求仍允许；数据库保留两个金额非负检查，但不限制 `quota_used_amount <= quota_limit_amount`。

未来若需要硬额度，须另行设计保留和结算：按 Key 串行化 `used + reserved + upper_bound`，持久化保留金额，确认 `billed_at` 后仅释放未结算保留额，并从未结算日志恢复。当前软预检查不能被误解为硬额度。

`api_key_policies` 不再保存格式、权限、RPM、并发、额度或最大 Key 数。它只保存
`allowed_group_ids`、`allowed_channel_ids` 和启用状态，表示用户创建或调整 Key 时可选择的资源上界。
选择整个组也允许用户选择该组内的单独渠道；Policy 更新不会反向改写既有 Key 的实际限制。

### 4.3 `models`

一个模型行就是当前目录信息和当前价格，不维护价格版本历史。管理员可直接维护模型，也可显式从 models.dev 导入；`price_effective_at` 表示这一组当前价格从何时开始有效。

| 列 | 类型 | 说明 |
| --- | --- | --- |
| `id` | `uuid` | 主键。 |
| `source_model_id` | `varchar(300)` | 非空、唯一；本地模型来源标识。models.dev 导入时使用远端原始 `model_id`。 |
| `display_name` | `varchar(300)` | 非空。 |
| `provider_name` | `varchar(200)` | 可空。 |
| `enabled` | `boolean` | 非空，默认 `true`。 |
| `currency` | `char(3)` | 非空且固定为 `USD`；仅作为价格快照元数据，不是可配置项。 |
| `price_unit_tokens` | `bigint` | 非空、正数，例如 `1000000`。 |
| `input_unit_price` | `numeric(24,12)` | 非空、非负。 |
| `cached_input_unit_price` | `numeric(24,12)` | 非空、非负；不单独收费时为 `0`。 |
| `cache_write_unit_price` | `numeric(24,12)` | 非空、非负；不单独收费时为 `0`。 |
| `output_unit_price` | `numeric(24,12)` | 非空、非负。 |
| `price_effective_at` | `timestamptz` | 非空；当前价格生效时间。 |
| `advanced_billing` | `jsonb` | 非空对象；保存长上下文价格档位和基于原始请求体的整次请求计费倍率。 |
| `source_payload` | `jsonb` | 非空，默认 `{}`；不透明来源元数据。models.dev 导入时保存已限制大小的远端元数据。 |
| `last_synced_at` | `timestamptz` | 可空；最近一次 models.dev 同步时间。 |
| `created_at` / `updated_at` | `timestamptz` | 通用时间列。 |

管理员同步价格时更新同一行。每次可计费请求在 `request_logs` 复制币种、计价单位、四类单价和 `price_effective_at`，所以价格被更新后，已有日志仍可精确解释费用。缓存读/写没有单独价格时存 `0`，不使用 `NULL` 表示“未知”。

models.dev 的长上下文价格档位会替换 `advanced_billing.long_context_tiers`。当目录模式同时提供
`provider.body.service_tier`，且网关支持的基础输入、缓存读、缓存写和输出价格都按同一比例变化时，
同步器会生成 `/service_tier` 精确匹配的 `request_multipliers` 规则。可选模式缺失、无效或不是统一
倍率时会被忽略。已有模型同步时，相同 JSON Pointer 与匹配值的规则由目录值更新，其他管理员维护的
请求倍率保持不变。

### 4.4 `model_rules`

| 列 | 类型 | 说明 |
| --- | --- | --- |
| `id` | `uuid` | 主键。 |
| `client_model` | `varchar(300)` | 非空；客户端 `model` 的精确匹配值。 |
| `api_format` | `api_format` | 非空。 |
| `model_id` | `uuid` | 非空，外键至 `models(id)`；本规则的计费模型。 |
| `upstream_model` | `varchar(300)` | 非空；目标上游收到的模型名。 |
| `channel_group_ids` | `uuid[]` | 非空，默认空数组；可选渠道组。 |
| `channel_ids` | `uuid[]` | 非空，默认空数组；可选直接渠道。 |
| `enabled` | `boolean` | 非空，默认 `true`。 |
| `description` | `text` | 可空。 |
| `created_at` / `updated_at` | `timestamptz` | 通用时间列。 |

```sql
UNIQUE (client_model, api_format);
CHECK (cardinality(channel_group_ids) + cardinality(channel_ids) > 0);
```

一个规则只有一个 `upstream_model` 和一个计费模型。这是首版的刻意限制：如果同一客户端模型在不同渠道需要不同别名，应创建不同的渠道组，或在真实需求出现后再拆出专用路由目标表。

### 4.5 `channel_groups`

| 列 | 类型 | 说明 |
| --- | --- | --- |
| `id` | `uuid` | 主键。 |
| `name` | `varchar(100)` | 非空、唯一。 |
| `api_format` | `api_format` | 非空。 |
| `connector_kind` | `text` | `openai_compatible` 或 `codex_oauth`。 |
| `request_compression` | `text` | `default` 或 `zstd`；默认 `default`，`zstd` 只允许 Responses group。 |
| `priority` | `integer` | 非空、非负；数值越小优先级越高。 |
| `selection_strategy` | `text` | `weighted_random` 或 `weighted_round_robin`。 |
| `enabled` | `boolean` | 非空，默认 `true`。 |
| `created_at` / `updated_at` | `timestamptz` | 通用时间列。 |

除主键外建立 `UNIQUE (id, api_format)`，使 `channels` 可通过组合外键保证格式一致。

### 4.6 `channels`

渠道的请求/响应变换和测试配置采用 JSONB，避免为每种 DSL 操作再建立表。最终请求顺序固定为：
**客户端白名单 → 模板默认值 → 渠道覆盖 → Codex body 白名单、隐私归一化与安全补全（如适用）→
网关移除客户端鉴权和 hop-by-hop Header → Codex Header 白名单、隐私归一化与安全补全（如适用）
→ 上游鉴权注入**。

请求 `content-encoding` 与 `accept-encoding` 由网关拥有，不能由 Transform 修改。Responses
group 选择 `request_compression = zstd` 时，最终 HTTP JSON body 在 Connector 适配后使用
Zstandard level 3 编码。

| 列 | 类型 | 说明 |
| --- | --- | --- |
| `id` | `uuid` | 主键。 |
| `channel_group_id` | `uuid` | 非空；与格式组成外键至 `channel_groups`。 |
| `api_format` | `api_format` | 非空。 |
| `name` | `varchar(100)` | 同一渠道组内唯一。 |
| `base_url` | `text` | 非空；必须为绝对 HTTP(S) URL。 |
| `enabled` | `boolean` | 非空，默认 `true`；管理启停。 |
| `supports_websocket` | `boolean` | 非空，默认 `false`；仅 Responses channel 可启用。 |
| `supports_standalone_web_search` | `boolean` | 非空，默认 `false`；仅 Responses channel 可启用，作为 `/v1/alpha/search` operation capability。 |
| `auto_disabled` | `boolean` | 非空，默认 `false`；持久化的不可用标记，运行时会排除该渠道。 |
| `auto_disabled_reason` | `varchar(500)` | 可空；仅 `auto_disabled = true` 时使用。 |
| `weight` | `integer` | 非空、正数。 |
| `proxy_id` | `uuid` | 可空，外键至 `proxies(id)`。 |
| `config_template_id` | `uuid` | 可空，外键至 `config_templates(id)`。 |
| `override_document` | `jsonb` | 非空，默认 `{}`；渠道变换和网络覆盖。 |
| `connect_timeout_ms` | `integer` | 可空、正数；显式渠道建连超时。 |
| `response_header_timeout_ms` | `integer` | 可空、正数；显式等待响应头超时。 |
| `stream_idle_timeout_ms` | `integer` | 可空、正数；显式流空闲超时。 |
| `upstream_auth_kind` | `text` | `none`、`bearer` 或 `header`。 |
| `upstream_auth_header_name` | `varchar(100)` | `header` 模式必填；不得为受保护或 hop-by-hop Header。 |
| `upstream_api_key` | `text` | `none` 时为空，其他模式非空；按 PRD 原始保存。 |
| `available_models` | `text[]` | 非空，默认空数组；已知可用上游模型名。 |
| `created_at` / `updated_at` | `timestamptz` | 通用时间列。 |

```sql
UNIQUE (channel_group_id, name);
UNIQUE (id, api_format);
FOREIGN KEY (channel_group_id, api_format)
  REFERENCES channel_groups (id, api_format);
CHECK (weight > 0);
CHECK (jsonb_typeof(override_document) = 'object');
```

渠道实际可选条件为 `enabled AND NOT auto_disabled`，并在内存中再过滤被动连接健康、熔断和冷却状态。重启后被动健康运行状态回到未知，后续请求重新建立状态。

每类超时按以下优先级取值：渠道显式列 → `system_settings.forwarding_policy.upstream` 默认值。
Images generation/edit 在渠道未显式配置 `response_header_timeout_ms` 时使用独立的
`images_response_header_timeout_seconds`；standalone web search 使用
`standalone_web_search_response_header_timeout_seconds`；其他 HTTP 请求使用
`response_header_timeout_seconds`。建连和流空闲默认值仍跨格式共享。只允许建连、响应头和流
空闲超时，禁止总响应超时。首次启动时，若该系统配置行不存在，二进制会用 TOML `[upstream]`
的值一次性初始化；后续 TOML 变更不会覆盖数据库。

### 4.7 `proxies`

| 列 | 类型 | 说明 |
| --- | --- | --- |
| `id` | `uuid` | 主键。 |
| `name` | `varchar(100)` | 非空、唯一。 |
| `proxy_url` | `text` | 非空；仅 HTTP(S)/SOCKS URL。 |
| `username` / `password` | `text` | 可空；代理认证原始值。 |
| `no_proxy_hosts` | `text[]` | 非空，默认空数组。 |
| `enabled` | `boolean` | 非空，默认 `true`。 |
| `created_at` / `updated_at` | `timestamptz` | 通用时间列。 |

代理是 `reqwest::Client` 级设置。运行时按代理、TLS 与最终超时策略构造并复用客户端，不因本表存在而在每次请求新建 client。

### 4.8 `config_templates`

| 列 | 类型 | 说明 |
| --- | --- | --- |
| `id` | `uuid` | 主键。 |
| `name` | `varchar(100)` | 非空、唯一。 |
| `description` | `text` | 可空。 |
| `document` | `jsonb` | 非空；模板默认变换，必须为对象。 |
| `enabled` | `boolean` | 非空，默认 `true`。 |
| `created_at` / `updated_at` | `timestamptz` | 通用时间列。 |

`document` 与 `channels.override_document` 采用同一受限 schema。版本 1 支持请求/响应 Header 的 `set`/`remove`/`rename` 和受限 JSON Patch；版本 2 在保留这些能力的基础上增加受限数组插入/删除、浅层对象合并、仅引用当前目标值的 `$ref`/`$template`，以及仅检查当前目标的条件执行。响应体改写仅作用于逐个 SSE `data:` JSON 事件，不缓冲或改写普通非流式 JSON 响应。详细语法见 [Transform DSL](transform-dsl.md)。不支持 JavaScript、Shell、任意模板执行、跨路径/请求头/路由上下文变量或网络超时默认值。

保存模板或渠道时，编译器必须验证 JSON 语法、操作白名单、Pointer、SSE 适配性以及最终合并结果。配置不得改写 `Host`、`Content-Length`、`Connection`、`Transfer-Encoding`、客户端 `Authorization`、`Proxy-Authorization` 或 `Connection` 动态声明的 Header。模板更新会立即影响引用它的渠道；系统没有版本回滚能力，依赖 `audit_logs` 查看变更。

无论是否启用变换，转发器都必须在上游请求前移除客户端 `Authorization`、所有 hop-by-hop Header、`Connection` 中动态声明的 Header，以及 [`request-allowlists.json`](../reference/request-allowlists.json) 在客户端 Header policy 中显式 `ignore` 的常见反向代理/CDN 转发元数据（`Forwarded`、`Via`、`X-Forwarded-*` 的常用字段、真实客户端 IP Header 与 Cloudflare 转发 Header）；随后才注入 `upstream_auth_kind` 指定的上游鉴权。同一显式 `ignore` policy 必须在 Header Transform 后及最终派发边界再次执行，不能依赖入口层删除；自定义上游鉴权 Header 名和 Connector 生成 Header 不得与该集合冲突。该规则由所有普通、Codex、HTTP/SSE、Images 与 Responses WebSocket 渠道共享，渠道模型发现和 scheduled probe 等 transform-capable 内部请求也不能绕过。上游响应转发给客户端前也必须移除所有 hop-by-hop Header 和 `Connection` 动态声明的 Header；响应经过任何变换后还必须移除原始 `Content-Length`。这些是网关强制行为，不可由 JSON 配置关闭或覆盖。

### 4.9 `request_logs`

一行表示一个逻辑客户端请求。当前网关只进行一次上游尝试，`attempts` 保持默认空数组；若未来引入首字节前重试，才以该 JSONB 保存已脱敏尝试摘要。首版不对该表分区，严格保持 PRD 的 11 张表边界；日志量成为实际瓶颈后，再以独立设计决策引入分区。

| 列 | 类型 | 说明 |
| --- | --- | --- |
| `id` | `uuid` | 主键；应用生成，也是日志 worker 的幂等键。 |
| `started_at` / `completed_at` | `timestamptz` | 请求开始与结束时间。 |
| `user_id` / `api_key_id` | `uuid` | 请求归属与认证 Key。 |
| `api_format` | `api_format` | 本次请求格式。 |
| `request_protocol` | `text` | `non_stream`、`sse` 或 `websocket`；表示客户端请求使用的传输方式。 |
| `client_model` / `upstream_model` | `varchar(300)` | 客户端与实际发送模型名。 |
| `reasoning_effort` | `varchar(32)` | 可空；从客户端原始请求的 `reasoning.effort` 或 `reasoning_effort` 提取并规范化。 |
| `fast_mode` | `boolean` | 未被用户组策略过滤的客户端是否显式请求 `service_tier = "priority"`；历史、定时测试和已过滤日志默认为 `false`。 |
| `model_rule_id` | `uuid` | 使用的模型规则。 |
| `channel_group_id` / `channel_id` | `uuid` | 最终命中的组和渠道；拒绝请求时可空。 |
| `outcome` | `text` | `succeeded`、`failed`、`rejected` 或 `cancelled`。 |
| `response_status_code` | `smallint` | 可空。 |
| `streamed` | `boolean` | 兼容字段；SSE 与 WebSocket 为 `true`，非流式请求为 `false`。 |
| `ttft_ms` / `total_duration_ms` | `integer` | 可空、非负。 |
| `output_tokens_per_second` | `numeric(14,4)` | 可空、非负；按输出 Token 除以总耗时减 TTFT 后的生成时间计算。 |
| `input_tokens` / `cached_input_tokens` / `cache_write_tokens` / `output_tokens` / `reasoning_tokens` | `bigint` | 可空、非负；`reasoning_tokens` 是 `output_tokens` 中用于推理的子集；未知保持 `NULL`。 |
| `model_id` | `uuid` | 实际计价模型。 |
| `currency` / `price_unit_tokens` / `price_effective_at` | 币种、整数、时间 | 本次价格快照上下文；币种固定为 `USD`。 |
| `input_unit_price` / `cached_input_unit_price` / `cache_write_unit_price` / `output_unit_price` | `numeric(24,12)` | 本次价格快照。 |
| `cost_amount` | `numeric(24,8)` | 可空、非负；本次最终费用。 |
| `attempts` | `jsonb` | 非空，默认 `[]`；为未来首字节前重试审计预留。当前网关只进行一次上游尝试，始终保留空数组。 |
| `error_code` / `error_summary` | `varchar(100)` / `varchar(16384)` | 可空；应用分别限制为 100 字节和 16KiB，并清理控制字符。 |
| `billed_at` | `timestamptz` | 可空；余额/额度已成功应用的唯一标志。 |

必须满足：

- `cached_input_tokens` 和 `cache_write_tokens` 非空时都不得大于 `input_tokens`。
- `reasoning_tokens` 非空时不得大于 `output_tokens`。
- 四个价格字段、币种、计价单位和价格生效时间要么全部为空（未计费），要么全部存在。
- 费用计算为：`(input - cached_input) * input_price / unit + cached_input * cached_input_price / unit + cache_write * cache_write_price / unit + output * output_price / unit`。
- 当前不写入 `attempts`，因为网关只进行一次上游尝试。若未来引入重试，只能在尚未收到上游任何字节时追加下一项；收到响应头或首字节后不得切换渠道或重试。
- 非流式失败 HTTP 响应会在不改变转发流的前提下保留最长 16KiB 的文本正文前缀；JSON 错误同时提取结构化代码，并保留消息与完整 JSON 详情。SSE/Responses WebSocket 的失败终态事件使用相同规则保存完整结构化事件。网关生成的错误保存客户端可见错误对象，底层传输失败还可追加 source chain。二进制媒体响应、请求正文、成功响应正文和 Header 不进入错误详情。
- 结算 worker 对 `cost_amount` 非空且 Key 归属一致的日志，以 `UPDATE ... WHERE id = ? AND billed_at IS NULL RETURNING cost_amount` 取得唯一结算权，再在同一事务更新 `users.balance_amount` 和 `api_keys.quota_used_amount`。事务失败会回滚 `billed_at`，因此 worker 重投、启动恢复和并发结算不会重复扣费；余额不足不阻止结算，余额可为负。缺失成本或归属不一致的日志不会被重试扫描错误地结算。系统只接受 USD 价格和费用，不在内部进行货币转换。

不保存请求体、成功响应体、完整 Header、任何密钥、Cookie 或原始 IP。失败响应详情可能包含上游回显的业务数据，必须按敏感日志处理。请求遥测清理前，必须确认它仍是首版唯一的计费审计依据；在引入独立账本前，不应任意缩短其保留期。

### 4.10 `audit_logs`

| 列 | 类型 | 说明 |
| --- | --- | --- |
| `id` | `uuid` | 主键。 |
| `occurred_at` | `timestamptz` | 非空。 |
| `actor_user_id` | `uuid` | 可空；系统任务为空。 |
| `actor_type` | `text` | `user` 或 `system`。 |
| `action` | `varchar(100)` | 例如 `channel.updated`。 |
| `object_type` / `object_id` | 文本 / `uuid` | 变更对象。 |
| `before_redacted` / `after_redacted` | `jsonb` | 可空；已脱敏的前后快照。 |
| `correlation_id` | `varchar(100)` | 可空；控制台请求关联。 |
| `reason` | `varchar(500)` | 可空；敏感操作建议必填。 |
| `source_ip_prefix` | `cidr` | 可空；按隐私策略截断。 |

控制面写入与相应审计日志必须在同一个事务中完成。脱敏器必须删除 `secret_value`、`upstream_api_key`、代理密码、`Authorization`、Cookie 与正文；不能先写完整行再让前端隐藏。

### 4.11 `system_settings`

| 列 | 类型 | 说明 |
| --- | --- | --- |
| `setting_key` | `varchar(100)` | 主键。 |
| `value` | `jsonb` | 非空对象；当前实现使用固定键 `forwarding_policy`。 |
| `updated_at` | `timestamptz` | 非空。 |

`forwarding_policy.upstream` 保存建连、普通响应头、Images 响应头和流空闲默认超时，单位秒；
其余对象保存请求重试、被动健康、自动禁用、定时测试、Session affinity、Responses
WebSocket、MCP transport，以及 Codex 合成 workspace path/HTTPS origin remote 等系统策略。
Console 管理员通过 `GET`/`PUT /console/v1/system/settings` 读取和更新，
使用 `ETag`/`If-Match` 并写入审计日志。保存时会和整个控制面一起编译并立即发布新快照；已在
处理中的请求保留取得快照时的超时，新的请求使用新值。首次启动只在此行缺失时从 TOML
bootstrap 配置初始化，之后数据库为唯一运行时来源。

重试、主动健康检查、硬额度、注册和日志保留策略仍属于后续设计，不能通过向该表写入未定义 JSON 启用。

## 5. 约束边界与请求处理

### 5.1 PostgreSQL 直接保证

- `UNIQUE (model_rules.client_model, model_rules.api_format)`。
- `channels(channel_group_id, api_format)` 组合外键指向 `channel_groups(id, api_format)`，阻止渠道跨格式归属。
- 所有普通外键、唯一性、非空、正数/非负金额、数组非空与 JSONB 对象检查。
- API Key 原始值唯一；同一用户的 Key 名称唯一。
- 日志的单价快照完整性和一次性 `billed_at` 结算更新。

### 5.2 控制面编译器保证

以下规则涉及数组、JSONB 或当前内存状态，必须在保存配置及构建快照时验证：

1. `model_rules` 中所有渠道组/渠道 ID 存在、无重复且与规则 `api_format` 相同；禁止跨格式回退。目标资源可以处于禁用状态。
2. 模型规则分别编译目标渠道、模型兼容渠道和活跃候选。目标渠道由显式 `channel_ids` 与
   `channel_group_ids` 的当前成员展开；模型兼容渠道再按 `channels.available_models` 与规则
   `upstream_model` 求交；活跃候选还要求渠道组、渠道均启用且渠道未被自动禁用。直接渠道和
   渠道组产生的重复目标按渠道 ID 去重。
3. 启用规则可以没有模型兼容渠道，或暂时没有活跃候选；前者为 `disconnected`，后者为
   `temporarily_unavailable`，都可以保存和发布。只有实际进入同一活跃优先级 tier 的渠道组
   必须使用相同选路策略。缺失引用、跨格式引用和无法确定活跃 tier 策略仍是结构错误。
4. API Key 的 `allowed_group_ids` / `allowed_channel_ids` 均存在、无重复，并且 Key 的自动推导格式覆盖这些目标；`proxy` 权限用于代理，`/v1/models` 还需要 `models.read`。
5. API Key 的渠道组/渠道范围展开为 dense channel slot 位图；相同范围共享一个
   `AuthorizationProfile`。`accessible_routes` 通常按模型兼容渠道预计算；只有规则全局没有
   模型兼容渠道时才退回目标渠道，因此原本已授权客户端直接请求断开规则会得到路由不可用，
   同时不会向仅获准其他不兼容渠道的 Key 暴露仍可由别处提供的模型。`/v1/models` 还要求
   API Key 范围与模型兼容渠道相交，因此不会公布 `disconnected` 规则。临时禁用和被动健康冷却
   不会把仍有模型兼容目标的规则从模型列表移除。
6. 模板和渠道覆盖符合受限 DSL、Header 保护、SSE 逐事件处理、URL 和超时规则；
   `forwarding_policy` 的字段必须有效：普通与 Images 响应头超时都必须大于建连超时，Codex
   workspace path 必须是非空绝对路径，Codex remote 必须是无凭证/查询/fragment 的 HTTPS
   repository URL，且所有渠道覆盖与对应格式默认值合并后的有效超时均有效。

控制面在一个事务中保存变更和审计日志，完成上述全量校验并编译 `CompiledRuntimeConfig` 后提交；提交后直接替换内存 `ArcSwap` 快照。启动、定时重载和 Console 管理写入均使用 PostgreSQL 控制面；TOML 保留进程级监听、数据库、系统设置首次初始化值、日志、Console listener 和 JWT 密钥文件路径设置。

### 5.3 数据面流程

```text
Bearer Key
  → 内存快照校验 Key 状态、格式、权限和分组；软额度预检查 `used >= limit` 时拒绝
  → 客户端 Header 与顶层 body 白名单
  → 根据 (api_format, client_model) 从分格式索引找到 model_rule
  → 通过 accessible_routes 位图完成模型授权判断
  → 使用已编译的模型候选 tier，按渠道授权位图、组优先级、渠道权重和内存健康状态选择渠道
  → 模板默认值 + 渠道覆盖
  → Codex body 白名单（Codex only）
  → Codex installation/workspace body 隐私归一化与安全补全（Codex only）
  → Header 清理 + Codex Header 白名单、隐私归一化与安全补全（Codex only）+ 上游鉴权
  → 原路径流式转发
  → 异步写入一条 request_logs；对已持久化的可结算成本幂等更新余额和已用额度
```

只要客户端/Connector policy 未删除或覆盖字段、没有变换，且不存在
`client_model != upstream_model` 的别名映射，请求体仍使用原始字节。普通流式响应不缓冲；经过
SSE 变换时按 `data:` 事件边界处理。

## 6. 索引、保留与取舍

建议的首版索引：

| 表 | 索引 |
| --- | --- |
| `api_keys` | `UNIQUE(secret_value)`、`(user_id, status)`。 |
| `models` | `UNIQUE(source_model_id)`。 |
| `model_rules` | `UNIQUE(client_model, api_format)`。 |
| `channels` | `UNIQUE(channel_group_id, name)`、`(channel_group_id, enabled)`。 |
| `request_logs` | `(api_key_id, started_at DESC)`、`(user_id, started_at DESC)`、`(channel_id, started_at DESC)`、失败请求的部分索引，以及未结算成本的部分恢复索引。 |
| `audit_logs` | `(object_type, object_id, occurred_at DESC)`、`(actor_user_id, occurred_at DESC)`。 |

`request_logs` 是普通表；当前没有保留任务或 `system_settings.log_retention_policy` 的运行时实现。日志量成为实际瓶颈后，再设计显式保留策略并评估按月分区及其迁移方案，不能在“严格 11 张表”的首版中预先引入分区表。

简化方案明确放弃以下能力：

- 数组 ID 没有数据库外键；完整性依赖控制面发布器。
- 不保留模型价格、模板或渠道配置的版本历史；`audit_logs` 只用于查阅变更，不能完整回滚配置。
- 同一规则不能针对不同目标设置不同上游模型名或不同计价模型。
- 不存在独立账本；`request_logs` 和 `users.balance_amount` 是首版计费依据，财务对账/退款复杂后再拆分。
- 渠道运行状态、分钟级限流计数和并发数不跨重启、不跨实例；PRD 当前只考虑单实例，这一限制可接受。额度已用金额会持久化并在同进程结算后立即影响软预检查；仍不提供硬额度保留。已持久化但未结算日志可恢复，异步队列丢弃或日志落库前的进程崩溃不具备持久化恢复基础。

这些限制都是有意识地换取更少的表、更少的迁移和更直观的控制面。后续只在业务确实需要某项能力时，再将对应字段拆为独立表。
