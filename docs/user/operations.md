# 运行与接口说明

> 状态：当前。

服务是一个 OpenAI 兼容的数据面网关，加上独立的 **Console API**。`/v1/*` 面向 SDK 和程序调用，使用用户 API Key；`/console/v1/*` 面向用户登录和控制面管理，使用 JWT。`admin` 是用户角色，不是另一套接口或静态 Bearer 凭据。

当前运行时同时提供 Console API 和可选的浏览器管理界面。Console API 仍是程序化接口；
浏览器管理界面已实现于 `web/console/`，可通过 `embedded-console-ui` Cargo feature 嵌入并由
Console listener 提供。无论是否启用 UI，本文件描述的 API 行为与边界保持不变。设计详情见
[Console Web UI 设计与实施计划](../development/console-ui.md)。
公共接口与 OpenAI 官方语义的兼容范围见
[OpenAI API 兼容性总览](../reference/openai-compatibility.md)。

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
密码或哈希。若该管理员正处于临时密码恢复状态，此命令也会清除强制改密标记。请确保标准输入中的
新密码至少为 12 个字节，并妥善保护或删除临时密码文件。

## 监听器与请求体限制

```toml
[server]
host = "127.0.0.1"
port = 3000

[request_limits]
proxy_body_bytes = 1_048_576
image_edit_body_bytes = 67_108_864
image_edit_file_bytes = 52_428_800
image_edit_memory_bytes = 1_048_576
image_edit_spool_directory = "./data/image-edit-spool"
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
- `proxy_body_bytes` 限制 JSON OpenAI 代理请求，包括 Images generation。
- `image_edit_body_bytes` 与 `image_edit_file_bytes` 分别限制 multipart edit 总 body 和单个
  image/mask part；`image_edit_memory_bytes` 是转为匿名临时文件前的内存阈值。
- `image_edit_spool_directory` 必须位于容量足够的本地文件系统。Unix 上目录和临时文件分别使用
  `0700` 与 `0600`；图片字节不会进入请求日志。
- `console_body_bytes` 限制已认证 Console 写操作；`auth_body_bytes` 限制登录、注册、刷新和邀请激活请求。

## 上游超时

```toml
[upstream]
connect_timeout_seconds = 10
response_header_timeout_seconds = 30
images_response_header_timeout_seconds = 300
stream_idle_timeout_seconds = 90
```

这些 TOML 值只在数据库 `forwarding_policy` 系统设置不存在时用于首次初始化；之后应在 Console
的“系统设置”页面修改。Images generation/edit 使用独立的
`images_response_header_timeout_seconds`，因为上游通常要完成图片处理后才返回响应头。Chat
Completions、Responses 和其他辅助上游请求继续使用 `response_header_timeout_seconds`。
渠道显式 `response_header_timeout_ms` 始终优先于对应的系统默认值；Images 仍与其他格式共享建连
超时和流空闲超时。两个响应头超时都必须大于有效建连超时。

## 公共数据面

- `GET /health`：返回 `204`，无需认证。
- `GET /v1/models`：列出当前 API Key 可达的模型；需要相应格式的 `proxy` 和 `models.read` 权限。
- `POST /v1/chat/completions`：仅匹配 Chat Completions 路由规则。
- `POST /v1/responses`：仅匹配 Responses 路由规则。
- 带 WebSocket Upgrade 的 `GET /v1/responses`：接受顺序的 Responses
  `response.create` 文本消息，仅匹配 Responses 路由规则。
- `POST /v1/images/generations`：接受带顶层 `model` 的 JSON 请求，仅匹配 Images
  路由规则；当前只支持非流式 generation。
- `POST /v1/images/edits`：接受带 `model`、一个或多个 `image`/`image[]` 和可选
  `mask` 的 `multipart/form-data`，仅匹配 Images 路由规则。

三个 OpenAI 格式绝不互相回退。客户端 `Authorization` 不会转发给上游；网关清理
hop-by-hop headers 后，按渠道配置最后注入上游认证。

数据面在认证后、读取请求体前执行 RPM、并发与已结算软额度预检查。请求体只有在模型别名或 JSON
变换启用时才重新序列化。客户端 `Accept-Encoding` 不直接转发；网关独立向上游声明
`gzip, deflate, br, zstd`，流式解码后执行 usage、错误诊断和 SSE 变换，再按客户端的
`Accept-Encoding` 对可压缩非 SSE 响应流式重编码。已知小于 `1 KiB` 的响应保持 identity；
长度未知的流不会为阈值判断而延迟或缓冲。SSE 下游保持 identity，Range 请求上游也使用
identity；整个过程不缓冲完整响应。未知上游 coding 返回
`502 upstream_content_encoding_unsupported`；读取中才能发现的损坏压缩流会终止响应 body 并
记录 `upstream_body_error`。连接失败、连接超时或等待响应头超时时，可以按系统设置在尚未尝试过
的其他健康渠道上故障转移；一旦收到上游响应头或向客户端发送任何响应字节，绝不重试或切换渠道。

Images generation/edit 是例外：请求一旦开始尝试上游，就不会自动切换渠道或重试，即使失败
发生在响应头之前，以避免重复生成和重复计费。`stream: true` 返回
`400 image_streaming_unsupported`，且不会联系上游。generation JSON 与其他数据面请求共享
`request_limits.proxy_body_bytes`；edit 使用独立的总 body、单文件、内存阈值和 spool 目录，
不会提高全局 JSON 内存上限。未配置渠道级响应头超时时，generation/edit 使用系统设置中的
Images 专用响应头超时，而不是 Chat Completions/Responses 的普通响应头超时。

multipart edit 最多接受 64 个 part、16 张输入图片和一个 mask；普通文本字段最多
单项 `64 KiB`、合计 `1 MiB`；boundary 最多 70 bytes，preamble、单个 part Header block 和
boundary padding 分别最多 `8 KiB`、`16 KiB` 与 `1 KiB`，防止畸形 framing 放大 parser
内存。不需要模型别名时，普通 OpenAI-compatible 渠道收到原始 multipart 字节；需要别名时，
网关流式等价重建并只替换 `model` part。edit 不应用请求 JSON Transform；若选中渠道配置了该类规则，返回
`400 image_edit_json_transform_unsupported`。Header 和响应 Header 变换仍照常执行。当前不接受
JSON/data URL 形式的公开客户端 edit 请求。

配置 Images 路由时，渠道组、渠道、模型规则和 API Key 的格式均使用
`open_ai_images`。Images 渠道不支持 `test_model`，不会进入定时付费探测；Session
粘性、SSE 变换和 WebSocket 也不适用于该格式。普通 Header 变换、请求 JSON 变换、模型别名、
被动健康、准入、请求日志和结算仍沿用统一数据面基础设施。

### Codex OAuth Connect

管理员可以把 ChatGPT Codex 订阅凭证作为共享 Connector pool 接入，而不增加 sidecar 或第二个
转发服务。同一凭证会投影为独立的 Responses 与 Images managed channels。客户端可以调用标准
`POST /v1/responses`、带 WebSocket Upgrade 的 `GET /v1/responses`、非流式 JSON
`POST /v1/images/generations`，或 multipart `POST /v1/images/edits`；控制面用
`connector_kind = codex_oauth` 区分特殊上游方式，各 projection 的 `api_format` 仍分别是
`open_ai_responses` 与 `open_ai_images`。

配置步骤：

1. 在“渠道”页新建 Channel Group，Connector 选择 **Codex OAuth**。创建请求使用
   Responses 格式；保存时服务会在同一 Connector pool 自动创建一个**默认停用**的 Images
   Channel Group。两个 group 的 Connector 类型和格式保存后不可修改。
2. 从渠道组详情或渠道列表进入
   `/admin/providers/codex-oauth/<channel-group-id>`。
3. 选择一种凭证添加方式：
   - **Connect account**：填写 label、可选 outbound proxy、weight 和 quota threshold，
     打开 PKCE Authorization URL；授权后浏览器会落到不可达的
     `http://localhost:1455/auth/callback?...`，复制完整地址回 Console 完成交换。
   - **Import tokens**：提交 access token、refresh token，以及可选 ID token、account ID
     和 user ID。网关先调用 Codex models 接口验证凭证，再创建 Responses 与 Images managed
     channels。
     Workspace account ID 不是所有 ChatGPT 凭证都具备：个人 Free/Plus/Pro Token 可以只带
     user ID。存在 `account_id` 时，凭证按 workspace/member 组合识别；缺少它时按个人
     `user_id` 识别。同一 Business workspace 的不同成员可以分别接入，而同一身份再次连接或
     导入会原位重新授权已有凭证。若 Token 同时缺少 account ID 和 user ID，导入会拒绝。
   - **Advanced import**：进入独立检查页，粘贴 JSON 或上传一个或多个 JSON 文件。
     Console 会自动识别 ai-gateway 原生导出、CLIProxyAPI（CPA）Codex Token 和
     Sub2API 数据导出，把内容先转换成只保存在当前页面内存中的草稿。提交前可以编辑
     label、enable、account ID、user ID、Token、weight、quota threshold 和逐凭证代理分配；还可以
     在同一页面新增、编辑或删除代理，并把导入文件中的代理映射到现有代理。最终仍逐条调用
     服务端凭证验证与导入接口，因此失败条目可在保留其他草稿的情况下修正和重试。
