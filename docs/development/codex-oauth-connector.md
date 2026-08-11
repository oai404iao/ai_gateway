# Codex OAuth Connector 设计记录

> 状态：当前。描述 `connector_kind = codex_oauth` 的已实现架构与扩展约束。

## 决策

特殊上游使用**静态链接、进程内 Connector**，不增加 sidecar、Unix Socket RPC、动态
`.so` 或 WASM。客户端使用标准 `POST /v1/responses`、带 Upgrade 的
`GET /v1/responses`、Codex 扩展 `POST /v1/alpha/search`、非流式 JSON
`POST /v1/images/generations` 或 multipart `POST /v1/images/edits`；Connector 只改变选中
format-specific channel 之后的上游准备过程。

`ApiFormat` 与 `ConnectorKind` 必须分离：

```text
client format: OpenAiResponses | OpenAiImages
client operation: Responses | StandaloneWebSearch | ImagesGeneration | ImagesEdit
upstream connector: CodexOauth
```

因此模型规则、API Key format 权限和 usage 解析仍使用对应的 Responses 或 Images 格式，不新增
`ApiFormat::CodexResponses` / `CodexImages`，也不允许格式间转换。

## 运行时边界

`UpstreamConnectorRegistry` 在进程启动时组装并注入 `ProxyService`。代理主循环只依赖
统一 attempt 接口：

1. `prepare`：在发送前验证动态凭证与 affinity 状态；
2. `adapt_body`：按 HTTP SSE、Responses WebSocket、standalone web search、Images JSON 或 multipart edit 应用
   provider 请求约束；
3. `upstream_url`：解析最终目标；
4. `inject_headers`：在通用变换和 hop-by-hop 清理之后注入最终认证；
5. `allows_automatic_retry`：声明 pre-header transport failure 是否可跨 channel 重试；
6. `observe_response`：处理 `401` 等 provider 状态，不延迟客户端响应。

普通 `OpenAiCompatible` attempt 保留原路径、原请求字节和 `UpstreamAuth`。Codex 细节集中在
`src/application/codex/attempt.rs`；`src/application/proxy.rs` 不引用 Codex 类型、路径、
Header 或错误分类。

正式 HTTP 请求直接通过共享 reqwest client streaming 转发；WebSocket 使用共享的上游拨号、代理
和有界连接池。worker 只维护凭证状态，不承载代理流量。

## 持久化模型

`channel_groups.connector_kind` 决定该组使用的 Connector。Codex group 可以使用
`open_ai_responses` 或 `open_ai_images`，保存后 Connector 类型和格式不可修改。

`connector_pools` 把一个 Responses group 与一个 Images group 组成共享凭证池；
`codex_oauth_credential_channels` 把每条凭证投影到两个 managed `channels` 记录：

- 既有 `codex_oauth_credentials.channel_id` 保留为稳定凭证 ID 与 Responses Channel ID；
- Responses 与 Images channel 的 `weight`、`proxy_id` 和各自 `available_models` 继续进入统一
  路由快照；
- credential 的逻辑 `enabled` 和动态状态由 Connector 快照持有；底层 managed channel
  始终保留为可选择的路由壳，Connector prepare 再排除新 Session 或让 affinity hit fail closed；
- 两种 channel 都固定 `upstream_auth_kind = none`、`auto_disable_allowed = false`；
  Responses 声明 `supports_websocket = true`，Images 必须为 false；状态监控不再是 channel
  属性，而由 Responses 与 Images 各自的 `channel_groups.status_statistics_enabled` 独立控制，
  新建时默认关闭；
- 普通 channel create/update/batch API 在 repository 层拒绝 provider-managed channel；
- provider mutation 在同一控制面事务中更新凭证与 channel、写 audit、编译候选快照并发布。

