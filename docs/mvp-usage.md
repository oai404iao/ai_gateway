# 运行与接口说明

服务是一个 OpenAI 兼容的数据面网关，加上独立的 **Console API**。`/v1/*` 面向 SDK 和程序调用，使用用户 API Key；`/console/v1/*` 面向用户登录和控制面管理，使用 JWT。`admin` 是用户角色，不是另一套接口或静态 Bearer 凭据。

当前运行时同时提供 Console API 和可选的浏览器管理界面。Console API 仍是程序化接口；
浏览器管理界面已实现于 `web/console/`，可通过 `embedded-console-ui` Cargo feature 嵌入并由
Console listener 提供。无论是否启用 UI，本文件描述的 API 行为与边界保持不变。设计详情见
[Console Web UI 设计与实施计划](console-ui-design.md)。

## 启动

1. 创建本地数据库密码和运行配置。服务不使用 XDG 配置目录：

   ```bash
   mkdir -p ./config
   openssl rand -hex 32 > ./config/postgres-password
   chmod 600 ./config/postgres-password
   cp config.example.toml ./config/config.toml
   ```

   默认配置通过 `[database].password_file` 读取该密码，不在 TOML 或
   Compose 中内置弱密码。
2. 启动经过单节点生产基线调优的 PostgreSQL：`docker compose up -d`。
   它不提供 HA、PITR 或自动备份；机器规格分档和参数覆盖方式见
   [生产配置与容量调优](production-configuration.md)。

   从旧根目录布局升级时，将 `./config.toml` 和
   `./console-jwt-*.pem` 移入 `./config/`。
3. 首次部署时，使用受控的一次性 CLI 创建首个管理员。密码必须经标准输入传入：

   ```bash
   cargo run -- bootstrap-admin \
     --email admin@example.com \
     --display-name "Initial Admin" \
     --password-stdin < password.txt
   ```

   该命令仅在不存在 `active admin` 时成功，并自动执行数据库迁移。
4. 启动服务：`cargo run`。

启动时服务会应用 migration、从 PostgreSQL 编译不可变数据面快照、启动配置重载和请求日志 worker。空控制面可以启动，但没有有效 API Key 和路由规则时无法代理请求。

服务不读取 dotenv。JWT Ed25519 私钥和公钥通过受限文件路径配置，不写入 TOML。

### 紧急重置管理员密码

Console 密码最少为 12 个字节；前端和后端都会拒绝更短的密码。若现有
`active admin` 因短密码或遗失密码无法登录，可在拥有配置文件和数据库访问权限的主机上执行：

```bash
cargo run -- reset-admin-password \
  --email admin@example.com \
  --password-stdin < new-password.txt
```

也可在命令末尾加 `--config ./config/other-config.toml`。该命令只会重置匹配邮箱的
`active admin`，新密码经 Argon2 哈希保存，并立即撤销该用户的所有 Console 会话；不会输出
密码或哈希。请确保标准输入中的新密码至少为 12 个字节，并妥善保护或删除临时密码文件。

## 监听器与请求体限制

```toml
[server]
host = "127.0.0.1"
port = 3000

[request_limits]
proxy_body_bytes = 1_048_576
console_body_bytes = 262_144
auth_body_bytes = 16_384

[console]
enabled = true
host = "127.0.0.1"
port = 3001
allowed_origins = ["https://console.example.com"]

[auth]
issuer = "ai-gateway"
audience = "ai-gateway-console"
access_token_ttl_seconds = 900
refresh_token_ttl_seconds = 2_592_000
key_id = "primary-2026"
signing_key_path = "./config/console-jwt-private.pem"
verification_key_path = "./config/console-jwt-public.pem"
```

- 公共数据面默认监听 `127.0.0.1:3000`。
- Console 是独立监听器；应仅通过 HTTPS 反向代理对外暴露。
- `proxy_body_bytes` 限制 OpenAI 代理请求；`console_body_bytes` 限制已认证 Console 写操作；`auth_body_bytes` 限制登录、刷新和邀请激活请求。
- 旧的 `[server].max_request_body_bytes` 仍作为 `proxy_body_bytes` 的兼容别名；新配置应使用 `[request_limits]`。

## 公共数据面

- `GET /health`：返回 `204`，无需认证。
- `GET /v1/models`：列出当前 API Key 可达的模型；需要相应格式的 `proxy` 和 `models.read` 权限。
- `POST /v1/chat/completions`：仅匹配 Chat Completions 路由规则。
- `POST /v1/responses`：仅匹配 Responses 路由规则。