4. 为返回的 Codex model slug 创建或启用本地 model，并创建 Responses model rule，
   将 Responses Channel Group 作为候选。
5. 如需图片生成或编辑，为 `gpt-image-2` 创建或启用本地 model，创建
   `open_ai_images` model rule，选择自动创建的 Images Channel Group，并显式启用该 group。
6. 确保调用方 API Key 允许所需格式、`proxy` 权限和对应格式的 Channel Group。服务不会自动把
   Images format、group 或 channel 加入现有 API Key、Policy 或规则。
7. 需要跨 Responses 请求固定同一订阅账户时，在系统设置中启用 Session affinity，并为目标模型配置稳定
   key source，例如请求 Header `session-id` 或 JSON Pointer `/prompt_cache_key`。

每个凭证自动创建 Responses 与 Images 两个 provider-managed channels。既有 Responses Channel ID
继续作为稳定的凭证 ID；Images 使用独立 Channel ID，因此两个格式的被动健康和日志不会混合。
普通 Channel 详情、批量编辑和 model discovery 接口不能修改这些 channels；label、enable、
proxy、weight 和 quota threshold 必须在 Codex 凭证页维护。
每个凭证的 outbound proxy 独立生效，并由现有 reqwest client registry 按网络与超时策略复用。
凭证 enable、quota 和 refresh 状态由两个 projection 共享的 Connector 运行时持有；底层 managed
channels 保留为路由壳，使已绑定 Responses Session
在凭证关闭后仍能命中原账户并 fail closed，而不是静默切换到另一个账户。

凭证状态含义：

- `active`：可接收新的 Responses Session 或 Images generation/edit；
- `draining`：primary/secondary quota 用量达到 threshold；只允许已命中 affinity 的既有
  Responses Session，Images projection 在发送前被排除；
- `unavailable`：quota 不允许、额度耗尽或 refresh token 永久失效；
- `disabled`：管理员关闭。

永久 refresh 失败会设置持久的重新授权状态；后续 quota 成功或普通设置编辑不会把该凭证重新置为
`active`。重新执行 OAuth 或导入同一 workspace/member 身份，或同一无 workspace 个人 user ID
的新 Token，会复用原 managed channel IDs 并清除该状态。