新建 Codex Responses group 时会同时创建一个默认关闭的 Images group。migration 对现有 group 和
凭证执行同样投影，但不会增加 API Key format、Policy、模型规则或可访问路由。管理员必须显式启用
Images group，并配置 `gpt-image-2` 的本地模型、Images rule 和权限。

凭证的持久身份不是单独的 workspace account ID，而是
`(connector_pool_id, account_id?, user_id)`，并要求 account ID 与 user ID 至少存在一个。
`user_id` 优先来自 `chatgpt_user_id`，兼容同 namespace 的 `user_id` 和 JWT 顶层 `sub`
fallback。这样同一 Business workspace 中的多个成员可以分别成为路由凭证，而没有 workspace
account ID 的个人凭证按 user ID 独立保存。缺少 user claim 的旧 workspace Token 仍按
account/email 回退匹配，服务启动时会从已保存 Token 尽力补齐旧记录的 user ID。

删除使用软删除凭证记录加 managed-channel tombstone：事务内关闭凭证、清除三个 OAuth Token、
释放 proxy、记录 `deleted_at` 并把 Responses 与 Images channel 改成唯一 tombstone 名称。列表、
导出、刷新和后续身份匹配忽略已删除记录，但 channel 壳继续保留，使请求日志和显式 channel 引用
不失去历史主键。

OAuth PKCE 临时状态单独保存在 `codex_oauth_flows`，按 actor、group、过期时间和
`completed_at` 限定。数据库只保存 `state` 的 SHA-256；`code_verifier` 在一次性 flow
完成或清理前保存。

## 凭证快照与维护

Access token 不编入完整 `CompiledRuntimeConfig`，而是保存在独立
`CodexCredentialRuntime` / `ArcSwap<HashMap<projection_channel_id, credential>>` 中。
Responses 与 Images projection 指向同一个不可变 credential，数据面每次 attempt 只执行内存读取。

worker 每分钟加载数据库记录并先替换本地凭证快照，使其他实例完成的 enable、quota 或 token
更新最终收敛。需要维护的凭证以有界并发执行：

- access token 在过期前 5 分钟刷新；没有 `exp` 时使用保守 fallback age；
- quota 默认 15 分钟刷新；
- 完成或过期的 OAuth flow 清理；
- 单凭证先获取进程内 mutex，再在 PostgreSQL transaction 中
  `SELECT ... FOR UPDATE`，锁内核对 `refresh_generation` 后才调用 token endpoint；
- refresh 事务提交前更新 rotating token、generation、identity 和错误状态；取消或失败时
  transaction drop 会释放 row lock。

每次有效 quota 快照还会事务内维护
`codex_quota_window_periods`。主窗口和次窗口各自最多有一个当前周期，周期起点由
`reset_at - limit_window_seconds` 推导；每次观察更新末次使用率和观察时间，换窗时关闭旧周期并
开启新周期。计划边界附近的换窗优先归类为自然重置；提前换窗再按推导出的实际周期起点匹配附近的
手动 reset-credit 事件，因此即使 quota 观察延迟，仍可在恢复后补齐手动重置分类。quota
请求发起时间同时作为快照版本，较晚返回的旧请求不能覆盖更新的凭证状态或窗口历史。历史不进入
运行时凭证快照，也不参与数据面选路。Provider 在窗口始终为 `0%` 时可能按查询时间滑动
`reset_at`；在首次观察到非零使用率之前，这些快照会更新同一个未锚定周期，而不会生成连续的
`openai_official` 历史记录。历史读取还会过滤旧版本已经写入的零使用率
`openai_official` 误报。

worker、上游 `401` 恢复和多实例并发均传递 observed generation；如果其他执行者已经成功轮换，
后续执行者直接结束，不能再次消费旧 refresh token。管理员显式手动刷新不带 observed generation，
因此表示一次强制刷新。

