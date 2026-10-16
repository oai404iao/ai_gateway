# Codex OAuth 与订阅后端接入参考

> 类型：外部参考与插件兼容边界。
>
> 最近核对：2026-08-19。各补充源码版本见文末；本次整理未重新核验外部服务。
>
> 权威来源：[openai/codex 0.146.0](https://github.com/openai/codex/releases/tag/rust-v0.146.0)及文末各固定提交。

Codex 插件连接 ChatGPT 订阅后端，不是给普通 OpenAI API Key 换一个 base URL。
`chatgpt.com/backend-api/*` 是核对版本 Codex 客户端使用的后端接口，没有公开 Platform API
的稳定性承诺。授权条件、接口和账户政策可变化。

## 先选择任务

| 任务 | 入口 |
| --- | --- |
| 安装/启用 Codex 插件 | [插件安装](../user/connector-plugins.md) |
| OAuth、Token 导入、凭证绑定 | [上游凭证管理](../user/upstream-credentials.md) |
| 迁移 CLIProxyAPI/Sub2API 文件 | [导入格式](codex-credential-portability.md) |
| 配置公共调用 | [兼容性总览](openai-compatibility.md)、[Responses](responses.md)、[Images](openai-images.md) |
| 调查 WS 增量与 fallback | [固定版本源码研究](codex-responses-websocket.md) |

管理员先安装并启用插件，再导入/授权凭证，显式创建接入、绑定凭证的逻辑渠道及操作能力，
最后配置模型操作路由和 Key/Policy 的固定逻辑渠道授权。**导入凭证不自动创建这些资源。**

最小公共请求形状见各协议页；Codex HTTP Responses 必须请求 SSE：

```http
POST /v1/responses
Authorization: Bearer <gateway-api-key>
Content-Type: application/json

{"model":"your-codex-client-model","input":"Reply with OK.","stream":true}
```

这不是直接向 ChatGPT 发送 access token 的示例。上游 OAuth/身份由插件和宿主准备；实际调用会
消耗订阅额度，并按网关规则计费。

## OAuth 与 Token

### 核对版本的外部协议

核对的 Codex 使用 Authorization Code + PKCE：

| 项目 | 外部值 |
| --- | --- |
| Authorization endpoint | `https://auth.openai.com/oauth/authorize` |
| Token endpoint | `https://auth.openai.com/oauth/token` |
| Public client ID | `app_EMoamEEZ73f0CkXaXp7hrann` |
| Redirect URI | `http://localhost:1455/auth/callback` |
| Scope | `openid profile email offline_access api.connectors.read api.connectors.invoke` |
| PKCE | `S256` |

Authorization URL 还带 `id_token_add_organizations=true`、
`codex_cli_simplified_flow=true`、随机 state 和 originator。code 交换为 form-urlencoded，
refresh 为 JSON `{client_id, grant_type:"refresh_token", refresh_token}`；
ID/access/refresh token 可分别轮换。核对版本把 `refresh_token_expired`、
`refresh_token_reused`、`refresh_token_invalidated` 归为需重新登录的永久失败。

这些端点、参数和错误分类由插件协议实现提供，不是宿主另维护的一套 Codex 常量。

### 身份与有效性

核对的 JWT 元数据包括 email（顶层或 profile namespace）、auth namespace 中的
`chatgpt_account_id`、`chatgpt_user_id`（兼容 `user_id`）、
`chatgpt_plan_type`、`chatgpt_account_is_fedramp`，以及 access token 顶层 exp。

解析 payload 只是提取身份和过期时间，不等于本地验证 Token 有效；服务端仍请求 Codex models。
account ID 是可选 workspace 身份，不按 Free/Pro/Business 推断其必填性；
同 Business workspace 的多个成员可共享 account ID。导入显式身份与 Token claim 冲突时拒绝，
不能补造 workspace ID。账号匹配和去重规则见[凭证格式](codex-credential-portability.md)。

## Codex 接口

### 核对版本的路径

Codex base URL 为 `https://chatgpt.com/backend-api/codex`：

| 操作 | 方法与路径 |
| --- | --- |
| Responses HTTP / WS | `POST /backend-api/codex/responses` / 同路径 WS Upgrade |
| Standalone search | `POST /backend-api/codex/alpha/search` |
| Images generation / edit | `POST /backend-api/codex/images/generations` / `images/edits` |
| Models | `GET /backend-api/codex/models?client_version=...` |
| Quota | `GET /backend-api/wham/usage` |
| 消费 reset credit | `POST /backend-api/wham/rate-limit-reset-credits/consume` |

认证使用 access-token Bearer，有 account ID 时加 `ChatGPT-Account-ID`，FedRAMP 按需加
`X-OpenAI-Fedramp: true`。核对版本还发送 originator、User-Agent、版本及操作相关身份 Header。
`codex_cli_rs/0.146.0` 是该研究版本的基础身份，不代表当前安装插件永远固定此值。
Models 的 client_version 会影响返回目录，不能误用网关软件版本。

### 外部请求与响应形状

| 操作 | 固定版本观察 |
| --- | --- |
| HTTP Responses | Responses wire format，流式，`stream=true`、`store=false`；不以 HTTP previous_response_id 恢复连接增量 |
| WS Responses | `response.create`，可用 previous_response_id、generate、client_metadata |
| Search | id/model，及可选 reasoning/input/commands/settings/max_output_tokens；非流式 JSON |
| Images generation | 核对工具使用 gpt-image-2；prompt/background/model，及可选 n/quality/size |
| Images edit | JSON `images[].image_url` data URL，不是公共 multipart |

核对的 ResponsesApiRequest 不声明 `max_output_tokens`；这不是 OpenAI Responses 全局禁令。
Search 响应 output 为最终文本，encrypted_output/results 是可选 opaque 数据。
Images 返回非流式 JSON，大体积 `data[].b64_json` 和顶层 usage 不能迫使代理全量缓冲。

Models 使用 models 数组 envelope；网关保留有效、非空且未显式标记
`supported_in_api=false` 的 slug。Quota 使用 rate_limit allowed/limit_reached、主次窗口
used_percent/limit_window_seconds/reset_at，并可含 reset credits available_count。

reset-credit 请求携带唯一 `redeem_request_id`，外部还允许可选 credit_id；
网关不指定 credit_id，由 OpenAI 选可用 credit。结果 code 为 reset、nothing_to_reset、
no_credit 或 already_redeemed，附 windows_reset；之后应重新读取 quota。

原生客户端和网关补全不同：核对的原生 Images 最小 Header 不复用 Responses session/thread；
插件兼容层可保留调用方会话关联并补缺失值。该增强不意味着 Images 支持 affinity。
HTTP content coding 的固定版本观察见文末来源，不能把网关双向压缩协商写成 Codex 原生保证。

## ai-gateway 兼容行为

### 协议与 Connector 分离

接入连接器为 `codex`，OAuth 凭证子类型仍为 `codex_oauth`。
凭证独立于渠道组/pool，拥有 Token、quota、维护代理和账号目录；
一个凭证可被多个允许目标范围内的逻辑渠道显式引用。

逻辑渠道可配置 Responses HTTP、Responses WS、search、generation/edit 独立能力，不支持 Chat。
各操作自己的目录、变换、模型路由和启停状态不会因凭证导入或重新授权自动创建/恢复。
维护代理只用于 OAuth/refresh/quota 等维护，不替换数据面接入的转发代理。
当前身份模型见[凭证管理](../user/upstream-credentials.md)，不要沿用旧自动配对 projection 的配置步骤。

| 宿主拥有 | 插件拥有 |
| --- | --- |
| 凭证身份/存储、维护调度、锁和恢复 intent | OAuth URL、请求计划、Token/identity/models/quota 解析 |
| 公共入口白名单、授权、准入、路由、模型别名 | 服务商 body/Header 策略、协议字段补全和隐私归一化 |
| 网络/超时/代理、压缩、WS 池、日志和结算 | 后端目标构造、供应商 usage 解析、操作适配 |
| 通用出站安全 guard、凭证目标范围验证 | originator/version/User-Agent、合成 workspace 等插件设置 |

插件通过独立制品与设置 revision 发布不可变 generation。一次 attempt 或维护操作固定所用
generation，不受中途设置修改影响。宿主不能复制 provider 规则进 system settings，
也没有内置 Codex fallback。

### 请求准备

通用顺序为：公共白名单 → Fast 过滤 → 选路/别名 → 管理员变换 → 插件协议与隐私准备
→ 宿主最终安全 guard/鉴权 → transport。插件失败必须终止派发，不能跳过。

| 操作 | 插件适配与宿主传输边界 |
| --- | --- |
| HTTP Responses | 要求客户端 SSE；拒绝非空 previous_response_id；删除契约声明的 max_output_tokens/遥测/no-op，强制 stream=true/store=false |
| WS Responses | 独立字段策略；保留 previous_response_id/generate/client_metadata；不把 HTTP SSE 专用 Header 放进握手 |
| Search | 独立 web_search 路由，非流式；不执行请求 JSON Transform；目标为 Codex alpha/search |
| generation | 按 Images wire type 过滤，等价默认值可删除，注入 image-turn 与凭证身份 |
| edit | 专用 replayable multipart，流式转 data URL JSON；最多五图，不接受 mask 或无法等价表达字段 |

上述 provider 具体字段动作在[外部插件契约](https://github.com/oai404iao/ai-gateway-connectors)的
`codex/src/request-allowlists.json`，公共客户端入口字段在本仓库
[request-allowlists.json](request-allowlists.json)。二者不可混为同一契约。

Responses HTTP 可按操作能力的有效配置将最终适配 body 用 zstd level 3 编码；默认 identity。
此选项不用于 WS/search/Images。HTTP 成功 SSE 由连接器声明分类并规范客户端 Content-Type，
即使上游 Header 缺失/错误；逐块解析终态与 usage，下游 SSE 不压缩。
完成事件后的客户端断开不应把已完成请求覆盖成 client_cancelled。

### 会话身份与隐私

有效客户端 session/thread/request/window 等关联按策略保留，缺失时安全补齐。
HTTP 缺失 session 身份可用匹配 affinity 的不可逆 hash 派生 opaque UUID，否则用本次请求 UUID；
WS 使用下游握手身份的稳定 seed。缺失 x-client-request-id 可使用最终 thread-id。

插件将 flat/client_metadata 和 turn metadata 的 installation ID 替换为凭证稳定 opaque UUID，
workspaces 替换为插件设置的合成 Git 工作区。客户端本地路径、remote、commit、dirty 状态和
原始安装指纹不能直接发给订阅后端。不可解析 turn metadata 安全替换，其他允许 metadata 保留。
插件不凭空生成 beta、routing-hint、turn-state、attestation 或 sandbox 声明。

Search body id 表示其 Codex Session。可分别从 Responses prompt_cache_key 和 Search id
提取各操作的粘性键，但缓存还包含操作规则 ID：即使键值相同，也不会共享凭证绑定。
多凭证路由可能为两个操作选中不同账号；若业务要求同一账号，须在各操作候选中显式限定
对应凭证渠道，不能仅依赖 Session affinity。Images 传递会话关联也不启用 Images affinity。

### 凭证运行时与维护

Token 保存在所选数据库，普通 Console 列表/详情不返回 OAuth secret；管理员显式导出是例外，
返回敏感 Bundle，必须按密钥保护。数据库列未另加静态加密，数据库、备份和浏览器导出边界
都需要保护。Debug/audit 不得暴露 Token。

独立 ArcSwap 凭证快照供数据面读取，不逐请求查库。维护以有界并发刷新 Token、轮询 quota
和清理过期授权流；进程互斥、数据库锁及 refresh_generation 协调同凭证操作。
兑换、rotating refresh 和 reset-credit 有持久 intent/恢复边界，不能以事务回滚或重启为理由
盲目重新发送。永久失败标记 reauth_required，quota 成功或普通编辑不会自行恢复。

同一可识别账号重新导入更新原凭证，不改现有渠道、能力、目录和路由；多个历史记录含糊匹配时
拒绝静默合并。Business 不同 member 不是同一个账号。导出当前为 version 3，
不包含组/渠道归属或 routing weight。

| 凭证状态 | 新请求 | 已有 Responses affinity |
| --- | --- | --- |
| active | 可选择 | 可继续 |
| draining | 排除并在派发前重选 | 可继续 |
| unavailable / disabled | 不派发 | fail closed，不自动换账号 |

Images 无 affinity，派发前可排除不可用能力候选；发送后不换账号。
Codex HTTP 一经派发、WS 一经发消息，不自动重试复制请求。
HTTP/握手 401 和 WS status=401 可触发按 generation 去重的后台 refresh，不代表重放已发送请求。

Quota 主次窗口历史区分自然重置、匹配 Console reset-credit 的 manual 和提前换窗推断的
openai_official；上游 usage 不提供原因，该标签不是官方补偿的确定证明。
网关不自动消费 credit。手动消费结果与后续 quota 刷新分别持久化，不以刷新失败撤销已发生消费。

## 差异与限制

- 不实现跨格式转换；HTTP Responses 不支持非流式，也不借用 WS 增量状态。
- 删除受引用凭证前先解除逻辑渠道引用；删除不调用上游 token revocation，账户侧撤销需另做。
- models 在授权/导入等操作验证写入，不单独周期轮询；能力目录和路由仍需显式配置。
- quota threshold 是选路保护，不是账户硬限额或网关费用重置。
- 多进程凭证快照、WS 池和 affinity 本地存在，数据库变更通过重载收敛；不能假设跨进程共享连接。
- Images 五图/no-mask 是插件限制，不改变普通 multipart 入口十六图边界。
- provider 升级应同时修改插件契约与确定性边界测试；仅更新本文日期不能证明兼容。

## 维护检查项

1. 对比 OAuth 参数、Token 编码/轮换与永久失败分类。
2. 对比后端路径、身份 claim、可选 account/FedRAMP Header、版本过滤和 models/quota schema。
3. 分别验证 HTTP/WS 字段策略、增量状态、Search opaque 结果、Images 模型/形状/usage。
4. 验证 installation/workspace 不泄漏，缺失补全与管理员变换不能绕过插件隐私边界。
5. 验证凭证改绑/轮换隔离、quota/draining、恢复 intent、reset-credit 幂等及 Token 脱敏。
6. 用确定性插件与 gateway-boundary 测试检查；付费 smoke 按[专用流程](../development/real-upstream-smoke.md)授权执行。

## 核对来源与版本

下列链接保留原研究的各补充 commit；“当前”字样仅指原核对当时，不是本次重新验证。
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
> 出站身份和 Responses Header 另外核对本地
> [`openai/codex@fde2156057c38c0227ce94c8514d04c7498df60d`](https://github.com/openai/codex/tree/fde2156057c38c0227ce94c8514d04c7498df60d)：
> [default client identity](https://github.com/openai/codex/blob/fde2156057c38c0227ce94c8514d04c7498df60d/codex-rs/login/src/auth/default_client.rs)、
> [Responses client Headers](https://github.com/openai/codex/blob/fde2156057c38c0227ce94c8514d04c7498df60d/codex-rs/core/src/client.rs)、
> [SSE turn state](https://github.com/openai/codex/blob/fde2156057c38c0227ce94c8514d04c7498df60d/codex-rs/codex-api/src/sse/responses.rs)
> 和 [WebSocket endpoint](https://github.com/openai/codex/blob/fde2156057c38c0227ce94c8514d04c7498df60d/codex-rs/codex-api/src/endpoint/responses_websocket.rs)。
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
> Responses 请求压缩另外核对同一提交的
> [compression selection](https://github.com/openai/codex/blob/7a0e974e08c798d1e8d59d407aeb6e24db1313af/codex-rs/core/src/client.rs)、
> [Responses endpoint](https://github.com/openai/codex/blob/7a0e974e08c798d1e8d59d407aeb6e24db1313af/codex-rs/codex-api/src/endpoint/responses.rs) 和
> [Zstandard encoder](https://github.com/openai/codex/blob/7a0e974e08c798d1e8d59d407aeb6e24db1313af/codex-rs/http-client/src/request.rs)。
> 本地研究 checkout 还核对到 `openai/codex@eb9dceba1a2e658142a456c5898836774835616b`
> 的同一 encoder 实现。
> HTTP content-coding 基线另外核对当前
> [`openai/codex@5af85998c23ddb9cc21c43ef41db44712b481611`](https://github.com/openai/codex/tree/5af85998c23ddb9cc21c43ef41db44712b481611)：
> [default client](https://github.com/openai/codex/blob/5af85998c23ddb9cc21c43ef41db44712b481611/codex-rs/login/src/auth/default_client.rs)、
> [HTTP client features](https://github.com/openai/codex/blob/5af85998c23ddb9cc21c43ef41db44712b481611/codex-rs/http-client/Cargo.toml)
> 和
> [response transport](https://github.com/openai/codex/blob/5af85998c23ddb9cc21c43ef41db44712b481611/codex-rs/http-client/src/transport.rs)。
> 官方客户端当前不主动发送 `Accept-Encoding`，也没有启用 reqwest 的 HTTP 响应解压 features；
> 网关的双向独立协商是代理增强能力，不是该版本 Codex 客户端的保证。