凭证列表支持多选后批量启用、停用、删除和导出选中项。单条和批量删除都使用乐观并发版本：
删除成功后凭证立即从列表消失，保存的 ID/access/refresh token 被清除，代理引用被释放；为保留
请求日志等历史引用，Responses 与 Images managed channels 都只保留不含敏感信息的 tombstone。
该删除不会调用
OpenAI token revocation endpoint，若还需要使外部 Token 失效，应在账户侧另行撤销授权。

凭证页的 **Export credentials** 需要显式确认，并下载 ai-gateway 原生 JSON Bundle。Bundle 包含
原始 ID/access/refresh token、可选 workspace/member 身份、凭证路由设置，以及被这些凭证引用的代理定义和代理认证信息；它必须
按密钥或未加密备份处理。常规凭证列表和详情接口仍不会返回已保存 Token，只有管理员显式调用导出
接口时才会读取这些敏感字段。高级导入会保留 Bundle 中的 enable 状态；如果 `id_token` 缺失，则
验证阶段从 `access_token` 读取身份声明。

高级导入页允许删除代理，但服务端要求 `If-Match`，并且只有当代理未被普通渠道或未完成的 Codex
OAuth 流引用时才会删除；否则返回 `proxy_in_use`。已分配给导入草稿的代理还必须存在且已启用。

后台 worker 每分钟检查凭证；access token 接近过期时刷新，quota 默认每 5 分钟重新读取。手动
refresh token / quota 也可从凭证页执行。同一凭证的 refresh 在实例内和 PostgreSQL 行锁层面串行，
并核对 generation，避免 rotating refresh token 被并发重复使用。上游请求返回 `401` 时会触发一次
generation 去重的后台刷新。

凭证页同时记录主窗口和次窗口的周期历史。达到计划边界后的换窗显示为“自然重置”；管理员确认后
调用 OpenAI reset-credit 接口并消费可用 credit，随后匹配到的换窗显示为“手动 reset credit”；
没有对应手动事件却提前换窗时显示为“OpenAI 官方重置”。最后一种是根据提前换窗推断的故障补偿等
provider-side 重置，因为 OpenAI usage 响应本身不返回重置原因。Gateway 不会自动消费
reset credit。手动 reset-credit 操作会写审计；如果调用成功但紧随其后的 quota 刷新失败，后台
轮询会继续补齐当前状态和窗口历史。

Codex Responses HTTP Connector 只接受 `stream: true` 的 SSE 请求，强制上游
`store: false`，并拒绝非空 `previous_response_id`。客户端仍可发送
`max_output_tokens`，但选中 Codex managed channel 后，Connector 会在最终上游请求中静默删除
该字段，因为当前 Codex 订阅请求类型不支持它；该值因此不会限制 Codex 输出。这个兼容处理同时
适用于 HTTP SSE 与 WebSocket `response.create`，普通 OpenAI-compatible channel 不受影响。
当前只屏蔽这一项已确认字段，其他未知顶层字段仍保持透明转发。
Codex HTTP 成功响应即使缺少或错误声明上游 `Content-Type`，Gateway 也会向客户端规范化为
`text/event-stream`；非成功 JSON 错误响应仍保留原内容类型。
Codex managed channel 会自动启用
Responses WebSocket 能力；WebSocket `response.create` 同样强制 `stream: true` 和
`store: false`，但保留 `previous_response_id`、`generate` 与 `client_metadata`，使同一条
上游连接可以使用 Codex 增量状态。首次派发前凭证不可用时，未命中 affinity 或 WebSocket pin
的请求可以换到同组其他凭证；HTTP 请求或 WebSocket 消息一旦发送到 Codex，不做跨凭证自动重试。
已命中 affinity 的凭证处于 `unavailable`、`disabled` 或 Token 过期时，会在 affinity TTL
内持续 fail closed；已经固定到 managed channel 的 WebSocket Session 也不会改用其他订阅账户。

Codex Images generation 保留模型别名和受限变换后的 JSON，请求目标改为
`/backend-api/codex/images/generations`。Connector 注入共享凭证的 Bearer、可选
account/FedRAMP、
`originator`、版本、User-Agent 和新生成的 `x-codex-image-turn-id`，并删除客户端
`session-id`、`thread-id` 与 `x-client-request-id`。Images 不使用 Session affinity；发送前若凭证
不可用可以选择同一 Images group 的其他 projection，但请求一旦发送就不会自动换账户或重试。
成功响应按非流式 JSON 转发并增量提取顶层 usage，不会为 `data[].b64_json` 缓冲完整响应。

Codex Images edit 接收相同客户端模型的 multipart 请求，并在 replayable body 上流式读取图片，
转换为 `/backend-api/codex/images/edits` 的 JSON `images[].image_url` data URL。该 adapter
provider-specific 地限制最多五张输入图片、不接受 mask，并只转发 `prompt`、`background`、
`model`、`n`、`quality` 和 `size`。未核对字段返回 `codex_image_edit_field_unsupported`，
不会静默丢弃。认证、image turn Header、draining 和发送后不重试边界与 generation 相同。

客户端已有的合法 `session-id` / `thread-id` 会转发。缺少时，HTTP 请求若匹配 Session affinity，
会从不可逆 session hash 派生稳定 opaque UUID；未匹配 affinity 的 HTTP 请求仅使用本次请求
UUID。WebSocket Session 从下游握手身份派生稳定 seed，使顺序请求和池化重连使用一致身份。

OAuth token 不会进入 audit/debug 输出；除显式管理员导出接口外，也不会由常规 Console API 返回。
当前仍与普通 upstream API key 一样依赖受保护的 PostgreSQL、备份和主机访问边界，未额外实施列级
静态加密。外部接口与限制见
[Codex OAuth 与订阅后端接入参考](../reference/codex-oauth-connect.md)和
[Codex 凭证导入格式兼容性](../reference/codex-credential-portability.md)。

### Responses WebSocket

WebSocket Upgrade 在 HTTP 握手阶段验证 Gateway API Key 和 Responses `proxy`
权限，但不消耗 RPM 或并发槽。该传输默认关闭，只有以下三层均开启才接受请求：