永久 refresh 失败设置持久的 `reauth_required`，maintenance 不再自动重复消费该 Token，quota
成功和普通设置更新也不能清除状态。再次 OAuth 或导入相同
Connector pool/workspace/member，或相同 accountless personal user ID 的新 Token
会事务内更新原 credential/channel、递增 generation 并清除 `reauth_required`，不会创建重复
channel。

## Quota 与 Session 粘性

Quota 状态：

| 状态 | 新 Session | affinity hit |
| --- | --- | --- |
| `active` | 允许 | 允许 |
| `draining` | 拒绝并排除该 channel 后重选 | 允许 |
| `unavailable` | 拒绝 | fail closed |
| `disabled` | 拒绝并排除该 channel 后重选 | fail closed |

窗口换窗原因记录为：

- `natural`：新周期在旧周期计划重置边界附近或之后开始；
- `manual`：管理员通过 Console 消费 OpenAI reset credit，随后观察到的换窗与该事件匹配；
- `openai_official`：没有匹配的手动 reset-credit 事件，却在计划边界前观察到新周期。当前上游
  usage 响应不返回“故障补偿重置”原因，因此这是基于提前换窗的明确推断，不是 OpenAI 提供的
  原始枚举。

管理员手动重置调用 OpenAI 的
`/backend-api/wham/rate-limit-reset-credits/consume`，使用唯一
`redeem_request_id`。返回的 `reset`、`nothing_to_reset`、`no_credit` 或
`already_redeemed` 结果、重置窗口数和 correlation ID 会持久化并写审计；随后立即刷新 quota。
调用期间持有该凭证的数据库行锁，使其他实例的 quota 持久化必须在 reset 事件提交后再分类换窗。
若后续刷新失败，reset-credit 结果仍返回成功，后台 quota 轮询会继续补齐窗口历史。该操作不会被
maintenance 自动触发；OpenAI 因故障主动补偿的重置也只做观察和记录，不消耗 Gateway 发起的
reset credit。

Connector prepare 发生在上游发送前。非 affinity hit 遇到不可用凭证时，代理把当前 dense
channel slot 加入排除集合并重新使用统一路由器；排除集合先使用固定 inline 容量，只有凭证池超过
普通重试上限时才分配 overflow `Vec`。这样标准请求保持无分配路径，而大凭证池仍能遍历到可用账户。

Affinity binding 只在成功终态后写入。首次选择后若凭证进入 `draining`，同一个 Session 仍可继续；
新 Session 不会绑定到 draining 凭证。若已绑定凭证变为 unavailable/disabled/expired，不自动换
账户，也不会因本次失败删除 affinity；绑定会保留到正常 TTL/清理边界，以免后续请求静默切换
provider 账户。

Standalone web search 使用 Responses affinity：Codex 请求 body 的 `/id` 与触发 Search 的
Responses Session ID 相同，推荐规则同时配置 `/prompt_cache_key` 和 `/id`。已命中 affinity
的 Search 请求沿用 Responses fail-closed 边界。

Images 不使用 Session affinity。Images 请求在发送前遇到 draining/unavailable/disabled 凭证时，
可以排除当前 projection 并选择同一 Images group 的其他凭证；一旦发送则不再换账户。

客户端 `session-id` / `thread-id` 优先保留。缺失时，HTTP 请求若匹配 affinity，则从 session
hash 加 domain separation 派生稳定 opaque UUID；没有 affinity 的 HTTP 请求生成本次请求 UUID。
WebSocket Session 从下游握手身份派生稳定 seed，使同一条下游连接及其可复用上游连接始终使用
一致的 Codex Session/thread identity。

## 请求与重试边界

Codex HTTP attempt：

- 要求 SSE streaming；
- 强制 `stream=true`、`store=false`；
- 在通用 JSON Transform 之后应用 Codex Responses HTTP body 白名单；
- 把 flat `x-codex-installation-id` 与 turn metadata 中的 `installation_id` 替换为按逻辑凭证
  稳定的 opaque UUID；存在的 `workspaces` 折叠为固定 `{"/workspace":{}}`，其他 metadata 和
  W3C trace/baggage 保留；