两个 OpenAI 格式绝不互相回退。客户端 `Authorization` 不会转发给上游；网关清理 hop-by-hop headers 后，按渠道配置最后注入上游认证。

数据面在认证后、读取请求体前执行 RPM、并发与已结算软额度预检查。请求体只有在模型别名或 JSON 变换启用时才重新序列化；响应默认逐块流式转发，SSE 变换按事件边界执行且不缓冲整条流。连接失败、连接超时或等待响应头超时时，可以按系统设置在尚未尝试过的其他健康渠道上故障转移；一旦收到上游响应头或向客户端发送任何响应字节，绝不重试或切换渠道。

## Console 认证

Console 登录接口：

- `POST /console/v1/auth/login`
- `POST /console/v1/auth/refresh`
- `POST /console/v1/auth/activate-invitation`
- `POST /console/v1/auth/logout`（需要 access JWT）

登录或邀请激活成功后：

- 响应 JSON 返回短期 Access JWT，客户端以 `Authorization: Bearer <token>` 调用 Console API；
- 响应设置轮换的 `HttpOnly; Secure; SameSite=Lax` refresh Cookie；
- refresh token 仅保存 SHA-256 哈希。刷新时会轮换；重放旧 refresh token 会撤销该 session；
- 每个 Console 请求都会验证 JWT 签名、issuer、audience、用户状态、session 状态和 `auth_version`。禁用用户、改密码、登出和角色变化会立即使旧 token 失效。

用户由管理员邀请创建。邀请响应中的 `invitation_token` 只返回一次，外部邮件/通知系统负责投递。激活邀请后用户设置自己的密码；管理员不提交或保存用户明文密码。

## 普通用户接口

所有下列资源均强制从 JWT 主体推导 user ID，不能通过路径或 body 参数访问他人的数据：

- `GET/PATCH /console/v1/me`
- `POST /console/v1/me/password`
- `GET /console/v1/me/sessions`
- `DELETE /console/v1/me/sessions/{id}`
- `GET/POST /console/v1/me/api-keys`
- `GET /console/v1/me/api-key-options`
- `GET/PUT /console/v1/me/api-keys/{id}`
- `POST /console/v1/me/api-keys/{id}/revoke`
- `GET /console/v1/me/request-logs?limit=50`
- `GET /console/v1/me/request-logs/{id}`

管理员分配的默认 `api_key_policy` 只定义用户可选择的渠道组和单独渠道。用户通过
`GET /console/v1/me/api-key-options` 获取当前可选列表；创建或更新 API Key 时，从该列表中选择
`allowed_group_ids` / `allowed_channel_ids`，并为该 Key 独立配置 RPM、最大并发和可选额度上限。
API 格式由所选目标自动推导，自助创建 Key 的权限固定为 `proxy` 和 `models.read`。

Policy 不再保存额度、RPM、并发、格式、权限或最大活动 Key 数，也不会反向修改既有 Key 的实际限制。
未分配策略、策略已禁用或提交了策略范围外的目标时，接口分别返回
`default_api_key_policy_required`、`default_api_key_policy_disabled` 或
`api_key_target_not_allowed`。

## 管理员接口

拥有 `role = admin` 的用户可使用全部普通用户接口，以及以下 Console 控制面接口：

- 用户与邀请：`/console/v1/users`
- API Key Policy：`/console/v1/api-key-policies`
- 全局 API Key：`/console/v1/api-keys`
- 模型：`/console/v1/models`
- models.dev：`/console/v1/catalog/models/sync/preview`、`/sync`、`/import`
- 路由：`/console/v1/routing/channel-groups`、`/channels`、`/model-rules`
- 网络：`/console/v1/network/proxies`
- 变换模板：`/console/v1/transforms/templates`
- 观测事实：`GET /console/v1/request-logs`、`GET /console/v1/audit-logs`
- 系统转发设置：`GET` / `PUT /console/v1/system/settings`（管理员；`PUT` 使用 `If-Match`，保存后立即发布快照）
- 手动重载：`POST /console/v1/system/reload`

大多数可更新资源遵循 `GET` 返回 `ETag`、`PUT` 携带 `If-Match` 的乐观并发模型。控制面写入在 serializable 事务中再次确认 actor 仍为 active admin，校验完整候选快照、写入脱敏审计记录，并在提交后立即发布运行时快照。

为迁移旧控制台客户端，`/console/v1/channel-groups`、`/channels`、`/model-rules`、`/proxies`、`/config-templates`、`/models/sync/*` 和 `/reload` 仍有同一 JWT/角色边界下的 Console 别名；`/admin/v1/*` 不再存在。