1. 管理员在 `/console/v1/system/settings` 中设置 `websocket.enabled = true`；
2. 用户在个人设置页 `/account/settings` 中开启 WebSocket，对应
   `GET/PUT /console/v1/me/settings` 的 `websocket_enabled`；
3. 管理员在普通 OpenAI Responses 渠道上设置 `supports_websocket = true`；Codex OAuth
   Responses projection 在创建时自动设置，不提供普通 Channel 编辑入口。

migration 后的现有系统、用户和普通渠道以及所有新普通渠道都保持关闭，必须显式启用。现有和新建
Codex OAuth Responses projection 会自动声明 WebSocket 能力，但 Images projection 永不声明；
系统与用户开关仍默认关闭。
Chat Completions 渠道不能声明 WebSocket 支持。系统或用户未开启时，HTTP Upgrade 返回
`403 websocket_disabled`；没有可用且声明支持的 Responses 渠道时，首条
`response.create` 返回 `503 no_healthy_channel`。

每个后续 `response.create` 才作为一个独立逻辑请求：

- 重新检查 API Key 是否仍有效，执行 RPM、并发和软额度准入；
- 从消息顶层 `model` 完成 Responses 路由、模型别名和请求 JSON 变换；
- 应用渠道请求 Header 变换；若最终缺失则注入
  `OpenAI-Beta: responses_websockets=2026-02-06`，再应用上游认证，然后连接或复用上游
  WebSocket；
- 将上游 Responses JSON 事件逐消息转发，Responses SSE 事件变换规则也用于同类型
  WebSocket 事件；
- 在 `response.completed`、`response.failed`、`response.incomplete`、
  `response.cancelled` 或 `error` 终态记录 usage、计费和请求日志。

OpenAI 的增量 `previous_response_id` 缓存属于具体上游 WebSocket 连接，因此网关不会把同一条连接上的
请求多路复用到多个上游连接。每个请求成功终止后，只有没有残留消息的上游连接才会立即归还进程内
有界空闲池；同一条或重连后的下游 Session 会优先取回这个精确连接。池按 Gateway API Key、下游握手
身份、渠道、目标 URL、代理/TLS 策略和最终上游请求 Header 精确隔离；不同下游 Session 不共享连接级
上下文。

每条 WebSocket 连接同时只允许一个 `response.create` 在途。上游握手完成前的连接类失败仍可按全局
重试设置切换未尝试渠道；消息一旦发往上游，就不再自动重试，以避免重复生成。连接期间客户端 API Key
被撤销或过期后，下一条消息会收到 `invalid_api_key` 错误；系统或用户开关在连接期间关闭后，下一条
消息会收到 `websocket_disabled` 并结束连接。

由于上游渠道只能在下游 Upgrade 完成并收到首条 `response.create` 后确定，上游 Upgrade 响应 Header
无法回填到已经完成的下游握手；配置的响应 Header 变换因此只适用于 HTTP Responses，WebSocket
事件变换仍正常生效。HTTP、HTTPS、SOCKS4/4a 和 SOCKS5/5h 渠道代理策略均用于上游 WebSocket 建连。
若公共 listener 前有 TLS/负载均衡反向代理，必须允许 WebSocket Upgrade，并把连接空闲和最长时限
设置得足以覆盖模型响应；网关自身仍不终止 TLS。

Codex 会在握手中发送 `session-id`、`thread-id`、`x-client-request-id`、`originator` 和
User-Agent。网关会把这些非 hop-by-hop Header 纳入连接池隔离并转发；反向代理和渠道 Header 变换
不应无意删除它们。Codex 请求形状与恢复逻辑见
[Codex Responses WebSocket 实现参考](../reference/codex-responses-websocket.md)。

系统设置中的 WebSocket 连接池参数为：

- `max_idle_connections`：进程级最大空闲上游连接数，范围 `0..=4096`，默认 `128`；`0`
  表示保留 WebSocket 转发但不复用空闲连接。
- `idle_timeout_seconds`：空闲连接保留时间，范围 `1..=3600`，默认 `300`。
- `max_connection_age_seconds`：连接总寿命，范围 `60..=3600`，默认 `3300`，且必须大于
  空闲超时。

进程收到关闭信号后不再接受新的 WebSocket Upgrade，立即清空空闲上游池并用 `1001` 关闭空闲下游
连接；已经在途的 `response.create` 可在 `shutdown_grace_period_seconds` 内完成，超过时限后会被
强制取消并按客户端取消记录。

## Console 认证

Console 登录接口：

- `POST /console/v1/auth/login`
- `POST /console/v1/auth/register`
- `POST /console/v1/auth/refresh`
- `POST /console/v1/auth/activate-invitation`
- `POST /console/v1/auth/complete-password-reset`（仅限临时密码登录后的受限 Session）
- `POST /console/v1/auth/logout`（需要 access JWT）

登录、自助注册或邀请激活成功后：

- 响应 JSON 返回短期 Access JWT，客户端以 `Authorization: Bearer <token>` 调用 Console API；
- 响应设置轮换的 `HttpOnly; Secure; SameSite=Lax` refresh Cookie；
- refresh token 仅保存 SHA-256 哈希。刷新时会轮换；重放旧 refresh token 会撤销该 session；
- 每个 Console 请求都会验证 JWT 签名、issuer、audience、用户状态、session 状态和 `auth_version`。禁用用户、改密码、登出和角色变化会立即使旧 token 失效。
- 新建或刷新 session 时会保存最长 512 字符的浏览器 `User-Agent`，供账户本人在登录设备页面识别会话。
  升级前已存在的 session 在下一次刷新后补齐该字段；网关不根据未经信任的转发 Header 推断客户端 IP。

### 管理员辅助密码恢复

