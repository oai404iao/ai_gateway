# Codex OAuth 与订阅后端接入参考

> 类型：外部参考
> 最近核对：2026-08-11
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
> [Responses request type](https://github.com/openai/codex/blob/e363b08c9175ac1cbe5893615dd2cb9ddf95043b/codex-rs/codex-api/src/common.rs)、
> [session headers](https://github.com/openai/codex/blob/e363b08c9175ac1cbe5893615dd2cb9ddf95043b/codex-rs/codex-api/src/requests/headers.rs) 和
> [usage endpoint client](https://github.com/openai/codex/blob/e363b08c9175ac1cbe5893615dd2cb9ddf95043b/codex-rs/backend-client/src/client/rate_limit_resets.rs)。
> Account-ID 可选行为另外核对
> [`openai/codex@bb5054fe47abe73ecbbd454751066a28c89f4bb9`](https://github.com/openai/codex/tree/bb5054fe47abe73ecbbd454751066a28c89f4bb9)：
> [optional TokenData claims](https://github.com/openai/codex/blob/bb5054fe47abe73ecbbd454751066a28c89f4bb9/codex-rs/login/src/token_data.rs)、
> [optional Bearer account header](https://github.com/openai/codex/blob/bb5054fe47abe73ecbbd454751066a28c89f4bb9/codex-rs/model-provider/src/bearer_auth_provider.rs)
> 和
> [accountless Pro auth test](https://github.com/openai/codex/blob/bb5054fe47abe73ecbbd454751066a28c89f4bb9/codex-rs/login/src/auth/auth_tests.rs)。
> Reset-credit 行为另外核对
> [`openai/codex@2b5bdcf67547860f2e5c5a605009a70026796b2b`](https://github.com/openai/codex/tree/2b5bdcf67547860f2e5c5a605009a70026796b2b)：
> [backend reset client](https://github.com/openai/codex/blob/2b5bdcf67547860f2e5c5a605009a70026796b2b/codex-rs/backend-client/src/client/rate_limit_resets.rs)、
> [backend contract tests](https://github.com/openai/codex/blob/2b5bdcf67547860f2e5c5a605009a70026796b2b/codex-rs/backend-client/src/client/rate_limit_resets_tests.rs)
> 和
> [app-server account API](https://github.com/openai/codex/blob/2b5bdcf67547860f2e5c5a605009a70026796b2b/codex-rs/app-server/README.md)。
> Responses WebSocket 行为另外核对本地
> [`openai/codex@aa064463458adbef10400c74174107fc4b3550f0`](https://github.com/openai/codex/tree/aa064463458adbef10400c74174107fc4b3550f0)：
> [WebSocket endpoint](https://github.com/openai/codex/blob/aa064463458adbef10400c74174107fc4b3550f0/codex-rs/codex-api/src/endpoint/responses_websocket.rs)
> 和 [turn client](https://github.com/openai/codex/blob/aa064463458adbef10400c74174107fc4b3550f0/codex-rs/core/src/client.rs)。
> Images 行为核对同一提交的
> [Images endpoint](https://github.com/openai/codex/blob/aa064463458adbef10400c74174107fc4b3550f0/codex-rs/codex-api/src/endpoint/images.rs)、
> [Images wire types](https://github.com/openai/codex/blob/aa064463458adbef10400c74174107fc4b3550f0/codex-rs/codex-api/src/images.rs)、
> [image backend](https://github.com/openai/codex/blob/aa064463458adbef10400c74174107fc4b3550f0/codex-rs/ext/image-generation/src/backend.rs)
> 和 [image tool](https://github.com/openai/codex/blob/aa064463458adbef10400c74174107fc4b3550f0/codex-rs/ext/image-generation/src/tool.rs)。
> Standalone web search 行为另外核对
> [`openai/codex@5af85998c24fb3353ddd8164c3ed472057b03cb3`](https://github.com/openai/codex/tree/5af85998c24fb3353ddd8164c3ed472057b03cb3)：
> [Search endpoint](https://github.com/openai/codex/blob/5af85998c24fb3353ddd8164c3ed472057b03cb3/codex-rs/codex-api/src/endpoint/search.rs)、
> [Search wire types](https://github.com/openai/codex/blob/5af85998c24fb3353ddd8164c3ed472057b03cb3/codex-rs/codex-api/src/search.rs)
> [Search tool Header](https://github.com/openai/codex/blob/5af85998c24fb3353ddd8164c3ed472057b03cb3/codex-rs/ext/web-search/src/tool.rs)
> 和
> [provider capability](https://github.com/openai/codex/blob/5af85998c24fb3353ddd8164c3ed472057b03cb3/codex-rs/model-provider-info/src/lib.rs)。
> Codex 请求指纹另外核对本地
> [`openai/codex@7a0e974e08c798d1e8d59d407aeb6e24db1313af`](https://github.com/openai/codex/tree/7a0e974e08c798d1e8d59d407aeb6e24db1313af)：
> [Responses metadata](https://github.com/openai/codex/blob/7a0e974e08c798d1e8d59d407aeb6e24db1313af/codex-rs/core/src/responses_metadata.rs)、
> [Responses wire types](https://github.com/openai/codex/blob/7a0e974e08c798d1e8d59d407aeb6e24db1313af/codex-rs/codex-api/src/common.rs) 和
> [default client fingerprint](https://github.com/openai/codex/blob/7a0e974e08c798d1e8d59d407aeb6e24db1313af/codex-rs/login/src/auth/default_client.rs)。
> HTTP content-coding 基线另外核对当前
> [`openai/codex@5af85998c23ddb9cc21c43ef41db44712b481611`](https://github.com/openai/codex/tree/5af85998c23ddb9cc21c43ef41db44712b481611)：
> [default client](https://github.com/openai/codex/blob/5af85998c23ddb9cc21c43ef41db44712b481611/codex-rs/login/src/auth/default_client.rs)、
> [HTTP client features](https://github.com/openai/codex/blob/5af85998c23ddb9cc21c43ef41db44712b481611/codex-rs/http-client/Cargo.toml)
> 和
> [response transport](https://github.com/openai/codex/blob/5af85998c23ddb9cc21c43ef41db44712b481611/codex-rs/http-client/src/transport.rs)。
> 官方客户端当前不主动发送 `Accept-Encoding`，也没有启用 reqwest 的 HTTP 响应解压 features；
> 下述双向独立协商是 ai-gateway 的代理增强能力，不是 Codex 客户端保证。

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
请求验证。`chatgpt_account_id` 在存在时表示所选 workspace，`chatgpt_user_id` 表示用户身份。
该 account claim 是可选的，并非所有 ChatGPT 认证状态都有 workspace ID；官方当前测试也覆盖
没有 account ID 的普通 Pro ChatGPT auth，因此不能按 plan 类型推断该字段必然存在。Business
plan 的多个成员可以共享 account ID。管理员导入凭证时，如果显式 `account_id`/`user_id` 与
Token claim 不同，网关拒绝导入。

## Codex 接口

核对版本的 ChatGPT Codex base URL 是
`https://chatgpt.com/backend-api/codex`：

| 操作 | 方法与路径 |
| --- | --- |
| Responses | `POST /backend-api/codex/responses` |
| Responses WebSocket | `GET wss://chatgpt.com/backend-api/codex/responses` Upgrade |
| Standalone web search | `POST /backend-api/codex/alpha/search` |
| Images generation | `POST /backend-api/codex/images/generations` |
| Images edit | `POST /backend-api/codex/images/edits` |
| Models | `GET /backend-api/codex/models?client_version=...` |
| Quota / rate limit | `GET /backend-api/wham/usage` |
| Consume reset credit | `POST /backend-api/wham/rate-limit-reset-credits/consume` |

认证请求使用：

- `Authorization: Bearer <access_token>`；
- 有 account ID 时发送 `ChatGPT-Account-ID: <account_id>`；没有时省略；
- FedRAMP 账户附加 `X-OpenAI-Fedramp: true`；
- `originator: codex_cli_rs`；
- 以 `codex_cli_rs/0.146.0` 为基础值的 `User-Agent` 和独立的版本标识；
- Responses 请求的 `session-id`、`thread-id` 与 `x-client-request-id`。

Standalone web search 额外保留 `x-codex-turn-metadata`，但发送前会把客户端
`originator` 和 `User-Agent` 统一覆盖为 Gateway 的 `codex_cli_rs` Connector 身份。请求顶层
字段为 `id`、`model` 以及可选 `reasoning`、`input`、`commands`、`settings`、
`max_output_tokens`。响应为非流式 JSON，`output` 是最终文本，`encrypted_output` 和
`results` 为可选 opaque 数据。

Codex 的 image tool 当前对 generation 使用 `gpt-image-2`，请求字段包含
`prompt`、`background`、`model`、可选 `n`、`quality` 与 `size`。Images 请求额外发送
`x-codex-image-turn-id`，但不复用 Responses 的 session/thread identity。Images 响应是非流式
JSON，`data[].b64_json` 可能很大，并可在顶层包含 usage。Codex edit 使用 JSON
`images[].image_url` data URL，而不是公开 OpenAI API 常见的 multipart 形状；该差异只属于
provider adapter。

Codex Responses HTTP 客户端使用 Responses wire format。当前直连接口按流式方式工作，
请求强制 `stream=true`、`store=false`，且不会用 HTTP
`previous_response_id` 恢复增量连接状态。核对版本的 `ResponsesApiRequest` 不声明
`max_output_tokens`。

Models 响应是带 `models` 数组的 envelope；网关只保留非空且未显式声明
`supported_in_api=false` 的 `slug`。Quota 响应使用 `rate_limit.allowed`、
`limit_reached`、primary/secondary window 的 `used_percent`、
`limit_window_seconds` 和 Unix `reset_at`，并可附带
`rate_limit_reset_credits.available_count`。

官方 reset-credit 请求 JSON 为
`{"redeem_request_id":"<idempotency key>"}`，也支持可选 `credit_id`。Gateway 当前让 OpenAI
选择可用 credit，不发送 `credit_id`。响应 `code` 为 `reset`、`nothing_to_reset`、
`no_credit` 或 `already_redeemed`，并返回 `windows_reset`。Codex app-server 要求 ChatGPT
认证，并建议 reset 后重新读取 rate limits。

Models 查询参数 `client_version` 和请求 Header `version` 固定报告当前核对的 Codex
客户端版本 `0.146.0`。该版本独立于 `ai-gateway` 自身版本，因为 Codex 后端会根据客户端
版本过滤模型；误用较小的网关版本可能得到成功但为空的 `models` 数组。

## ai-gateway 兼容行为

### 协议与 Connector 分离

客户端协议仍是 `api_format = open_ai_responses` 或 `open_ai_images`，特殊上游方式由
`connector_kind = codex_oauth` 表示。网关没有新增 provider-specific 客户端 API 格式，也不会在
Chat Completions、Responses 与 Images 之间转换。

每个 Codex OAuth 凭证属于一个共享 Connector pool，并对应独立的 Responses 与 Images
provider-managed `channels` 记录。因此普通优先级、权重、API Key 授权、独立 outbound proxy、
请求日志和 Responses Session affinity 继续使用统一路由系统。普通 Channel CRUD 和批量编辑不能
修改这些 managed channels。

### 请求准备

客户端可以调用 `POST /v1/responses`、`POST /v1/alpha/search` 或带 WebSocket Upgrade 的
`GET /v1/responses`。选中 Codex managed channel 后，HTTP 路径：

1. 要求请求本身为 SSE streaming；
2. 拒绝非空 `previous_response_id`；
3. 在普通 Transform 后应用 Codex Responses HTTP body/Header 白名单，删除
   `max_output_tokens`、纯遥测和契约列出的空值/no-op；其他 provider-unsupported 非默认值报错；
4. 把 `client_metadata["x-codex-installation-id"]` 和 turn metadata 中的
   `installation_id` 替换为按逻辑凭证稳定的 opaque UUID；`workspaces` 强制替换为系统设置中的
   单一合成 Git 工作区；
5. 缺失时创建 `client_metadata` 并补齐 session/thread/turn/window、turn metadata 与
   `prompt_cache_key`；不推测 request kind、sandbox、beta、subagent、attestation、turn-state
   或 residency，其他 metadata 与 W3C trace/baggage 保留；
6. 强制写入 `stream=true`、`store=false`；
7. 将目标改为 `/backend-api/codex/responses`；
8. `Accept-Encoding` 由通用代理层独立设置为 `gzip, deflate, br, zstd`，上游响应在终态 SSE
   与 usage 解析前流式解码；
9. 最后注入当前凭证的 Bearer、可选 account、FedRAMP 和 Codex 会话 Header；
10. 成功响应按 SSE 分类，并将客户端可见 `Content-Type` 规范化为
   `text/event-stream`，即使 Codex 上游缺少或改写了该 Header；
11. 逐块转发解码后的上游 SSE，不在 Connector 中缓冲整条响应；下游 SSE 保持 identity。

WebSocket 路径：

1. 要求消息 `type = "response.create"` 并取得顶层 `model`；
2. 在普通 Transform 后应用独立的 Codex Responses WebSocket body/Header 白名单，删除
   `max_output_tokens` 和契约列出的纯遥测/no-op，未知 body 字段报错；
3. 强制 `stream=true`、`store=false`，但保留
   `previous_response_id`、`generate` 和 `client_metadata`；缺失的 `client_metadata` 与
   `prompt_cache_key` 会补齐，installation/workspace 指纹按 HTTP 相同规则归一化；
4. 把目标改为 managed channel base URL 下的 `/responses`，再将
   `http`/`https` 转成 `ws`/`wss`；
5. 注入 `OpenAI-Beta: responses_websockets=2026-02-06`、Bearer/可选 account、
   FedRAMP、session/thread、User-Agent、`originator` 和版本 Header；
6. 不发送 HTTP SSE 专用的 `Accept`、`Accept-Encoding` 和 `Content-Type`；
7. 顺序转发事件，并把成功、无残留的连接放回 Session 隔离池，使后续请求可以继续使用
   connection-local `previous_response_id`。

Standalone web search 路径：

1. 使用现有 `open_ai_responses` 模型规则和 API Key 权限，但只选择
   `supports_standalone_web_search = true` 的 Responses channel；
2. 固定为非流式 JSON，在模型别名后应用独立 Search body/Header 白名单；
3. 不应用 Request JSON Transform；Header 和响应 Header Transform 仍有效；
4. 将目标改为 managed channel base URL 下的 `/alpha/search`；
5. 保留合法的 `x-codex-turn-metadata` 和可选 `x-client-request-id`；缺失或无效的 turn metadata
   会安全合成，installation/workspace 指纹按同一凭证/系统设置规则归一化；客户端
   `originator` 和 `User-Agent` 固定覆盖为 Gateway 的 `codex_cli_rs` Connector 身份，再注入
   Bearer/可选 account/FedRAMP 和版本，删除 Responses Session Header；
6. `results` DTO 透明转发；没有 usage 时不估算 token 或费用。

客户端也可以调用非流式 JSON `POST /v1/images/generations`。选中 Codex Images projection 后：

1. 在模型别名和受限变换后应用 Codex Images generation body 白名单，只保留 wire type 字段；
   `output_format=png`、`moderation=auto` 等等价值删除；
2. 将目标改为 `/backend-api/codex/images/generations`；
3. 注入共享 credential 的 Bearer/可选 account/FedRAMP，以及 `originator`、版本、User-Agent 和
   Gateway 生成的 `x-codex-image-turn-id`；
4. 删除客户端 `session-id`、`thread-id` 与 `x-client-request-id`；
5. 按普通 JSON 逐块转发响应，并增量提取顶层 usage，不缓冲完整 base64 图片。

客户端还可以调用 multipart `POST /v1/images/edits`。选中相同 Images projection 后：

1. 网关先在专用限制内捕获 replayable multipart，并完成模型路由；
2. 最多五个 `image`/`image[]` part 被增量 base64 编码为
   `images[].image_url = data:<mime>;base64,...`；
3. 客户端 multipart 与 Codex adapter 分别应用入口/出口白名单，只保留
   `prompt`、`background`、`model`、`n`、`quality` 与 `size`；`moderation=auto` 在入口层
   删除，`output_format=png` 在 Codex 层删除，mask 和无法等价表达的值在联系上游前拒绝；
4. 将目标改为 `/backend-api/codex/images/edits`；
5. 注入与 generation 相同的 credential 和 image-turn Header，并按非流式 JSON 处理响应。

Responses 客户端提供的合法 `session-id` / `thread-id` 会被保留。缺少时，HTTP 请求如果匹配已配置的
Session affinity 规则，网关从该规则的不可逆 session hash 派生稳定、opaque 的 UUID；
没有匹配 affinity 的 HTTP 请求使用本次请求 UUID。WebSocket Session 从下游握手身份派生稳定
seed，使顺序请求和池化重连保持一致的身份。缺失的
`x-client-request-id` 使用最终 `thread-id`。

Standalone web search 的 body `id` 是同一 Codex Session ID。推荐 affinity 规则同时使用
Responses `/prompt_cache_key` 和 Search `/id`，使两种 operation 固定到同一个订阅凭证。

Codex 成功响应按 Connector 契约视为 SSE，而不只依赖上游
`Content-Type`。网关在转发 `response.completed` / `response.failed` 的同时完成
usage 与请求终态记录；客户端随后立即停止读取时，不会把已经完整结束的请求覆盖成
`client_cancelled`。

为提高 ChatGPT Codex 后端兼容性，网关使用 Codex 默认的
`originator: codex_cli_rs`，并发送稳定的基础 User-Agent
`codex_cli_rs/0.146.0`。原生 Codex 还会在该基础值后附加操作系统和终端信息；网关不伪造这些
不属于服务进程的交互式客户端元数据。Codex `client_version`/`version` 同样报告独立维护的
兼容版本。OAuth 公共 client ID、scope、redirect URI 和后端路径与核对版本保持一致。

原生 Codex 还会通过 flat `client_metadata` 和 JSON 字符串形式的
`x-codex-turn-metadata` 上报持久 installation ID，以及 workspace 根路径、Git remote、HEAD
commit 和 dirty 状态。`ai-gateway` 将 installation ID 按稳定 credential ID 派生 opaque UUID，
同一逻辑凭证跨 Responses HTTP/WebSocket/Search 保持一致；`workspaces` 始终替换为
`forwarding_policy.codex` 配置的单一合成工作区。默认 path 为 `/workspace`，默认 origin 为
`https://github.com/oai404iao/ai_gateway`。缺失的 Responses 身份 metadata 与 Search turn
metadata 会安全补齐；无法解析的 turn metadata 会被替换而不是 opaque 转发。已有
Session/thread/turn/window、request kind、compaction、sandbox、子 Agent、App Server extra
metadata 和 W3C trace/baggage 保留。

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
在同一 Connector pool 的任一格式 Channel Group 再次 OAuth 或导入相同 workspace/member 身份，
或相同 accountless personal user ID，会原位更新 Token 和两个 managed channels；同一 Business
workspace 的不同 `user_id` 会创建独立凭证，不会互相覆盖。

Quota 状态由两个 projection 共享，并映射为：

- `active`：可接收新的 Responses Session、standalone web search 或 Images generation/edit；
- `draining`：达到管理员阈值，只允许已命中 affinity 的既有 Responses Session；Images
  projection 在发送前被排除；
- `unavailable`：quota 不允许、额度耗尽或永久 refresh 失败；
- `disabled`：管理员关闭。

Gateway 额外保存主/次窗口周期历史。计划边界后的换窗记为自然重置；通过 Console 调用上述
reset-credit 接口并匹配到的换窗记为手动重置；未匹配手动事件且早于旧计划边界的换窗记为
`openai_official`。最后一类用于记录 OpenAI 故障补偿等 provider-side 重置，但 usage 响应本身
没有携带原因字段，因此该标签是 Gateway 根据提前换窗作出的推断。Gateway 不会自动消费
reset credit，也不会为了检测官方补偿而调用 consume 接口。

首次派发前发现凭证不可用时，非 affinity hit 请求可以排除该 channel 后重新选路；Images 不使用
affinity，因此只能在发送前排除不可用 projection。已经粘到该
凭证的 HTTP Session，或已经由下游 WebSocket/空闲池固定到该 channel 的 WebSocket Session，
持续 fail closed，失败本身不会删除 affinity 或改绑其他账户。HTTP 请求一旦发出，或 WebSocket
`response.create` 一旦发送，不做跨凭证自动重试。HTTP/握手 `401` 和带 `status = 401` 的
WebSocket 错误会触发按 refresh generation 去重的后台 token refresh。

## 差异与限制

- Codex Connector 支持 Responses HTTP SSE、Responses WebSocket、非流式 standalone web
  search、非流式 Images generation 与 multipart Images edit；
  Responses HTTP 仍拒绝非空 `previous_response_id`，WebSocket 保留该字段并依赖同一条上游连接
  的增量状态；两种传输都通过独立 allowlist 明确分类已知字段，未知 body 字段不再透明转发。
- edit adapter 最多接受五张图片，不接受 mask 或无法等价表达的 Codex wire type 外字段；
  显式默认值/no-op 按契约删除。这些 provider-specific 限制不改变普通 OpenAI-compatible edit
  的最多 16 张图片边界。
- Connector 不实现 Chat Completions、Responses 与 Images 之间的转换。
- Console 删除会清除数据库中的 OAuth Token 并保留两个非敏感 managed-channel tombstone，但
  不会调用 OpenAI token revocation endpoint；外部撤销仍需在账户侧完成。
- Models catalog 在首次连接、重新授权或 Token 导入时验证并写入；当前不单独周期轮询 models。
- Quota threshold 是路由保护，不是计费或账户侧硬额度；已有 affinity Session 在
  `draining` 时仍可继续使用。
- 多进程部署的凭证快照、refresh mutex 和 Session affinity 仍是进程本地状态；数据库更新由
  各实例周期重载最终收敛。
- ChatGPT Codex 后端不是公开稳定 API。出现 OAuth 参数、JWT claim、models schema、quota
  schema、Header 或路径变化时，应先更新本参考和本地 deterministic tests，再修改 Connector。

完整字段和 Header 动作的机器可读来源是
[`request-allowlists.json`](request-allowlists.json)，维护规则见
[`请求字段与 Header 白名单`](request-allowlists.md)。

## 维护检查项

每次升级或重新核对 Codex 时至少检查：

1. OAuth client ID、scope、redirect URI、PKCE 参数和 token request encoding；
2. refresh error code 与 token rotation 语义；
3. ChatGPT Codex base URL、Responses/Images/models/quota 路径；
4. Bearer/可选 account/FedRAMP/session 与 `x-codex-image-turn-id` Header；
5. HTTP 与 WebSocket Responses 的 `stream`、`store`、`max_output_tokens`、
   `previous_response_id`、`generate` 和 `client_metadata` 边界；
6. models 与 quota JSON shape；
7. reset-credit available count、consume 路径、请求幂等键、结果 code 与 `windows_reset`；
8. Images generation/edit JSON shape、当前 image model 和 usage 响应；
9. User-Agent/originator 变化是否会影响授权或后端兼容性。
10. `client_metadata` 与 `x-codex-turn-metadata` 中 installation/workspace/identity 字段的位置、
    类型和保留必要性，以及缺失字段补全和系统合成 workspace 是否仍与上游兼容。
