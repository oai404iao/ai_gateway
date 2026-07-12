# MVP 2 阶段 2 运行说明

OpenAI Chat Completions 与 Responses 以相同格式透明转发。TOML 仅包含监听、PostgreSQL、默认上游超时、重载间隔和日志设置；API Key、模型规则、渠道组和渠道必须存储在 PostgreSQL 控制面。TOML 中的 `api_keys`、`channels` 和 `model_rules` 会被明确拒绝。

1. 启动 PostgreSQL：`docker compose up -d`。
2. 复制 `config.example.toml` 为已忽略的 `config.local.toml`，并填写进程级设置。
3. 写入有效的控制面记录；管理接口将在 MVP 2 阶段 4 提供，阶段 1 的本地开发可直接使用迁移后的数据库。
4. 运行 `cargo run -- config.local.toml`。服务启动时应用迁移、在可重复读事务中编译快照，并以配置的间隔重载。空控制面可启动，但会拒绝所有认证请求。

每个可用 API Key 需要对应格式的 `proxy` 权限；`GET /v1/models` 额外需要 `models.read`。客户端 Key 在编译后只以 SHA-256 摘要索引，不会保留在运行时路由快照中。每个启用规则在阶段 1 必须恰好展开为一个可用渠道，且渠道的 `available_models` 必须包含规则的 `upstream_model`。渠道与规则的 `api_format` 必须完全相同。

`base_url` 必须是没有凭据、查询参数或片段的 HTTP(S) URL；网关会在其路径后附加客户端的 `/v1/...` 路径和查询参数。上游认证支持 Bearer 或安全的自定义头；客户端的 `Authorization` 头不会透传。

已实现的端点：

- `GET /health`：返回 `204`。
- `GET /v1/models`：返回当前 API Key 可达格式中的模型规则；有权限但无可达模型时返回空列表。
- `POST /v1/chat/completions`：只查找 Chat Completions 规则。
- `POST /v1/responses`：只查找 Responses 规则。

无模型别名时，请求 JSON 保持原始字节直通；仅当 `upstream_model` 与 `client_model` 不同时，网关才会重写顶层 `model` 字段。响应通过 reqwest 字节流直接传给客户端，不缓冲 SSE。响应头、流空闲和建连超时分别受配置约束；客户端断开会丢弃上游流。

每次已选路请求都会写出一个 `proxy_request_completed` tracing 事件，并尽力异步持久化一条终态 `request_logs` 记录。`[request_logging] queue_capacity` 必须为正数；队列过载、关闭或数据库写入失败时，请求日志可能丢失，但不会阻塞或改变代理响应和流式转发。

收到 SIGTERM 或 Ctrl-C 后，网关停止接收新连接并在 `[server] shutdown_grace_period_seconds`（必须为正数）内等待进行中的请求。超过该期限仍未完成的连接会被强制关闭；其响应会被取消，并以 `cancelled` 终态处理。连接关闭后才开始请求日志 worker 的有界排空。