已设置密码的 `active` 用户忘记密码时，管理员可在用户详情页生成临时密码，对应
`POST /console/v1/users/{id}/temporary-password`。操作要求管理员重新输入自己的当前密码，
且不能对当前登录的管理员本人执行；管理员自身的紧急恢复继续使用另一管理员或主机上的
`reset-admin-password` 命令。`invited` 或从未设置密码的账户仍使用重新签发邀请流程，
`suspended` / `disabled` 账户必须先恢复为 `active`。

临时密码由服务端随机生成，固定有效 24 小时，只在创建响应中显示一次。签发或重新签发会立即：

- 替换目标用户原有 Console 密码，使旧密码和上一个临时密码失效；
- 递增 `auth_version` 并撤销该用户全部 Console 登录 Session；
- 保持用户角色、状态、余额、用户组、Policy 和数据面 API Key 不变；
- 写入不含密码或哈希的 `issue_temporary_password` 审计记录。

用户用临时密码调用普通登录接口后会得到 `password_change_required = true` 的
password-change Session。该 Session 的 access/refresh 有效期不会超过临时密码有效期，并且后端
只允许刷新、退出和调用 `/auth/complete-password-reset`；直接请求个人、统计或管理员资源会返回
`403 password_change_required`。完成接口只接收新密码，拒绝与临时密码相同的值。成功事务会替换为
正式密码、清除临时状态、递增 `auth_version`、撤销全部受限 Session，并立即签发新的普通 Session。
临时密码过期后不会恢复旧密码，必须由管理员重新生成。

账户有两种创建方式，当前都不要求邮箱确认：

1. **管理员按用户邀请。** 管理员先创建 `invited` 用户，可通过
   `initial_balance_amount` 设置非负的初始 USD 余额；省略时为 `0`。响应中的
   `invitation_token` 只返回一次，外部邮件/通知系统负责投递。邀请有效期为 7 天，用户通过
   `/auth/activate-invitation` 设置密码并激活账户。
2. **邀请码自助注册。** 匿名用户向 `/auth/register` 提交管理员创建的注册邀请码、邮箱、显示名称和
   密码。成功后直接创建 `role = user`、`status = active` 的账户并立即签发 Console session。
   同一邮箱仍保持大小写无关唯一。

注册邀请码由管理员自定义，长度为 12 到 128 个字符、区分大小写且不能包含空白。数据库只保存
SHA-256 哈希，明文仅在创建响应中返回一次，之后无法查看或修改。每个邀请码可独立设置可选的最大
使用次数、可选过期时间、启用状态、目标用户组和非负初始 USD 余额；次数和过期时间为空分别表示
不限次数和永不过期。管理员可以调整上述设置，修改只影响后续注册。注册时在 serializable 事务中
锁定邀请码、再次检查启用/过期/剩余次数、创建用户并递增使用次数，失败不会消耗次数。

每个用户必须属于一个用户组。按用户邀请未显式指定 `user_group_id` 时，普通用户进入内置“默认
用户组”，管理员进入内置“默认管理员组”；自助注册使用邀请码当前配置的用户组。这两个系统组可以
修改名称、说明和默认策略，但不能删除。

## 普通用户接口

所有下列资源均强制从 JWT 主体推导 user ID，不能通过路径或 body 参数访问他人的数据：

- `GET/PATCH /console/v1/me`
- `GET/PUT /console/v1/me/settings`
- `POST /console/v1/me/password`
- `GET /console/v1/me/sessions`
- `DELETE /console/v1/me/sessions`（撤销除当前会话外的所有活跃会话）
- `DELETE /console/v1/me/sessions/{id}`
- `GET/POST /console/v1/me/api-keys`
- `GET /console/v1/me/api-key-options`
- `GET/PUT /console/v1/me/api-keys/{id}`
- `POST /console/v1/me/api-keys/{id}/revoke`
- `GET /console/v1/me/request-logs?limit=50`
- `GET /console/v1/me/request-logs/{id}`
- `GET /console/v1/me/usage`
- `GET /console/v1/me/codex-quotas`
- `GET /console/v1/me/codex-quotas/{credential-id}/windows?limit=100`

`GET /console/v1/me/sessions` 返回每条 session 的浏览器 `user_agent`、`active` / `expired` /
`revoked` 状态和 `is_current` 标记。`last_seen_at` 表示 refresh token 最近一次轮换时间，而不是每个
Console HTTP 请求的最后访问时间。按 ID 撤销当前 session 时，响应还会清除 refresh Cookie；所有
撤销操作都在 SQL 中以 JWT 主体的 user ID 限定资源归属。

用户的有效 `api_key_policy` 按“用户覆盖优先，否则继承用户组默认策略”解析。Policy 只定义用户
可选择的渠道组和单独渠道。用户通过
`GET /console/v1/me/api-key-options` 获取当前可选列表；创建或更新 API Key 时，从该列表中选择
`allowed_group_ids` / `allowed_channel_ids`，并为该 Key 独立配置 RPM、最大并发和可选额度上限。
API 格式由所选目标自动推导，自助创建 Key 的权限固定为 `proxy` 和 `models.read`。

Policy 不再保存额度、RPM、并发、格式、权限或最大活动 Key 数，也不会反向修改既有 Key 的实际限制。
未分配策略、策略已禁用或提交了策略范围外的目标时，接口分别返回
`default_api_key_policy_required`、`default_api_key_policy_disabled` 或
`api_key_target_not_allowed`。

用户组还可以授权只读查看指定 Codex 凭证池的额度窗口。管理员只配置 canonical
`open_ai_responses` Codex Channel Group；同一 Connector pool 的 Images projection 自动共享这份
可见性。普通用户接口只返回凭证 UUID（`name` 固定使用同一个 UUID）、Provider 报告的
`plan_type`、当前主/次窗口以及窗口周期历史，不返回管理员 label、邮箱、可选 workspace/member
身份、Token、代理、权重、运行状态、错误或 reset-credit 信息。接口没有写方法，也不提供
refresh、reset、编辑或导出操作；未授权凭证与不存在的凭证统一返回 `404`。

