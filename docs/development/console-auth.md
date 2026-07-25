# Console API、JWT 与角色授权重构计划

> 状态：已完成设计记录。本文记录将静态管理 Bearer 接口重构为用户登录、JWT 鉴权及角色授权 Console API 的设计与实施清单。
>
> **后续变更：** migration `0010_api_key_target_selection.sql` 已替换本文最初的 Policy 模板语义。
> 当前行为以 `docs/user/operations.md` 和 Console OpenAPI 为准。

## 1. 目标

1. 保持 OpenAI 数据面兼容：`/v1/models`、`/v1/chat/completions`、`/v1/responses` 仍使用用户 API Key，不改为 JWT。
2. 移除“Admin API”这一产品边界：新增独立监听器上的 **Console API**，统一使用 `/console/v1/*` 路由。
3. `admin` 仅是 `users.role` 的角色值；普通用户与管理员使用同一登录机制，但能访问的路由不同。
4. 普通用户只能访问个人资料、安全设置、自己的 API Key、自己的请求日志和用量/费用事实。
5. 管理员在上述能力之上拥有用户、API Key Policy、模型、路由、渠道、代理、模板、目录同步、全局日志、审计和重载等完整控制面权限。
6. 将代理、Console 与认证请求的 body 上限分别配置。

## 2. 已确认的产品决策

| 主题 | 决策 |
| --- | --- |
| 账户创建 | 邀请制；首个管理员通过一次性 Bootstrap CLI 创建 |
| 会话 | 短期 Ed25519/EdDSA JWT access token + 轮换 opaque refresh token Cookie |
| Console 部署 | 独立 Console listener，由 HTTPS 反向代理对外暴露 |
| 请求体上限 | proxy、console、auth 三类独立配置 |

服务不读取 dotenv。JWT 密钥通过受限文件路径配置，不把私钥写入 TOML。

## 3. 目标拓扑

```text
OpenAI data plane                         Console API
------------------                        ------------------------
/v1/* + API Key                           /console/v1/* + JWT
server listener                            console listener
no database query per proxy request       database/session checks allowed

                 PostgreSQL control plane + auth/session state
```

Console listener 只应部署在 TLS 反向代理后。网关仅允许显式配置的 CORS origins，不允许通配 origin 与 credential 组合。

## 4. 配置设计

```toml
[request_limits]
proxy_body_bytes = 1_048_576
console_body_bytes = 262_144
auth_body_bytes = 16_384

[console]
enabled = true
host = "127.0.0.1"
port = 3001
allowed_origins = ["https://console.example.test"]

[auth]
issuer = "ai-gateway"
audience = "ai-gateway-console"
access_token_ttl_seconds = 900
refresh_token_ttl_seconds = 2_592_000
key_id = "primary-2026"
signing_key_path = "/run/secrets/ai-gateway-jwt-private.pem"
verification_key_path = "/run/secrets/ai-gateway-jwt-public.pem"
```

## 5. 数据模型

### 5.1 users

保留余额和状态；用户不再拥有币种设置，所有金额统一按 USD 结算。新增或调整：

- `email`：大小写无关唯一；已有部署允许暂时为 NULL，未设置凭据的遗留用户不能登录。
- `display_name`：原 `name` 列迁移而来。
- `role`：`user` 或 `admin`。
- `password_hash`：Argon2id 哈希，不保存明文密码。
- `auth_version`：认证状态变更时递增，用于立即作废旧 JWT。
- `password_changed_at`：可选安全审计字段。
- `default_api_key_policy_id`：普通用户自助创建 API Key 的默认策略。

邀请中的用户使用 `status = 'invited'`；接受邀请后变为 `active`。

### 5.2 user_sessions

存储 refresh token 的哈希、会话状态、过期时间和轮换时间。refresh token 永不明文入库；每次刷新必须轮换，重放旧 token 将撤销该会话。

### 5.3 user_invitations

管理员创建用户时产生一次性、过期的邀请 token 哈希。token 只在创建响应中返回一次；邮件发送由
外部系统负责。未设置密码的待激活、暂停或禁用用户可重新签发邀请；重新签发会设置
`revoked_at` 撤销所有旧邀请，将用户恢复为 `invited`，并返回新的单次可见 token。

### 5.4 api_key_policies

管理员定义普通用户创建或调整 API Key 时可选择的渠道组和单独渠道。Policy 只表达资源选择上界，
不再保存格式、权限、RPM、并发、额度或最大活动 Key 数。

## 6. 认证和授权

### 6.1 Token