- 删除显式兼容的 `max_output_tokens` 和纯遥测字段；`previous_response_id` 只允许空值后删除；
  其他已知但 provider 无法表达的非默认值以及未知字段返回客户端错误；
- 目标固定为 managed channel base URL 下的 `/responses`；
- 注入 Bearer、存在 workspace account ID 时才注入 `ChatGPT-Account-ID`、可选 FedRAMP、
  session/thread、User-Agent、
  `originator` 和版本 Header。
- `Accept-Encoding` 由通用 HTTP 代理层拥有；Codex Connector 不单独指定 coding，通用层向
  上游声明 gzip、deflate、Brotli 与 Zstandard，并在 SSE 解析前流式解码。
- 成功响应无论上游如何声明 `Content-Type`，都按 SSE 处理并向客户端规范化为
  `text/event-stream`；非成功响应不强制改写。

Codex WebSocket attempt：

- 只接受 `response.create` 文本消息，并同样强制 `stream=true`、`store=false`；
- 在通用 JSON Transform 之后应用独立的 Codex Responses WebSocket body 白名单，并删除
  `max_output_tokens` 与显式纯遥测字段；
- 保留客户端的 `previous_response_id`、`generate` 和 `client_metadata`；
- 对 `client_metadata` 应用与 HTTP 相同的安装 ID/工作区归一化，不改写 Session/thread、
  turn-state 或其他 metadata；
- 把 managed channel base URL 转成 `/responses` 的 `ws`/`wss` 目标；
- 使用 Codex 同源的 WebSocket Beta、Bearer/可选 account、FedRAMP、session/thread、
  User-Agent、`originator` 和版本 Header，不发送 HTTP SSE 专用的 `Accept`、
  `Accept-Encoding` 或 `Content-Type`；
- 成功终态后只复用无残留的同一上游连接，保留 connection-local
  `previous_response_id` 状态。

Codex standalone web search attempt：

- 只接受 `ApiOperation::StandaloneWebSearch` 的非流式 JSON；
- 在模型别名之后应用独立 Search body 白名单，允许 `id`、`model`、`reasoning`、`input`、
  `commands`、`settings` 与 `max_output_tokens`，不添加 body override；
- 不允许 Request JSON Transform，但继续应用 Header 和响应 Header Transform；
- 目标固定为 managed Responses channel base URL 下的 `/alpha/search`；
- 保留客户端 `originator` 与合法的 `x-codex-turn-metadata`，originator 缺失时使用 Connector
  默认值；turn metadata 的安装 ID 与工作区使用同一凭证级固定投影；
- 注入 Bearer、存在时的 account、可选 FedRAMP、User-Agent 和版本，删除
  `session-id`、`thread-id` 与 image-turn Header；
- 成功响应按非流式 JSON 处理，`results` DTO 不解释、不重写；没有可识别 usage 时不估算 token
  或费用。

Codex Images generation attempt：

- 只接受 `ApiOperation::ImagesGeneration` 的非流式 JSON；
- 在模型别名和受限变换后应用 Codex Images generation body 白名单，只保留 wire type 字段；
  `output_format=png`、`moderation=auto` 等契约列出的等价值被删除，无法表达的非默认值返回错误；
- 目标固定为 managed channel base URL 下的 `/images/generations`；
- 注入 Bearer/可选 account、FedRAMP、User-Agent、`originator`、版本和 Gateway 生成的
  `x-codex-image-turn-id`；
- 删除客户端 `session-id`、`thread-id` 与 `x-client-request-id`；
- 把流式解码后的成功响应按普通 JSON 交给 Images usage collector，不按 SSE 解释。

Codex Images edit attempt：

- 只接受 `ApiOperation::ImagesEdit` 的非流式 multipart；
- 复用 `ReplayableRequestBody` 顺序读取 image parts，并把最多五张图片增量 base64 编码为
  JSON `images[].image_url` data URL；