## 管理员接口

拥有 `role = admin` 的用户可使用全部普通用户接口，以及以下 Console 控制面接口：

- 用户与邀请：`/console/v1/users`
- 用户组：`/console/v1/user-groups`
- 注册邀请码：`/console/v1/registration-invitation-codes`
- 用户批量修改：`POST /console/v1/users/batch`
- API Key Policy：`/console/v1/api-key-policies`
- 全局 API Key：`/console/v1/api-keys`
- 模型：`/console/v1/models`
- models.dev：`/console/v1/catalog/models/sync/preview`、`/sync`、`/import`
- 路由：`/console/v1/routing/channel-groups`、`/channels`、`/model-rules`
- 渠道批量修改：`POST /console/v1/routing/channels/batch`
- 网络：`/console/v1/network/proxies`、`POST /console/v1/network/proxies/test`
- 变换模板：`/console/v1/transforms/templates`
- 观测事实：`GET /console/v1/request-logs`、`GET /console/v1/audit-logs`
- 花费排行榜：`GET /console/v1/statistics/spend-leaderboard`
- 系统负载：`GET /console/v1/system/load`（当前实例的 CPU、内存、运行时、队列、日志积压、Responses WebSocket Session/连接池和数据库连接池压力；Console 页面位于“运维”下的 `/admin/system-load`）
- 系统转发设置：`GET` / `PUT /console/v1/system/settings`（管理员；`PUT` 使用 `If-Match`，保存后立即发布快照）
- 手动重载：`POST /console/v1/system/reload`

用户详情支持带 `If-Match` 的 `PATCH /console/v1/users/{id}`，只修改请求中出现的字段；
例如仅提交 `balance_amount` 不会重写邮箱、角色、用户组、策略或状态。用户级
`default_api_key_policy_id` 是可选覆盖；显式设为 `null` 后立即恢复继承用户组默认策略。
`invited` 是邀请流程拥有的待激活
状态，管理员修改资料或余额时会保持该状态，只有持有邀请令牌的用户完成激活后才会变为
`active`。兼容用的完整 `PUT` 仍保留，但新客户端应使用 `PATCH`。

`POST /console/v1/users/batch` 一次最多接收 100 个用户及各自的 `updated_at` 版本，可原子地统一
修改运行状态、用户组、用户级 API 策略覆盖和余额。余额支持设置绝对值、统一增加或统一扣减；
任一用户版本过期、状态转换非法或引用不存在时，整批修改与审计全部回滚。包含当前管理员时，
批量操作不能暂停或禁用其自己的账户。

`DELETE /console/v1/users/{id}` 需要 `If-Match` 和 Console 二次确认。删除不会物理移除用户主键：
服务会清空邮箱与密码、匿名化显示名称、撤销全部会话、未接受邀请和 API Key，并从管理列表隐藏
该用户；请求日志和审计记录继续保留原 user ID。管理员不能删除自己，也不能删除最后一个活跃的
非系统管理员。匿名化后原邮箱可重新使用。

用户组通过 `/console/v1/user-groups` 管理。每个组可设置一个默认 API Key Policy，并通过
`visible_codex_quota_group_ids` 选择成员可只读查看额度的 canonical Codex Responses Channel
Group；普通 OpenAI-compatible group、Codex Images projection、重复 ID 或不存在的 group 都会被
拒绝。修改后，没有用户级覆盖的组成员立即使用新策略，Codex 额度可见性也立即按当前用户组查询。
自定义组只有在没有成员时才能删除；内置默认用户组和默认管理员组始终受保护。仍被注册邀请码引用的
用户组同样不能删除，必须先把相关邀请码调整到其他组。

注册邀请码通过 `/console/v1/registration-invitation-codes` 管理。列表和详情只返回名称、启用状态、
次数、过期时间、用户组、初始额度和使用统计，不返回明文或哈希。详情 `GET` 返回 `ETag`，调整名称、
最大次数、过期时间、启用状态、用户组或初始额度时必须用 `PUT` 携带 `If-Match`；最大次数不能调低到
当前 `used_count` 以下。邀请码值本身不可调整，如需更换，应创建新邀请码并禁用旧邀请码。

对于邀请过期、令牌丢失，或历史版本误把待激活用户改成 `disabled` 的情况，管理员可调用
`POST /console/v1/users/{id}/invitation` 重新签发邀请。该操作仅适用于尚未设置密码的
`invited`、`suspended` 或 `disabled` 用户；会保留用户资料、策略和余额，将状态恢复为
`invited`，撤销所有旧邀请令牌，并返回一个新的只显示一次的令牌。没有密码的账户不能由管理员
直接改成 `active`。

其他大多数可更新资源遵循 `GET` 返回 `ETag`、`PUT` 携带 `If-Match` 的乐观并发模型。控制面
写入在 serializable 事务中再次确认 actor 仍为 active admin，校验完整候选快照、写入脱敏
审计记录，并在提交后立即发布运行时快照。

代理编辑页可以在保存前测试当前 HTTP、HTTPS 或 SOCKS 代理草稿。测试接口固定通过该代理请求
ip-api.com，并显示观察到的出口 IP、位置、ISP、自治系统、网络类型和请求耗时；它刻意忽略
`enabled` 与 `no_proxy_hosts`，也不会修改渠道健康或运行时快照。已有代理的隐藏凭据只会在代理
端点未改变时复用；更换 scheme、host 或有效 port 后必须重新输入凭据。免费 ip-api.com 接口使用
HTTP、仅允许非商业用途并带独立限流，因此结果只适合作为人工诊断信息。外部语义和限制见
[ip-api.com 代理出口 IP 查询](../reference/ip-api-proxy-test.md)。