Access JWT claims：`sub`、`sid`、`role`、`auth_version`、`iss`、`aud`、`iat`、`exp`、`jti`。

- Access JWT 通过 `Authorization: Bearer` 使用。
- refresh token 是高熵 opaque token，使用 `HttpOnly; Secure; SameSite=Lax` Cookie。
- JWT 验签后，Console middleware 仍查询用户和 session，验证用户状态、角色、session 撤销状态和 `auth_version`；因此禁用、改密、登出、角色降级可立即生效。
- 登录及密码验证必须限流，密码字段不得进入日志、审计或错误响应。

### 6.2 角色

| 路由类别 | user | admin |
| --- | --- | --- |
| `/console/v1/me/*` | 是 | 是 |
| `/console/v1/users/*` | 否 | 是 |
| `/console/v1/api-key-policies/*` | 否 | 是 |
| 模型、路由、渠道、代理、模板、同步、重载 | 否 | 是 |
| 全局请求日志和审计 | 否 | 是 |

所有自助查询都以认证主体 ID 作为 SQL 过滤条件，不能信任路径或 body 中的用户 ID。控制面写操作在 `SERIALIZABLE` 事务内再次确认 actor 仍为 active admin，避免 TOCTOU。

## 7. 路由设计

### 公共认证

```text
POST /console/v1/auth/login
POST /console/v1/auth/refresh
POST /console/v1/auth/logout
POST /console/v1/auth/activate-invitation
```

### 本人资源

```text
GET/PATCH /console/v1/me
PUT       /console/v1/me/password
GET/DELETE /console/v1/me/sessions[/{{id}}]
GET/POST  /console/v1/me/api-keys
GET/PUT   /console/v1/me/api-keys/{{id}}
POST      /console/v1/me/api-keys/{{id}}/revoke
GET       /console/v1/me/request-logs
GET       /console/v1/me/request-logs/{{id}}
```

### 管理员资源（没有 `/admin` 路径）

```text
/console/v1/users
/console/v1/api-key-policies
/console/v1/models
/console/v1/catalog/*
/console/v1/routing/channel-groups
/console/v1/routing/channels
/console/v1/routing/model-rules
/console/v1/network/proxies
/console/v1/transforms/templates
/console/v1/request-logs
/console/v1/audit-logs
/console/v1/system/reload
```

## 8. API Key 自助服务规则

普通用户不可传入 `user_id`、API 格式或权限。用户从默认 Policy 允许的列表中选择
`allowed_group_ids` / `allowed_channel_ids`，并可在创建和后续更新时设置该 Key 的 RPM、并发和额度。
格式由目标自动推导，自助创建权限固定为 `proxy`、`models.read`；Policy 更新不反向改写既有 Key。

## 9. 迁移与兼容

1. 添加新 schema，同时允许旧用户缺失 email/password，以便升级不会立即阻断数据面。
2. 通过 `bootstrap-admin` CLI 创建或迁移首个登录管理员；禁止通过 TOML 固定 token 提权。
3. `/v1/*` 保持兼容。
4. `/admin/v1/*` 移除而非保留静态 token 兼容路径；Console API 从 `/console/v1` 开始。
5. `src/http/admin.rs` 和静态 `AdminState` 已移除；HTTP 使用 Console 语义，控制面 mutation/list DTO 使用 ControlPlane 语义。

## 10. 实施阶段

- [x] P0：写入本设计、拆分 request-limit 配置、删除静态 admin bootstrap 配置模型。
- [x] P1：数据库迁移：用户登录字段、session、invitation、API Key policy。
- [x] P2：实现密码、JWT、refresh token、session repository 和 Bootstrap CLI。
- [x] P3：实现独立 Console listener、CORS、认证 middleware、角色 middleware。
- [x] P4：实现认证端点和本人资料/会话/API Key/日志端点，确保资源归属过滤。
- [x] P5：将原有完整控制面路由迁移至 Console 管理员权限边界；transaction 内复核 admin 角色。
- [x] P6：更新文档、配置、迁移测试、认证/越权/刷新重放/413 测试；运行格式、lint、测试和必要的真实上游 smoke。

## 11. 验收标准

- 普通用户无法读取、更新、撤销其他用户的 Key、日志或会话。
- 普通用户不能通过 body 参数扩大 API Key 权限。
- 用户禁用、改密、登出、session 重放和角色降级均使对应 access token 失效。
- 管理员保有完整控制面能力，但所有操作审计到实际 JWT actor。
- `/v1/*` 不查询数据库来验证 JWT，继续只依赖 API Key 和配置快照。
- 三类 body limit 在各自路由返回安全的 413，且互不影响。