- 在客户端 multipart 白名单之后应用 Codex Images edit body 白名单，只保留
  `prompt`、`background`、`model`、`n`、`quality` 与 `size`；
- provider-specific 地拒绝 `mask`、第六张图片及无法等价删除的字段；客户端兼容字段和
  `output_format=png` 等 provider 默认值按机器契约删除；
- 目标固定为 managed channel base URL 下的 `/images/edits`；
- 使用与 generation 相同的 Bearer/可选 account/FedRAMP、User-Agent、`originator`、版本和新
  `x-codex-image-turn-id`，不发送 Responses Session Header；
- 成功响应继续按普通 JSON 与 Images usage collector 处理。

preparation 失败可以在发送前换凭证。HTTP Codex attempt（包括 standalone web search）不启用普通 transport retry，因为
reqwest 返回 pre-header error 时不能证明请求体未被上游接收。WebSocket 只允许在上游 Upgrade
完成且尚未发送 `response.create` 前按全局重试策略换 channel；已经命中 affinity 或下游
WebSocket pin 的 Codex Session fail closed，不换账户。消息发出后不重试。Responses 或 Images
HTTP `401`、WebSocket 握手 `401` 或带 `status = 401` 的终态错误会按共享 credential ID 异步触发
generation 去重 refresh；已经发送的图片请求不会重放。

客户端入口和 Codex 出口的完整 Header/body 动作由
[`request-allowlists.json`](../reference/request-allowlists.json) 定义，维护说明见
[`请求字段与 Header 白名单`](../reference/request-allowlists.md)。Codex Header policy 位于
普通 Header Transform 与最终凭证/协议 Header 注入之间，因此自定义 Header 不能绕过 provider
白名单，也不能覆盖最终认证。

## 安全边界

- Console 只返回 token 元数据，不返回保存的 ID/access/refresh token。
- reset-credit 操作只返回结果、窗口数和 correlation ID，不返回 credit 或 OAuth secret。
- credential `Debug`、audit before/after 和错误摘要必须脱敏。
- 删除必须在提交前清除持久化 OAuth Token；tombstone 不得继续引用 proxy 或出现在凭证列表/导出。
- callback URL、authorization code 和导入 token 不进入日志或浏览器持久化状态。
- Codex 客户端原始 installation ID、本地 workspace 路径、Git remote、commit 和 dirty 状态不
  发往订阅后端。安装 ID 按逻辑凭证稳定，workspace 统一投影为 `/workspace`；其他
  `client_metadata`、turn metadata 和 W3C trace/baggage 不在本功能中改写。
- 当前 token 与普通 upstream API key 一样以数据库明文列保存；部署者必须保护 PostgreSQL、备份、
  主机和 Console 管理权限。若未来增加列级加密，应使用明确的进程主密钥配置和轮换设计，不能在
  Connector 内临时引入不可恢复的本地密钥。
- OAuth 外部语义不是网关保证；见
  [Codex OAuth 与订阅后端接入参考](../reference/codex-oauth-connect.md)。

## 新增下一种 Connector

新增 provider 时：

1. 增加 `ConnectorKind` 和 group/channel 编译校验；
2. 在独立 provider 模块实现 attempt 与动态凭证运行时；
3. 向 `UpstreamConnectorRegistry` 注册，不修改标准 Connector 行为；
4. 使用 managed channel 复用统一路由、代理、权限、日志和计费；
5. 增加 provider migration、Console OpenAPI、生成类型和独立管理页；
6. 明确 streaming、发送后重试、Session affinity、WebSocket 和 secret 存储边界；
7. 添加协议 mock、数据库事务、端到端转发、quota/draining、并发 refresh 和脱敏测试；
8. 在 `docs/reference/` 记录权威来源、核对日期和外部变化检查项。