渠道的 `billing_multiplier` 为非负十进制数，默认 `1`。最终选定渠道的倍率会乘到模型
输入、缓存输入、缓存写入和输出单价上；请求日志保存乘算后的有效价格快照，因此历史费用
不受后续倍率调整影响。批量修改接口一次最多接收 100 个渠道及各自的 `updated_at` 版本，
可统一修改启用状态、自动禁用授权、权重和计费倍率。任一版本过期或候选路由
配置无效时，整批修改、审计和运行时发布都会回滚。

渠道组的 `status_statistics_enabled` 控制是否进入公开的渠道组状态监控报告。启用后，
报告会聚合该组全部成员渠道的请求和可用模型；单个渠道不再提供独立的监控开关。

模型的 `advanced_billing.request_multipliers` 会在请求变换前，对原始客户端 JSON
请求体执行 JSON Pointer 精确匹配；所有命中的倍率与渠道倍率相乘，并应用到整次请求费用。
从 models.dev 导入或更新模型时，网关会尽力把
`experimental.modes.*.provider.body.service_tier` 及其统一缩放的输入、缓存和输出价格转换为
`/service_tier` 请求倍率。目录没有该信息、价格不是统一倍率或结构无效时，只跳过该可选规则，
不会排除基础模型；显式价格更新会更新相同匹配条件的目录倍率，同时保留其他本地请求倍率。

渠道与变换模板的列表接口只返回摘要字段；管理员读取单条详情时，渠道响应还会返回
`upstream_api_key` 与 `override_document`，模板响应会返回 `document`，供 Console
编辑页回显和直接修改。上述详情响应仍使用 `Cache-Control: no-store`，审计日志继续排除
上游密钥和变换文档。

`GET /console/v1/system/load` 是只读、管理员权限的当前实例快照。Linux 上从 procfs
采样主机与网关进程 CPU、内存、load average、RSS、文件描述符和线程数；不支持的平台将对应字段
返回 `null`。它还返回进程内准入与路由 in-flight 状态、请求日志通知/投影队列、自动禁用队列、
本地 spool pending bytes、PostgreSQL ingress/settlement backlog 以及控制面和请求日志连接池占用。
Responses WebSocket 部分返回全局启用状态、活跃下游 Session、空闲和借出的上游连接、空闲池容量/
占用、命中/未命中/丢弃累计计数以及当前空闲超时和连接最长寿命。
`image_body_spool` 另外返回 multipart edit 的活跃临时文件/字节、文件系统可用容量、累计落盘
body/字节和存储失败次数。容量不足或捕获/回放存储失败必须按数据面硬失败告警；上游派发前发现的
失败返回 `503 image_body_spool_unavailable`。
CPU 百分比依赖相邻采样差值，因此进程启动后的首次采样可能为 `null`。这些数据不是多实例集群聚合；
Console 的“系统负载”页默认每 5 秒重新获取一次。

所有已登录用户可在 Console 的“统计”页面查看自己的“个人使用情况”。页面固定展示截至当前
UTC 日期的连续 365 天客户端请求数，并使用类似 GitHub 贡献图的日期网格显示每日强度；没有请求的
日期也会保留。摘要同时显示总请求数、活跃天数、当前连续活跃天数和最长连续活跃天数。该接口只从
JWT 主体推导用户 ID，管理员也只能在个人使用情况标签中看到自己的数据；系统定时渠道测试不会计入。

同一“统计”页面的“花费统计”标签也始终限定为当前 JWT 用户，包括管理员。它只支持时间区间、
API Key 和小时/天聚合粒度，不提供用户或渠道筛选，响应中的渠道明细固定为空。
管理员需要查看全局、指定用户或指定渠道的花费时，使用“系统”分组下独立的“花费统计”页面；
该页面调用 `GET /console/v1/system/statistics/costs`，可按用户、API Key、单一渠道或 Codex
逻辑凭证筛选并显示渠道明细。Codex 凭证筛选会同时覆盖其 Responses 与 Images managed channels，
并与单一渠道筛选互斥。凭证页的任一主/次窗口周期都可以直接跳转到这里，自动带入周期时间范围和
该 Codex 凭证筛选。

所有已登录用户可在 Console 独立的“花费排行榜”页面查看用户花费排名。页面固定提供自然日、
自然周和自然月榜，均按 `Asia/Shanghai` 时区切分：日榜为当日 00:00 至次日 00:00，周榜为周一至周日，
月榜为每月 1 日至月底；并可前后浏览已保留的历史榜单；不再提供任意日志时间范围筛选。后台每 15 分钟汇总一次排行榜快照，因此当前
数据不是实时数据，除刷新间隔外还会受到请求日志投影和刷新耗时影响。前三名使用领奖台柱状图展示，排行榜表格显示最多 50 名用户的已记录 USD
花费、占比、已计价请求数和 Token。排行榜仅包含该周期内至少有一个已定价请求的用户；其总花费来自
客户端请求的 `request_logs.cost_amount`，不等待异步结算 worker 写入 `billed_at`；系统定时渠道测试不会参与排行榜。

## 自动禁用与定时测试

`/console/v1/system/settings` 的完整配置还包含：

- `websocket.enabled`、`max_idle_connections`、`idle_timeout_seconds` 和
  `max_connection_age_seconds`：Responses WebSocket 总开关和进程级上游空闲池策略。
- `request_retry.enabled`：是否启用响应头前故障转移，默认启用。
- `request_retry.max_retries`：首次请求失败后的最大自动重试次数，范围 `1..=10`，默认 `1`。同一客户端请求不会重复尝试同一渠道。
- `automatic_disable.enabled`：自动禁用总开关。关闭时，即使渠道允许自动禁用也不会执行状态变更。
- `automatic_disable.error_status_codes`：触发临时禁用的上游 HTTP 状态码列表。
- `automatic_disable.error_message_keywords`：触发临时禁用的上游错误消息关键字；匹配大小写不敏感。自动禁用扫描器本身不保存正文；请求日志会为失败的非流式 HTTP、SSE 和 Responses WebSocket 请求保存最长 16KiB、已清理控制字符的文本或结构化错误详情。
- `scheduled_testing.mode`：`global` 测试全部启用渠道；`failure_only` 只测试临时自动禁用的渠道。
- `scheduled_testing.auto_recover`：测试成功后是否自动清除临时禁用。
- `scheduled_testing.interval_minutes`：测试间隔，默认 `5`。
- `scheduled_testing.prompt`：测试 prompt，默认 `reply '1'`。