## 自动禁用与定时测试

`/console/v1/system/settings` 的完整配置还包含：

- `request_retry.enabled`：是否启用响应头前故障转移，默认启用。
- `request_retry.max_retries`：首次请求失败后的最大自动重试次数，范围 `1..=10`，默认 `1`。同一客户端请求不会重复尝试同一渠道。
- `automatic_disable.enabled`：自动禁用总开关。关闭时，即使渠道允许自动禁用也不会执行状态变更。
- `automatic_disable.error_status_codes`：触发临时禁用的上游 HTTP 状态码列表。
- `automatic_disable.error_message_keywords`：触发临时禁用的上游错误消息关键字；匹配大小写不敏感。自动禁用扫描器不会保存被扫描的响应正文；仅当 SSE 协议解析器识别出结构化错误事件时，请求日志才保存受限、已清洗的错误代码与消息摘要。
- `scheduled_testing.mode`：`global` 测试全部启用渠道；`failure_only` 只测试临时自动禁用的渠道。
- `scheduled_testing.auto_recover`：测试成功后是否自动清除临时禁用。
- `scheduled_testing.interval_minutes`：测试间隔，默认 `5`。
- `scheduled_testing.prompt`：测试 prompt，默认 `reply '1'`。

渠道的 `auto_disable_allowed` 必须为 true 才会被自动禁用；`test_model` 必须从该渠道的
`available_models` 中选择。定时测试按渠道 API 格式发出非流式 Chat Completions 或 Responses 请求，
并复用该渠道的代理、超时、变换和上游鉴权配置。手工禁用的渠道与禁用渠道组不会被测试。

定时测试日志写入 `request_logs`，`request_source` 为 `scheduled_test`。它们使用系统内置、
管理员角色的内部 API Key，不计入任何普通用户的费用或额度；系统内部身份不会出现在用户和 API Key
管理列表中。自动禁用和自动恢复都会写入系统审计日志并立即发布新的路由快照。

## Session 粘性

`/console/v1/system/settings` 的 `session_affinity` 可以按请求 Header 或 JSON Pointer
提取 Session Key，并优先复用该 Session 最后一次成功请求所使用的渠道。规则按配置顺序执行，
第一个成功提取非空标量值的规则生效。

- 缓存 Key 自动按规则、API Key 和模型规则隔离，原始 Session Key 只用于计算 SHA-256，
  不写入数据库、请求日志或审计详情。
- 缓存有 TTL 和最大条目数，只存在于当前 Gateway 进程。
- 命中的渠道仍须满足当前授权、模型候选、最低可用优先级和被动健康状态，否则删除旧映射并执行普通选路。
- 只有完整成功的 2xx 请求才写入或刷新映射；上游失败会删除本次命中的旧映射。
- Session 粘性本身不增加尝试次数；如果全局请求故障转移已启用，失败的粘性渠道会从本次请求的候选中排除，并清除命中的旧映射。
- JSON 来源使用 RFC 6901 Pointer，例如 Responses 请求的 `/prompt_cache_key`。

多实例部署没有共享粘性缓存；若同一 Session 被负载均衡到不同 Gateway 进程，各实例会独立学习渠道。

## 日志、用量与结算

每次故障转移会产生 `proxy_request_retry` tracing 事件；每个客户端请求仍只产生一个终态 tracing
事件和一条 `request_logs`，其中渠道、结果和计费快照对应最终尝试。worker 从两种格式的普通 JSON
或 SSE 事件增量提取 usage，在选路时绑定价格快照，并在可结算时以 `billed_at` 条件幂等更新用户余额
和 API Key 已用额度。

额度是软预检查：不预留金额，已结算额度达到上限后才拒绝后续请求；余额可以为负。队列饱和时请求日志可被丢弃，但不会阻塞或破坏代理响应。

## 已知边界

- 仅支持 Chat Completions 与 Responses，不提供 OpenAI 的 embeddings、images、audio、files、batches、assistants 或 fine-tuning API。
- 所有余额、额度、模型价格和请求费用统一使用 USD；没有跨实例限流、健康状态或 Session 粘性协调。自动重试仅覆盖收到响应头前的连接失败、连接超时和响应头超时，不覆盖 HTTP 错误、SSE 流中断或流空闲超时；也没有独立财务账本、充值/退款或货币兑换。
- 服务本身不终止 TLS；Console 必须部署在正确配置的 HTTPS 反向代理后。
