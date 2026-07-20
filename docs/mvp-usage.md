# 运行与接口说明

服务是一个 OpenAI 兼容的数据面网关，加上独立的 **Console API**。`/v1/*` 面向 SDK 和程序调用，使用用户 API Key；`/console/v1/*` 面向用户登录和控制面管理，使用 JWT。`admin` 是用户角色，不是另一套接口或静态 Bearer 凭据。

当前运行时同时提供 Console API 和可选的浏览器管理界面。Console API 仍是程序化接口；
浏览器管理界面已实现于 `web/console/`，可通过 `embedded-console-ui` Cargo feature 嵌入并由
Console listener 提供。无论是否启用 UI，本文件描述的 API 行为与边界保持不变。设计详情见
[Console Web UI 设计与实施计划](console-ui-design.md)。

## 启动

1. 启动 PostgreSQL：`docker compose up -d`。
2. 创建当前目录下已忽略的 `./config/`，并复制 `config.example.toml` 为 `./config/config.toml`；填写监听、数据库、上游和可选 Console 设置。服务不使用 XDG 配置目录：

   ```bash
   mkdir -p ./config
   cp config.example.toml ./config/config.toml
   ```

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

数据面在认证后、读取请求体前执行 RPM、并发与已结算软额度预检查。请求体只有在模型别名或 JSON 变换启用时才重新序列化；响应默认逐块流式转发，SSE 变换按事件边界执行且不缓冲整条流。每个请求只选择一次渠道，绝不在响应头或响应字节发送后重试或切换渠道。

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
- `GET/PUT /console/v1/me/api-keys/{id}`
- `POST /console/v1/me/api-keys/{id}/revoke`
- `GET /console/v1/me/request-logs?limit=50`
- `GET /console/v1/me/request-logs/{id}`

普通用户创建 API Key 时只能设置名称和过期时间。格式、权限、渠道组、RPM、并发和额度由管理员分配的默认 `api_key_policy` 决定；用户不能通过 API body 扩大权限。策略还限制最大活动 Key 数。

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
- 手动重载：`POST /console/v1/system/reload`

大多数可更新资源遵循 `GET` 返回 `ETag`、`PUT` 携带 `If-Match` 的乐观并发模型。控制面写入在 serializable 事务中再次确认 actor 仍为 active admin，校验完整候选快照、写入脱敏审计记录，并在提交后立即发布运行时快照。

为迁移旧控制台客户端，`/console/v1/channel-groups`、`/channels`、`/model-rules`、`/proxies`、`/config-templates`、`/models/sync/*` 和 `/reload` 仍有同一 JWT/角色边界下的 Console 别名；`/admin/v1/*` 不再存在。

## 日志、用量与结算

每个已选路请求会产生终态 tracing 事件，并尽力异步写入一条 `request_logs`。worker 从两种格式的普通 JSON 或 SSE 事件增量提取 usage，在选路时绑定价格快照，并在可结算时以 `billed_at` 条件幂等更新用户余额和 API Key 已用额度。

额度是软预检查：不预留金额，已结算额度达到上限后才拒绝后续请求；余额可以为负。队列饱和时请求日志可被丢弃，但不会阻塞或破坏代理响应。

## 已知边界

- 仅支持 Chat Completions 与 Responses，不提供 OpenAI 的 embeddings、images、audio、files、batches、assistants 或 fine-tuning API。
- 没有主动健康检查、跨实例限流/配置协调、通用自动重试、独立财务账本、充值/退款或多币种兑换。
- 服务本身不终止 TLS；Console 必须部署在正确配置的 HTTPS 反向代理后。