渠道的 `auto_disable_allowed` 必须为 true 才会被自动禁用；`test_model` 必须从该渠道的
`available_models` 中选择，并匹配已配置模型的 `source_model_id`，以便保存价格快照。定时测试按渠道 API 格式发出非流式 Chat Completions 或 Responses 请求，
并复用该渠道的代理、超时、变换和上游鉴权配置。Images 渠道不能配置 `test_model`，
不会被定时测试。手工禁用的渠道与禁用渠道组不会被测试。

定时测试日志写入 `request_logs`，`request_source` 为 `scheduled_test`。它们使用系统内置、
管理员角色的内部 API Key。网关会解析响应中的 token 用量，并按该模型的不可变价格快照、模型高级计费规则和渠道计费倍率计算成本；结算会扣减该系统管理员账户余额并累计其内部 API Key 的额度用量，不会归属到任何普通用户。系统内部身份不会出现在用户和 API Key 管理列表中。自动禁用和自动恢复都会写入系统审计日志并立即发布新的路由快照。
管理员也可以在渠道列表中直接启用、禁用或手工恢复渠道。手工恢复只清除
`auto_disabled` 与其原因，不会改变渠道显式的 `enabled` 值，并使用列表中的
`updated_at` 做并发版本检查。

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

管理员 Console 会按规则显示当前进程中尚未过期的有效缓存数量，并可清理单条规则或
整个进程的 Session 粘性缓存。对应接口是
`GET/DELETE /console/v1/system/session-affinity/cache`；清理只影响当前进程内存，
不会修改数据库中的规则。

多实例部署没有共享粘性缓存；若同一 Session 被负载均衡到不同 Gateway 进程，各实例会独立学习渠道。

## 日志、用量与结算

每次故障转移会产生 `proxy_request_retry` tracing 事件；每个客户端请求仍只产生一个终态 tracing
事件和一条 `request_logs`，其中渠道、结果和计费快照对应最终尝试。worker 从三种格式的普通 JSON
以及 Chat Completions/Responses 的 SSE 事件增量提取 usage，在选路时绑定价格快照，并在可结算时以 `billed_at` 条件幂等更新用户余额
和 API Key 已用额度。usage 同时保留输入、缓存命中、缓存写入、输出总量，以及输出中包含的
reasoning token。Chat Completions 的 `completion_tokens` 始终作为包含 reasoning 的输出总量保存；
OpenAI 的最终空 `choices` usage chunk 和 DeepSeek 将 usage 附在 `finish_reason` chunk 的形式都可解析。
Console 请求日志的 `Tokens` 列将未缓存输入和输出总量作为主数字，并用紧凑标记分别展示缓存命中
与作为输出子集的 reasoning token。

对于客户端原始请求，日志还会在不改变转发校验或请求字节的前提下提取显式模式元数据：
OpenAI Responses 的 `reasoning.effort`、DeepSeek/OpenAI Chat Completions 兼容的
`reasoning_effort`，以及表示 OpenAI Priority processing 的
`service_tier = "priority"`。Console 模型列按顺序显示思考等级和绿色 `Fast` 标记；未显式提供
对应字段时不显示。耗时列第三行显示输出 TPS，计算为
`output_tokens / ((total_duration_ms - ttft_ms) / 1000)`；usage 或 TTFT 不可用时显示空值。

额度是软预检查：不预留金额，已结算额度达到上限后才拒绝后续请求；余额可以为负。
终态请求日志先同步追加到本地 durable spool，后台通知队列饱和只会合并唤醒，不会丢弃
spool 中的事件。spool 写入失败和磁盘空间耗尽仍是必须告警的耐久边界。

Console 请求日志会显示请求协议，将请求区分为非流式 HTTP、SSE 或 Responses
WebSocket，并在 API 响应中返回独立的 `api_operation`，区分 Chat Completions、
Responses、Images generation 与 Images edit；同时显示渠道组名称。个人“请求日志”
始终只查询当前 JWT 用户；即使当前用户是管理员，
服务端也会将用户名称、具体 `channel_id` 和渠道名称置空。个人列表固定显示开始时间、模型、
请求协议、渠道组、结果、Token、成本和耗时；模型旁可显示思考等级和 `Fast` 标记，耗时同时包含
TTFT、总耗时和 TPS。详情再显示 API operation、HTTP 状态、错误代码、错误详情和完成时间。错误详情
可能包含上游返回的完整结构化错误对象、文本错误正文前缀或网关/传输诊断，并最多保留 16KiB。

只有管理员“系统”栏下的全局“请求日志”可以读取所有用户的日志。该页面调用管理员接口
`GET /console/v1/request-logs`，并在上述字段基础上额外显示
当前用户名称和渠道名称；Console 列表与详情不显示用户、渠道或请求日志 ID。

## 已知边界

- 支持 Chat Completions、Responses、非流式 JSON Images generation 与非流式 multipart
  Images edit；不提供 JSON/data URL Images edit、图片流式响应、embeddings、audio、files、
  batches、assistants 或 fine-tuning API。
- 所有余额、额度、模型价格和请求费用统一使用 USD；没有跨实例限流、健康状态或 Session
  粘性协调。Chat Completions 与 Responses 的自动重试仅覆盖收到响应头前的连接失败、连接超时
  和响应头超时，不覆盖 HTTP 错误、SSE 流中断或流空闲超时；Images generation/edit 不自动重试。
  系统也没有独立财务账本、充值/退款或货币兑换。
- 服务本身不终止 TLS；Console 必须部署在正确配置的 HTTPS 反向代理后。
