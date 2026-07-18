# 运行说明（MVP 4 阶段 1）

服务以 PostgreSQL 作为控制面，TOML 只包含监听、数据库、默认上游超时、重载、日志和本地管理监听器设置。动态 API Key、用户、模型、路由、渠道、代理和模板均不允许写入 TOML。

## 启动

1. 启动 PostgreSQL：`docker compose up -d`。
2. 复制 `config.example.toml` 为已忽略的 `config.local.toml`，并填写进程级设置。
3. 首次启用管理监听器前，必须由受控的数据库 provisioning 流程创建一个 `users.status = 'active'` 的 bootstrap actor，并将其 UUID 配置为 `[admin].actor_user_id`。这是管理信任根；当前二进制没有创建首个 actor 的 CLI。
4. 启用 `[admin]` 时必须使用 loopback 地址和至少 32 字符的高熵 Bearer token；管理接口不会与公共监听器共享路由。
5. 运行 `cargo run -- config.local.toml`。启动时应用迁移、读取一致性控制面快照，并按配置周期重载。空控制面可以启动，但会拒绝客户端鉴权。

启动管理监听器后，先创建普通用户和模型，再创建渠道组、渠道、模型规则和 API Key；除 bootstrap actor 外，无需直接 SQL 创建这些控制面资源。

## 公共接口

- `GET /health`：返回 `204`。
- `GET /v1/models`：返回当前 API Key 可达的模型规则，需要 `proxy` 和 `models.read` 权限。
- `POST /v1/chat/completions`：仅匹配 Chat Completions 规则。
- `POST /v1/responses`：仅匹配 Responses 规则。

两个格式绝不互相回退。请求默认保留原始 JSON bytes；只有模型别名或已配置的 JSON 变换才会重新序列化。响应以字节流转发；SSE 变换按事件边界执行，不缓冲整条流。

## 本地管理接口

管理接口绑定独立的 loopback listener，所有请求需要 bootstrap Bearer token，并返回 `Cache-Control: no-store`。

- `GET` / `POST` `/admin/v1/users`
- `GET` / `PUT` `/admin/v1/users/{id}`
- `GET` / `POST` `/admin/v1/models`
- `GET` / `PUT` `/admin/v1/models/{id}`
- `GET` / `POST` `/admin/v1/api-keys`
- `GET` / `PUT` `/admin/v1/api-keys/{id}`，`POST /admin/v1/api-keys/{id}/revoke`
- `GET` / `POST` `/admin/v1/channel-groups`，`GET` / `PUT` `/admin/v1/channel-groups/{id}`
- `GET` / `POST` `/admin/v1/channels`，`GET` / `PUT` `/admin/v1/channels/{id}`
- `GET` / `POST` `/admin/v1/model-rules`，`GET` / `PUT` `/admin/v1/model-rules/{id}`
- `GET` / `POST` `/admin/v1/proxies`，`GET` / `PUT` `/admin/v1/proxies/{id}`
- `GET` / `POST` `/admin/v1/config-templates`，`GET` / `PUT` `/admin/v1/config-templates/{id}`
- `POST /admin/v1/reload`

`PUT` 需要先通过 `GET` 获取 `ETag`，然后以 `If-Match` 提交当前版本。所有写入都在一个 serializable 事务中校验完整候选快照、写入 allowlist 审计记录，并在提交后立即替换运行时快照；校验或版本冲突不会改变数据库、审计或当前快照。

用户的 `balance_amount` 是只读字段，MVP 4 阶段 1 不提供余额调整或结算。模型创建时可提供 `source_payload` JSON object；常规列表、读取和审计记录均不返回该不透明字段。模型更新时省略 `source_payload` 会保留已存数据，显式提供 `{}` 才会清空它。

停用用户会使其 API Key 在新快照中立即失效。停用仍被启用模型规则引用的模型会使候选快照无效，因此整个写入和审计都会回滚。

## 数据面行为

每个可用 API Key 需要对应格式的 `proxy` 权限。网关在读取请求体前执行 RPM、并发和已结算软额度预检查；`tokens_per_minute` 仍未支持。路由按最低优先级、同优先级权重和被动健康状态选择一次渠道，绝不自动重试或在响应头后切换渠道。

请求和响应可应用模板与渠道的受限 Header / JSON / SSE 变换。客户端 `Authorization`、hop-by-hop headers、路由和长度相关 headers 始终受保护；上游认证最后注入。HTTP/SOCKS 出口代理和有效连接超时决定复用的 reqwest 客户端。

每个已选路请求会发出 tracing 终态事件，并尽力异步写入一条 `request_logs` 记录。当前日志不解析 token usage、不计算费用，也不会更新用户余额或 API Key 已用额度；这些属于 MVP 4 后续阶段。

收到 SIGTERM 或 Ctrl-C 后，服务停止接收新连接，并在 `[server] shutdown_grace_period_seconds` 内等待在途连接；超过期限的响应被取消。请求日志 worker 随后进行有界排空。
