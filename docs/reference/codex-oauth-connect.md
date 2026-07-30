# Codex OAuth 与订阅后端接入参考

> 类型：外部参考
> 最近核对：2026-07-30
> 权威来源：
> [`openai/codex` 0.146.0 release](https://github.com/openai/codex/releases/tag/rust-v0.146.0)、
> [OAuth server](https://github.com/openai/codex/blob/e363b08c9175ac1cbe5893615dd2cb9ddf95043b/codex-rs/login/src/server.rs)、
> [ID Token claim parser](https://github.com/openai/codex/blob/e363b08c9175ac1cbe5893615dd2cb9ddf95043b/codex-rs/login/src/token_data.rs)、
> [token refresh manager](https://github.com/openai/codex/blob/e363b08c9175ac1cbe5893615dd2cb9ddf95043b/codex-rs/login/src/auth/manager.rs)、
> [default HTTP client](https://github.com/openai/codex/blob/e363b08c9175ac1cbe5893615dd2cb9ddf95043b/codex-rs/login/src/auth/default_client.rs)、
> [ChatGPT Codex provider](https://github.com/openai/codex/blob/e363b08c9175ac1cbe5893615dd2cb9ddf95043b/codex-rs/model-provider-info/src/lib.rs)、
> [Bearer auth provider](https://github.com/openai/codex/blob/e363b08c9175ac1cbe5893615dd2cb9ddf95043b/codex-rs/model-provider/src/bearer_auth_provider.rs)、
> [Models endpoint](https://github.com/openai/codex/blob/e363b08c9175ac1cbe5893615dd2cb9ddf95043b/codex-rs/codex-api/src/endpoint/models.rs)、
> [Responses endpoint](https://github.com/openai/codex/blob/e363b08c9175ac1cbe5893615dd2cb9ddf95043b/codex-rs/codex-api/src/endpoint/responses.rs)、
> [session headers](https://github.com/openai/codex/blob/e363b08c9175ac1cbe5893615dd2cb9ddf95043b/codex-rs/codex-api/src/requests/headers.rs) 和
> [usage endpoint client](https://github.com/openai/codex/blob/e363b08c9175ac1cbe5893615dd2cb9ddf95043b/codex-rs/backend-client/src/client/rate_limit_resets.rs)。
> Responses WebSocket 行为另外核对本地
> [`openai/codex@aa064463458adbef10400c74174107fc4b3550f0`](https://github.com/openai/codex/tree/aa064463458adbef10400c74174107fc4b3550f0)：
> [WebSocket endpoint](https://github.com/openai/codex/blob/aa064463458adbef10400c74174107fc4b3550f0/codex-rs/codex-api/src/endpoint/responses_websocket.rs)
> 和 [turn client](https://github.com/openai/codex/blob/aa064463458adbef10400c74174107fc4b3550f0/codex-rs/core/src/client.rs)。

本文记录 `ai-gateway` 的 Codex OAuth Connector 所依赖的外部行为。这里的
`chatgpt.com/backend-api/*` 是 Codex 客户端使用的 ChatGPT 后端接口，不是公开的
OpenAI Platform API 稳定性承诺；外部接口、授权条件和账户政策可能随时变化。

## OAuth 与 Token

核对版本使用 OAuth 2.0 Authorization Code + PKCE：

| 项目 | 外部值 |
| --- | --- |
| Authorization endpoint | `https://auth.openai.com/oauth/authorize` |
| Token endpoint | `https://auth.openai.com/oauth/token` |
| Public client ID | `app_EMoamEEZ73f0CkXaXp7hrann` |
| Redirect URI | `http://localhost:1455/auth/callback` |
| Scope | `openid profile email offline_access api.connectors.read api.connectors.invoke` |
| PKCE | `S256` |

Authorization URL 还包含 `id_token_add_organizations=true`、
`codex_cli_simplified_flow=true`、随机 `state` 和 `originator`。Authorization code
交换使用 `application/x-www-form-urlencoded`；refresh 使用 JSON
`{client_id, grant_type:"refresh_token", refresh_token}`。refresh 响应中的
`id_token`、`access_token` 和 `refresh_token` 都可能独立轮换。

Codex 将 `refresh_token_expired`、`refresh_token_reused` 和
`refresh_token_invalidated` 视为需要重新登录的永久失败。网关沿用这个分类，不会在
refresh token 已失效时继续把凭证投入新请求。

ID token / access token 的 JWT payload 中，本接入使用：

- `email` 或 `https://api.openai.com/profile.email`；
- `https://api.openai.com/auth.chatgpt_account_id`；
- `https://api.openai.com/auth.chatgpt_user_id`，缺失时兼容同一 namespace 下的 `user_id`；
- `https://api.openai.com/auth.chatgpt_plan_type`；
- `https://api.openai.com/auth.chatgpt_account_is_fedramp`；
- access token 顶层 `exp`。

JWT payload 解析只用于提取元数据和过期时间；真正的凭证有效性由后续 Codex models
请求验证。`chatgpt_account_id` 表示所选 workspace，`chatgpt_user_id` 表示该 workspace 中的
成员；Business plan 的多个成员可以共享 account ID。管理员导入凭证时，如果显式
`account_id`/`user_id` 与 token claim 不同，网关拒绝导入。

## Codex 接口

核对版本的 ChatGPT Codex base URL 是
`https://chatgpt.com/backend-api/codex`：

| 操作 | 方法与路径 |
| --- | --- |
| Responses | `POST /backend-api/codex/responses` |
| Responses WebSocket | `GET wss://chatgpt.com/backend-api/codex/responses` Upgrade |
| Models | `GET /backend-api/codex/models?client_version=...` |
| Quota / rate limit | `GET /backend-api/wham/usage` |

认证请求使用：

- `Authorization: Bearer <access_token>`；
- `ChatGPT-Account-ID: <account_id>`；
- FedRAMP 账户附加 `X-OpenAI-Fedramp: true`；
- `originator: codex_cli_rs`；
- 以 `codex_cli_rs/0.146.0` 为基础值的 `User-Agent` 和独立的版本标识；
- Responses 请求的 `session-id`、`thread-id` 与 `x-client-request-id`。

Codex Responses HTTP 客户端使用 Responses wire format。当前直连接口按流式方式工作，
请求强制 `stream=true`、`store=false`，且不会用 HTTP
`previous_response_id` 恢复增量连接状态。

Models 响应是带 `models` 数组的 envelope；网关只保留非空且未显式声明
`supported_in_api=false` 的 `slug`。Quota 响应使用 `rate_limit.allowed`、
`limit_reached`、primary/secondary window 的 `used_percent`、
`limit_window_seconds` 和 Unix `reset_at`。

Models 查询参数 `client_version` 和请求 Header `version` 固定报告当前核对的 Codex
客户端版本 `0.146.0`。该版本独立于 `ai-gateway` 自身版本，因为 Codex 后端会根据客户端
版本过滤模型；误用较小的网关版本可能得到成功但为空的 `models` 数组。

## ai-gateway 兼容行为

### 协议与 Connector 分离

客户端协议仍是 `api_format = open_ai_responses`，特殊上游方式由
`connector_kind = codex_oauth` 表示。网关没有新增第三种客户端 API 格式，也不会在
Chat Completions 与 Responses 之间转换。

每个 Codex OAuth 凭证对应一个 provider-managed `channels` 记录；Channel Group 是凭证池。
因此普通优先级、权重、API Key 授权、独立 outbound proxy、请求日志和 Session affinity
继续使用统一路由系统。普通 Channel CRUD 和批量编辑不能修改这些 managed channels。

### 请求准备

客户端可以调用 `POST /v1/responses` 或带 WebSocket Upgrade 的
`GET /v1/responses`。选中 Codex managed channel 后，HTTP 路径：

1. 要求请求本身为 SSE streaming；
2. 拒绝非空 `previous_response_id`；
3. 强制写入 `stream=true`、`store=false`；
4. 将目标改为 `/backend-api/codex/responses`；
5. 强制上游 `Accept-Encoding: identity`，使网关能直接观察终态 SSE 与 usage；
6. 最后注入当前凭证的 Bearer、account、FedRAMP 和 Codex 会话 Header；
7. 逐块转发上游 SSE，不在 Connector 中缓冲整条响应。

WebSocket 路径：

1. 要求消息 `type = "response.create"` 并取得顶层 `model`；
2. 强制 `stream=true`、`store=false`，但保留
   `previous_response_id`、`generate` 和 `client_metadata`；
3. 把目标改为 managed channel base URL 下的 `/responses`，再将
   `http`/`https` 转成 `ws`/`wss`；
4. 注入 `OpenAI-Beta: responses_websockets=2026-02-06`、Bearer/account、
   FedRAMP、session/thread、User-Agent、`originator` 和版本 Header；
5. 不发送 HTTP SSE 专用的 `Accept`、`Accept-Encoding` 和 `Content-Type`；
6. 顺序转发事件，并把成功、无残留的连接放回 Session 隔离池，使后续请求可以继续使用
   connection-local `previous_response_id`。

客户端提供的合法 `session-id` / `thread-id` 会被保留。缺少时，HTTP 请求如果匹配已配置的
Session affinity 规则，网关从该规则的不可逆 session hash 派生稳定、opaque 的 UUID；
没有匹配 affinity 的 HTTP 请求使用本次请求 UUID。WebSocket Session 从下游握手身份派生稳定
seed，使顺序请求和池化重连保持一致的身份。缺失的
`x-client-request-id` 使用最终 `thread-id`。

Codex 成功响应按 Connector 契约视为 SSE，而不只依赖上游
`Content-Type`。网关在转发 `response.completed` / `response.failed` 的同时完成
usage 与请求终态记录；客户端随后立即停止读取时，不会把已经完整结束的请求覆盖成
`client_cancelled`。

为提高 ChatGPT Codex 后端兼容性，网关使用 Codex 默认的
`originator: codex_cli_rs`，并发送稳定的基础 User-Agent
`codex_cli_rs/0.146.0`。原生 Codex 还会在该基础值后附加操作系统和终端信息；网关不伪造这些
不属于服务进程的交互式客户端元数据。Codex `client_version`/`version` 同样报告独立维护的
兼容版本。OAuth 公共 client ID、scope、redirect URI 和后端路径与核对版本保持一致。

### 凭证运行时与维护

OAuth/access/refresh token 保存在 PostgreSQL；Console API 永不把 token 返回给浏览器，
Debug 和 audit 表示也会脱敏。当前 schema 与普通 upstream API key 一样，依赖受保护的
数据库和备份边界，未额外实施列级静态加密。

数据面通过独立 `ArcSwap` 凭证快照读取 access token，不为每次请求查询数据库，也不因 quota
轮询重新编译整份控制面快照。进程内 worker 以有界并发执行 token/identity refresh、
quota polling 和过期 OAuth flow 清理；同一凭证的 refresh 同时使用进程内互斥锁和
PostgreSQL row lock，并在锁内再次核对 `refresh_generation`，避免同实例或多实例并发重复使用
rotating refresh token。

永久 refresh 失败会留下持久的重新授权标记，后续 quota 成功或普通设置编辑不会自动恢复凭证。
在同一 Channel Group 再次 OAuth 或导入相同 workspace/member 身份会原位更新 Token 和 managed
channel；同一 Business workspace 的不同 `user_id` 会创建独立凭证，不会互相覆盖。

Quota 状态映射为：

- `active`：可接收新 Session；
- `draining`：达到管理员阈值，只允许已命中 affinity 的既有 Session；
- `unavailable`：quota 不允许、额度耗尽或永久 refresh 失败；
- `disabled`：管理员关闭。

首次派发前发现凭证不可用时，非 affinity hit 请求可以排除该 channel 后重新选路；已经粘到该
凭证的 HTTP Session，或已经由下游 WebSocket/空闲池固定到该 channel 的 WebSocket Session，
持续 fail closed，失败本身不会删除 affinity 或改绑其他账户。HTTP 请求一旦发出，或 WebSocket
`response.create` 一旦发送，不做跨凭证自动重试。HTTP/握手 `401` 和带 `status = 401` 的
WebSocket 错误会触发按 refresh generation 去重的后台 token refresh。

## 差异与限制

- Codex Connector 支持 Responses HTTP SSE 与 Responses WebSocket，但不支持 HTTP
  non-streaming。HTTP 仍拒绝非空 `previous_response_id`；WebSocket 保留该字段并依赖同一条
  上游连接的增量状态。
- Connector 不实现 Chat Completions↔Responses 转换。
- Console 删除会清除数据库中的 OAuth Token 并保留非敏感 managed-channel tombstone，但不会调用
  OpenAI token revocation endpoint；外部撤销仍需在账户侧完成。
- Models catalog 在首次连接、重新授权或 Token 导入时验证并写入；当前不单独周期轮询 models。
- Quota threshold 是路由保护，不是计费或账户侧硬额度；已有 affinity Session 在
  `draining` 时仍可继续使用。
- 多进程部署的凭证快照、refresh mutex 和 Session affinity 仍是进程本地状态；数据库更新由
  各实例周期重载最终收敛。
- ChatGPT Codex 后端不是公开稳定 API。出现 OAuth 参数、JWT claim、models schema、quota
  schema、Header 或路径变化时，应先更新本参考和本地 deterministic tests，再修改 Connector。

## 维护检查项

每次升级或重新核对 Codex 时至少检查：

1. OAuth client ID、scope、redirect URI、PKCE 参数和 token request encoding；
2. refresh error code 与 token rotation 语义；
3. ChatGPT Codex base URL、Responses/models/quota 路径；
4. Bearer/account/FedRAMP/session Header；
5. HTTP 与 WebSocket Responses 的 `stream`、`store`、`previous_response_id`、
   `generate` 和 `client_metadata` 边界；
6. models 与 quota JSON shape；
7. User-Agent/originator 变化是否会影响授权或后端兼容性。
