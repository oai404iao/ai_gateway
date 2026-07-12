# 第一期运行说明

第一期提供 OpenAI Chat Completions 与 Responses 的同格式透明转发。控制面仍由启动时读取的 TOML 提供；数据库配置、热加载、权重、健康检查、变换、models.dev 同步和计费尚未接入请求链路。

1. 复制 `config.example.toml` 为已忽略的 `config.local.toml`。
2. 取消其中 `api_keys`、`channels` 和 `model_rules` 示例的注释，并替换全部示例密钥。
3. 运行 `cargo run -- config.local.toml`。

每个启用的 API Key 需要至少一个 API 格式和权限。`proxy` 允许代理请求；`models.read` 允许读取 `/v1/models`。客户端 Key 在配置编译后只以 SHA-256 摘要索引，不会保留在运行时路由快照中。

每个启用的模型规则只选择一个启用渠道，且规则与渠道的 `api_format` 必须完全相同。`base_url` 必须是没有凭据、查询参数或片段的 HTTP(S) URL；网关会在其路径后附加客户端的 `/v1/...` 路径和查询参数。`upstream_bearer_token`（若配置）会替换客户端的 `Authorization` 头。

已实现的端点：

- `GET /health`：返回 `204`。
- `GET /v1/models`：返回当前 API Key 可读取格式中的模型规则。
- `POST /v1/chat/completions`：只查找 Chat Completions 规则。
- `POST /v1/responses`：只查找 Responses 规则。

无模型别名时，请求 JSON 保持原始字节直通；仅当 `upstream_model` 与 `client_model` 不同时，网关才会重写顶层 `model` 字段。响应通过 reqwest 字节流直接传给客户端，不缓冲 SSE。响应头、流空闲和建连超时分别受配置约束；客户端断开会丢弃上游流。

每次已选路请求都会写出一个 `proxy_request_completed` tracing 事件，其中包含渠道 ID、API 格式、上游状态、TTFT、总延迟和结果。该日志目前是可观测性事件，不是数据库请求日志。
