# 运行与接口说明

> 状态：当前。

服务是一个 OpenAI 兼容的数据面网关，加上独立的 **Console API**。`/v1/*` 面向 SDK 和程序调用，使用用户 API Key；`/console/v1/*` 面向用户登录和控制面管理，使用 JWT。`admin` 是用户角色，不是另一套接口或静态 Bearer 凭据。

当前运行时同时提供 Console API 和可选的浏览器管理界面。Console API 仍是程序化接口；
浏览器管理界面已实现于 `web/console/`，可通过 `embedded-console-ui` Cargo feature 嵌入并由
Console listener 提供。无论是否启用 UI，本文件描述的 API 行为与边界保持不变。设计详情见
[Console Web UI 架构与开发指南](../development/console-ui.md)。
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

启动时服务会应用 migration、从选定数据库编译不可变数据面快照、启动配置重载和请求日志 worker。
默认步骤使用 PostgreSQL；Linux SQLite 的配置、CLI 停机要求和备份恢复见[专用指南](sqlite.md)。
空控制面可以启动，但没有有效 API Key 和路由规则时无法代理请求。

服务不读取 dotenv。JWT Ed25519 私钥和公钥通过受限文件路径配置，不写入 TOML。

### 上游拓扑硬切换升级

PostgreSQL migration `0065` / SQLite migration `0005` 将旧渠道组、渠道、协议规则和
Codex 双投影一次性转存为管理组、接入、逻辑渠道、操作能力与显式候选，随后删除旧配置表。
启动迁移在同一事务中完成转存、历史外键迁移和完整快照编译；任一步失败整批回滚。
修复后可重试，重复启动不会重复转存。旧日志、金融事实、结算收据、spool 和拼车账本不改写。

随后 `0066` / SQLite `0006` 拆分六种操作并删除可配置 transports；
`0067` / SQLite `0007` 将能力授权按所属逻辑渠道去重迁移为渠道授权。
已授权渠道的所有能力共享授权，但组中新加入的渠道不会自动授权。
`0068` / SQLite `0008` 删除凭证的组/pool 归属，将拼车绑定及 `sharing_only` 移到逻辑渠道；
`0069` / SQLite `0009` 增加不可变逐请求凭证归属，避免渠道改绑转移历史或在途费用。
原凭证、渠道、能力、席位、窗口、金额和 WAL 身份均保留；旧未完成的组绑定 OAuth 流失效。
完整快照在整批迁移结束后验证，失败仍回滚整批。

升级必须停止所有旧 Gateway 写入，先备份数据库和 spool；SQLite 使用成对离线备份。
不能新旧版本滚动混跑，也不能把已升级数据库直接交给旧二进制。需要回滚时恢复完整备份及
匹配版本。升级预检拒绝无法表示的停用草稿、非法凭证范围或路由；错误只报告 ID/类别，
请先在旧版本修复或删除这些配置，不能跳过检查或手工清空迁移状态。
更早版本仍须依次满足历史 migration 的别名、价格和路由约束。
迁移边界见[上游身份与能力设计](../development/upstream-identity-capabilities.md)。

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
standalone_web_search_response_header_timeout_seconds = 300
stream_idle_timeout_seconds = 90
```

这些 TOML 值只在数据库 `forwarding_policy` 系统设置不存在时用于首次初始化；之后应在 Console
的“系统设置”分类页面修改。左侧菜单可点击展开，按基础设置、上游超时、重试与健康、
定时测试、会话亲和、WebSocket、Codex 和运行维护分类；每类有独立地址，可直接打开或刷新。
保存只提交当前页面的编辑，其他分类保留读取时的值；整份配置仍使用同一个 ETag 防止覆盖
并发修改。切换分类前请先保存，未保存编辑不会带到另一个页面。Images generation/edit 使用独立的
`images_response_header_timeout_seconds`，因为上游通常要完成图片处理后才返回响应头。Chat
Completions、Responses 和其他辅助上游请求继续使用 `response_header_timeout_seconds`。
非流式 `/v1/alpha/search` 使用
`standalone_web_search_response_header_timeout_seconds`，因为 provider 可能在返回响应头前执行
多个 Search command。
渠道显式 `response_header_timeout_ms` 始终优先于对应的系统默认值；Images 仍与其他格式共享建连
超时和流空闲超时。所有响应头超时都必须大于有效建连超时。

## 公共数据面

- `GET /health`：返回 `204`，无需认证。
- `GET /v1/models`：列出当前 API Key 可达的模型；需要相应格式的 `proxy` 和 `models.read` 权限。
- `POST /v1/chat/completions`：仅匹配 Chat Completions 路由规则。
- `POST /v1/responses`：仅匹配 Responses 路由规则。
- `POST /v1/alpha/search`：Codex standalone web search 扩展；复用 Responses 路由规则和权限，
  但只选择显式声明 `supports_standalone_web_search = true` 的 Responses 渠道，并固定使用
  非流式 JSON。
- 带 WebSocket Upgrade 的 `GET /v1/responses`：接受顺序的 Responses
  `response.create` 文本消息，仅匹配 Responses 路由规则。
- `POST /v1/images/generations`：接受带顶层 `model` 的 JSON 请求，仅匹配 Images
  路由规则；当前只支持非流式 generation。
- `POST /v1/images/edits`：接受带 `model`、一个或多个 `image`/`image[]` 和可选
  `mask` 的 `multipart/form-data`，仅匹配 Images 路由规则。

三个 OpenAI 格式绝不互相回退。客户端 `Authorization` 不会转发给上游；网关清理
hop-by-hop headers 后，按渠道配置最后注入上游认证。


所有公开数据面请求先应用客户端入口白名单：未列出的 Header 被忽略，未列出的顶层 JSON 或
multipart 字段返回 `400 request_body_field_unsupported`。当前只检查顶层字段，允许字段内部的
嵌套结构仍由上游解释。完整字段和动作见
[`请求字段与 Header 白名单`](../reference/request-allowlists.md)。

`POST /v1/responses` 支持客户端以 `Content-Encoding: zstd` 发送 JSON body；网关会先解码，
再执行模型解析、入口白名单和路由。压缩后的请求和解压后的 JSON 都不能超过
`request_limits.proxy_body_bytes`。其他 JSON 数据面路由只接受 identity，Images edit 继续只
接受未编码 multipart body。

数据面在认证后、读取请求体前执行 RPM、并发与已结算软额度预检查。客户端/Connector policy
均未删除或覆盖字段、且没有模型别名或 JSON 变换时才保留原始请求字节。客户端
`Accept-Encoding` 不直接转发；网关独立向上游声明
`gzip, deflate, br, zstd`，流式解码后执行 usage、错误诊断和 SSE 变换，再按客户端的
`Accept-Encoding` 对可压缩非 SSE 响应流式重编码。已知小于 `1 KiB` 的响应保持 identity；
长度未知的流不会为阈值判断而延迟或缓冲。SSE 下游保持 identity，Range 请求上游也使用
identity；整个过程不缓冲完整响应。未知上游 coding 返回
`502 upstream_content_encoding_unsupported`；读取中才能发现的损坏压缩流会终止响应 body 并
记录 `upstream_body_error`。连接失败、连接超时或等待响应头超时时，可以按系统设置在尚未尝试过
的其他健康渠道/模型候选上故障转移；管理员也可以显式列出允许在转发响应前重试的 4xx/5xx
上游状态码。未配置的 HTTP 状态直接转发；一旦向客户端发送响应头或任何响应字节，绝不重试或
切换候选。

Images generation/edit 是例外：请求一旦开始尝试上游，就不会自动切换渠道或重试，即使失败
发生在响应头之前，以避免重复生成和重复计费。`stream: true` 返回
`400 image_streaming_unsupported`，且不会联系上游。generation JSON 与其他数据面请求共享
`request_limits.proxy_body_bytes`；edit 使用独立的总 body、单文件、内存阈值和 spool 目录，
不会提高全局 JSON 内存上限。未配置渠道级响应头超时时，generation/edit 使用系统设置中的
Images 专用响应头超时，而不是 Chat Completions/Responses 的普通响应头超时。

Images 的 `x-codex-image-turn-id` 可由调用方传入：普通渠道透传；Codex 渠道保留有效的
客户端/Transform 值，仅在缺失或不可用时生成随机 UUID。该 Header 只关联图片调用，不用于
鉴权，不会在 Codex Responses/Search 出口透传。鉴权与 Connector 身份仍由网关最终覆盖。
Codex Connect 的 Images / Search 同时像 Responses 一样保留 `session-id`、`thread-id`、
`x-client-request-id`、`x-codex-window-id` 和 turn metadata，仅缺失时补全。安装 ID / 工作区
继续隐私归一化；会话头传递不会启用 Images affinity，也不会改变独立请求的 body 格式。

Standalone web search 允许顶层 `id`、`model`、`reasoning`、`input`、`commands`、
`settings` 和 `max_output_tokens`；结果 JSON 中的 `output`、`encrypted_output` 和
`results` 原样流式转发，不解释 `results` DTO。该操作允许模型别名和 Header/响应 Header
Transform，但不应用 Request JSON Transform。开启 Search 能力的渠道若组合出非空 Request JSON
Transform，控制面编译失败。

Search 的 `id`、`input`、commands 和 settings 不借用 Responses body 规则。无需模型别名或
策略删除时保留原始 JSON；网关不会额外加入 `stream`、`store`、`client_metadata` 或伪造
`input`。

multipart edit 最多接受 64 个 part、16 张输入图片和一个 mask；普通文本字段最多
单项 `64 KiB`、合计 `1 MiB`；boundary 最多 70 bytes，preamble、单个 part Header block 和
boundary padding 分别最多 `8 KiB`、`16 KiB` 与 `1 KiB`，防止畸形 framing 放大 parser
内存。不需要模型别名时，普通 OpenAI-compatible 渠道收到原始 multipart 字节；需要别名时，
网关流式等价重建并只替换 `model` part。客户端 policy 删除兼容字段时也会通过同一 replayable
路径重建 multipart。edit 不应用请求 JSON Transform；若选中渠道配置了该类规则，返回
`400 image_edit_json_transform_unsupported`。Header 和响应 Header 变换仍照常执行。当前不接受
JSON/data URL 形式的公开客户端 edit 请求。

配置 Images 路由时，使用 `images_generation` 或 `images_edit` 能力及同操作路由规则；
Key 必须获得所属逻辑渠道授权，不再单独限制 API 格式。Images 能力不支持 `test_model`，不会进入定时付费探测；Session
粘性、SSE 变换和 WebSocket 也不适用于该格式。普通 Header 变换、请求 JSON 变换、目标模型改写、
被动健康、准入、请求日志和结算仍沿用统一数据面基础设施。

### 模型规则路由层级

`models.source_model_id` 是客户端模型身份，最多绑定一个 routing profile。每个 profile
按操作分别配置 `chat_completion`、`responses`、`responses-ws`、`web_search`、
`images_edit`、`images_generation` 规则；不会跨格式转换、降级或借用另一操作的路由。
停用规则可以是空草稿，启用规则必须包含非空 tier。

每个 tier 保存 `capability_id + upstream_model + weight` 显式候选。较低 priority
优先；在当前最低可用层内使用 `weighted_random` 或 `weighted_round_robin`。
同一能力可以使用不同 wire model 或出现在不同层；同层完全相同的能力/模型组合不能重复。
保存时 wire model 必须在能力目录中。计费始终使用 profile 的计价模型，转发使用候选模型。
后续移除目录模型、禁用组/接入/凭证/渠道/能力会影响可用性，但不会替换候选。

管理组不再拥有格式或连接器；接入拥有 Base URL、连接器、代理和超时；逻辑渠道绑定管理组、
接入与 nullable 凭证；能力拥有操作、模型目录、健康、探测、压缩、变换、倍率和统计开关。
能力的所属渠道与操作创建后不可修改。
接入连接器枚举为 `general`（通用）或 `codex`，与凭证类型 `codex_oauth` 不同。
传输由操作固定：Chat Completions/Responses HTTP 支持 JSON 与 SSE，`responses-ws`
只使用 WebSocket，搜索与图片生成使用非流式 JSON，图片编辑使用 multipart。
Console 中从“模型配置”选择客户端模型，再在右侧按操作添加路由规则。
客户端模型列表和选中模型面板展示输入、缓存命中、输出基础价格，并标明实际价格单位
Token 数；高级计价仍在价格配置页维护。

Key/Policy 选择组或逻辑渠道，不提供能力选择器。组选择在保存时展开为固定逻辑渠道集合；
保留选择不重新展开，后来加入组的渠道需要显式授权。组来源授权还要求渠道仍属于原组。
已授权逻辑渠道的现有和后续新增能力均共享授权，不再逐能力或按 API 格式限制。
自助 Key 的普通目标必须属于有效 Policy 的固定渠道范围；拼车目标只来自本人固定席位，
没有 Policy 也可创建仅拼车 Key。候选配置、能力启用和渠道授权仍是三个独立步骤。
旧管理 API 的 `allowed_api_formats` 作为废弃兼容字段保留，输入可省略且不参与授权；
活跃 Key 返回全部受支持格式。协议验证仍按请求操作执行，不能跨格式转换。

删除普通渠道能力时，后端在同一事务中解绑所有引用它的路由候选，删除空层，并把无候选的
规则停用为草稿；仍有候选的规则保留原开关、权重和策略。受影响规则的 ETag 更新。
版本冲突或审计失败会整体回滚；渠道、计价模型、固定渠道授权及历史计费事实不变。
Codex 使用相同的渠道与能力管理接口；删除能力同样自动解绑候选，不删除凭证或历史金额。

### Codex OAuth Connect

可选 [Codex 拼车](codex-sharing.md) 将直接分配给用户的固定席位绑定到逻辑渠道及其账号，并增加 USD
双窗口预占/结算门禁。该单实例功能默认关闭；启用后不借用其他凭证，Images 仍需显式启用并配置路由，
经拼车凭证的不可计价 standalone search 会被拒绝，同一用户经普通渠道的请求保持原行为。
Codex 逻辑渠道可开启“仅拼车使用”；同账号别名不能绕过限制，但不自动开启 Images。
车队可先保存空席；本人创建 Key 时，拼车渠道与 Policy 普通目标分栏选择，没有 Policy
也可创建仅含席位渠道的 Key。

管理员可把 ChatGPT Codex 订阅作为 Connector 接入，无需 sidecar。每个凭证保留稳定
credential UUID，可由多个兼容的逻辑渠道引用，没有组或池归属。渠道可显式配置 Responses HTTP、
Responses WS、独立 Search、Images generation、Images edit 能力。Token、quota 和维护代理属于凭证，
转发代理属于接入，健康、目录和路由按能力分离，访问授权属于逻辑渠道。

配置步骤：

1. 打开 **上游凭证 → Codex**：`/admin/routing/upstream-credentials?connector=codex`。
2. 使用 **Connect account** 的 PKCE 流程，或 **Import tokens** / **Advanced import**。
   PKCE 回调地址仍为 `http://localhost:1455/auth/callback`，复制完整地址回 Console 完成交换。
   高级导入支持原生 Bundle、CPA 和 Sub2API；草稿与 Token 只留在当前页面内存。
   workspace/member 或无 workspace 的个人 user ID 用于原位重授权，不按 Token 文本去重。
3. 配置 Codex 上游接入、渠道组及引用该凭证的逻辑渠道，再在渠道详情添加操作能力和模型目录。
   导入不会自动创建渠道或能力；旧配置迁移保留 Images 的停用状态，重导入不覆盖渠道的独立目录。
   HTTP Responses 的 zstd 压缩只在 Responses 能力配置，不影响 Search、Images 或 WebSocket。
4. 建立计价模型/profile，并为需要的每个操作分别保存能力/模型候选。Search 不再隐式借用
   Responses 规则；Images generation/edit 也各有规则。新增凭证不会自动加入候选。
5. 显式给 API Key/Policy 添加所需组或逻辑渠道；Key 需要 `proxy` 权限。
   新导入渠道不会自动加入已有组授权；已授权渠道的全部能力无需逐项重新授权。
6. 客户端 Codex provider 的 base URL 指向 Gateway `/v1`；独立搜索仍需客户端
   `supports_standalone_web_search = true`。需要 Responses/Search 固定账户时配置 Session affinity。

Codex 认证生命周期通过 `/console/v1/routing/upstream-credentials/codex` 及其子路径管理。
逻辑渠道与能力使用统一 CRUD；普通静态密钥接口不能替代 OAuth 认证流程。
删除凭证前必须解除全部活动渠道引用，且不得存在拼车绑定；删除只清除 Token 和维护代理，
不级联删除接入、渠道、能力或金融事实。维护代理修改不改写渠道接入的转发代理。

凭证状态含义：

- `active`：可接收新的 Responses Session、standalone web search 或 Images generation/edit；
- `draining`：primary/secondary quota 用量达到 threshold；只允许已命中 affinity 的既有
  Responses Session，Images 能力 在发送前被排除；
- `unavailable`：quota 不允许、额度耗尽或 refresh token 永久失效；
- `disabled`：管理员关闭。

永久 refresh 失败会设置持久的重新授权状态；后续 quota 成功或普通设置编辑不会把该凭证重新置为
`active`。重新执行 OAuth 或导入同一 workspace/member 身份，或同一无 workspace 个人 user ID
的新 Token，会复用原凭证并清除该状态，不更改已绑定渠道和能力。

凭证列表支持多选后批量启用、停用、删除和导出选中项。单条和批量删除都使用乐观并发版本：
删除成功后凭证立即从列表消失，保存的 ID/access/refresh token 被清除，代理引用被释放；为保留
请求日志等历史引用，逻辑渠道与能力只保留不含敏感信息的 tombstone。
该删除不会调用
OpenAI token revocation endpoint，若还需要使外部 Token 失效，应在账户侧另行撤销授权。

凭证页的 **Export credentials** 需要显式确认，并下载 ai-gateway 原生 JSON Bundle。当前 Bundle
为 version 2，包含原始 ID/access/refresh token、可选 workspace/member 身份、enable/quota
设置，以及被这些凭证引用的代理定义和代理认证信息，但不包含 routing weight；它必须按密钥或
未加密备份处理。常规凭证列表和详情接口仍不会返回已保存 Token，只有管理员显式调用导出接口时
才会读取这些敏感字段。高级导入会保留 Bundle 中的 enable 状态；如果 `id_token` 缺失，则验证
阶段从 `access_token` 读取身份声明。旧原生或外部 JSON 中若带 `weight`，前端会忽略并显示
warning。新凭证不会自动加入现有模型规则；管理员必须在协议路由编辑器中逐条添加其
能力候选。

高级导入页允许删除代理，但服务端要求 `If-Match`，并且只有当代理未被接入或未完成的 Codex
OAuth 流引用时才会删除；否则返回 `proxy_in_use`。已分配给导入草稿的代理还必须存在且已启用。

后台 worker 每分钟检查凭证；access token 接近过期时刷新，quota 默认每 15 分钟重新读取。手动
refresh token / quota 也可从凭证页执行。同一凭证的 refresh 在实例内和 PostgreSQL 行锁层面串行，
并核对 generation，避免 rotating refresh token 被并发重复使用。上游请求返回 `401` 时会触发一次
generation 去重的后台刷新。

凭证页同时记录主窗口和次窗口的周期历史。达到计划边界后的换窗显示为“自然重置”；管理员确认后
调用 OpenAI reset-credit 接口并消费可用 credit，随后匹配到的换窗显示为“手动 reset credit”；
没有对应手动事件却提前换窗时显示为“OpenAI 官方重置”。最后一种是根据提前换窗推断的故障补偿等
provider-side 重置，因为 OpenAI usage 响应本身不返回重置原因。Gateway 不会自动消费
reset credit。窗口使用率一直为 `0%` 时，Provider 可能按每次查询时间重新计算 `reset_at`；
Gateway 会把这类滑动时间合并为同一个未使用周期，首次出现非零使用率时再固定周期边界，不会把
每次查询误记为“OpenAI 官方重置”；历史查询也会隐藏升级前已产生的 `0% → 0%`
“OpenAI 官方重置”误报。手动 reset-credit 操作会写审计；如果调用成功但紧随其后的 quota
刷新失败，后台轮询会继续补齐当前状态和窗口历史。

当前主/次窗口和每条历史周期同时显示该逻辑凭证的 USD 周期花费。金额会合并 Responses 与
Images 各能力及旧投影上所有用户、API Key 和请求来源的已计价请求，因此它表示凭证总花费，
不是当前查看用户的个人花费。该值由 Gateway 保存的模型价格快照和渠道倍率计算，并不等同于 OpenAI
账单；未能解析 usage 或没有价格的请求不会计入。主窗口与次窗口通常重叠，两个金额不能相加。

Codex Responses HTTP Connector 只接受 `stream: true` 的 SSE 请求，强制上游
`store: false`，并拒绝非空 `previous_response_id`。当 Responses 能力的**请求压缩**选择
`Zstandard (zstd)` 时，发往 Codex 的最终 JSON body 使用 Zstandard level 3 编码，并设置
`Content-Encoding: zstd` 与 `Content-Type: application/json`；默认 `Default` 不压缩，
WebSocket、standalone search 和 Images 请求也不使用该请求编码。客户端仍可发送
`max_output_tokens`，但选中 Codex managed channel 后，Connector 会在最终上游请求中静默删除
该字段，因为当前 Codex 订阅请求类型不支持它；该值因此不会限制 Codex 输出。这个兼容处理同时
适用于 HTTP SSE 与 WebSocket `response.create`，普通 OpenAI-compatible channel 不受影响。
Codex body/Header 在普通 Transform 之后还会应用独立 provider 白名单：纯遥测、空值和明确
no-op 可以按契约删除；无法表达的非默认语义和未知 body 字段返回 `400`，未知 Header 被删除。
Responses HTTP/WebSocket 会透传已经显式允许的 `x-codex-beta-features`、
`x-codex-routing-hint`、`x-codex-turn-state` 和
`x-openai-internal-codex-responses-lite`；Gateway 不生成这些 beta、路由或 turn-state 值。
此外，Codex OAuth 出站会把 `client_metadata["x-codex-installation-id"]` 和 turn metadata 中的
`installation_id` 替换为按逻辑凭证稳定的 opaque UUID；turn metadata 的 `workspaces` 始终替换
为 Console“系统设置”中的单一合成 Git 工作区。默认 path 为 `/workspace`，默认
`associated_remote_urls.origin` 为 `https://github.com/oai404iao/ai_gateway`；本地路径、真实
Git remote、workspace 数量、commit 和 dirty 状态不会发送给订阅后端。
管理员可在 Console“系统设置”的 Codex 区修改全局 `originator`、`client_version` 和
`User-Agent`。默认值分别为 `codex_cli_rs`、`0.146.0` 和
`codex_cli_rs/0.146.0`；需要精确模拟原生 CLI 时，可把带操作系统、架构与终端后缀的完整
User-Agent 写入该设置。`client_version` 同时用于 `version` Header 和 Models 查询参数，因此
上游提高最小客户端版本时无需重新发布 Gateway。保存会发布新快照；已经在处理的请求以及正在执行的
Models/quota 管理请求保留取得身份时的设置。
Responses HTTP/WebSocket 缺少 `client_metadata` 时，Gateway 会创建并补齐 installation、
session、thread、turn、window、turn metadata 和 `prompt_cache_key`；已有非空身份值保留。
Gateway 不推测 request kind、sandbox、beta、subagent、attestation、turn-state 或 residency。
其他 metadata 和 W3C trace/baggage 保持不变。
Codex HTTP 成功响应即使缺少或错误声明上游 `Content-Type`，Gateway 也会向客户端规范化为
`text/event-stream`；非成功 JSON 错误响应仍保留原内容类型。
Codex managed channel 会自动启用
Responses WebSocket 能力；WebSocket `response.create` 同样强制 `stream: true` 和
`store: false`，但保留 `previous_response_id`、`generate` 与 `client_metadata`，使同一条
上游连接可以使用 Codex 增量状态。首次派发前凭证不可用时，未命中 affinity 或 WebSocket pin
的请求可以换到同组其他凭证；HTTP 请求或 WebSocket 消息一旦发送到 Codex，不做跨凭证自动重试。
已命中 affinity 的凭证处于 `unavailable`、`disabled` 或 Token 过期时，会在 affinity TTL
内持续 fail closed；已经固定到 managed channel 的 WebSocket Session 也不会改用其他订阅账户。

Codex standalone web search 使用独立 Search 能力和同一凭证，公共目标为
`POST /v1/alpha/search`，Connector 将上游目标改为 managed base URL 下的 `/alpha/search`。
该请求固定为非流式 JSON；保留合法的 `x-codex-turn-metadata`，缺失时由 Connector 安全补齐，
并对其应用相同 installation/workspace 归一化；客户端 `originator` 和 `User-Agent` 始终替换为
系统设置中的 Codex Connector 身份。随后注入共享 Bearer、可选 account/FedRAMP 和版本，
并按 Responses 相同规则保留会话身份 Header、仅补全缺失值。发送前不可用且未命中 affinity 时可以重选凭证；命中
affinity 后 fail closed；请求发送后不重试。上游没有返回可识别 usage 时，日志不估算 token
或费用。

Codex Images generation 在模型别名和受限变换后只保留 Codex wire type 声明的 JSON 字段，请求目标改为
`/backend-api/codex/images/generations`。Connector 注入共享凭证的 Bearer、可选
account/FedRAMP、`originator`、版本、User-Agent；`x-codex-image-turn-id` 保留有效调用方值，
缺失或不可用时补全。`session-id`、`thread-id`、`x-client-request-id`、window 和 turn metadata
沿用 Responses 的保留/缺失补全规则，installation/workspaces 仍归一化。Images 不使用 Session affinity；发送前若凭证
不可用可以选择同一操作规则中的其他 Images 能力，但请求一旦发送就不会自动换账户或重试。
成功响应按非流式 JSON 转发并增量提取顶层 usage，不会为 `data[].b64_json` 缓冲完整响应。

Codex Images edit 接收相同客户端模型的 multipart 请求，并在 replayable body 上流式读取图片，
转换为 `/backend-api/codex/images/edits` 的 JSON `images[].image_url` data URL。该 adapter
provider-specific 地限制最多五张输入图片、不接受 mask，并只转发 `prompt`、`background`、
`model`、`n`、`quality` 和 `size`。`moderation=auto` 在客户端入口层作为兼容默认值删除；
`output_format=png` 在 Codex 出口层作为 provider 等价值删除。其他无法等价忽略的取值或字段在
联系上游前拒绝。认证、会话身份和 image turn Header、draining 和发送后不重试边界与 generation 相同。

客户端已有的合法 `session-id` / `thread-id` 会转发。缺少时，Responses/Search HTTP 请求若匹配 Session affinity，
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
3. 管理员配置独立的 `responses-ws` 能力和同操作路由，API Key 已授权所属逻辑渠道；
   HTTP `responses` 能力和路由不能替代 WS 配置。

六操作升级只拆出旧配置已有的 WS 能力及候选。之后的逻辑渠道授权迁移不会创建或启用
新能力/路由，但已有渠道授权覆盖其 WS 能力，不再需要额外能力授权。
新建 Codex 凭证提供独立 Responses HTTP 与 WS 能力，但仍需配置路由及授权；
系统与用户开关仍默认关闭。没有可配置的 `transports` 字段。
Chat Completions 渠道不能声明 WebSocket 支持。系统、用户未开启，或没有可用且声明支持的
Responses WS 路由时，HTTP Upgrade 统一返回 `426 websocket_unavailable`，提示客户端改用
`POST /v1/responses`，且不向客户端暴露系统开关、渠道或 Connector 凭证的内部状态：

- 握手时还没有请求模型。如果该 API Key 的全部已授权 Responses 路由都没有可选 WS 渠道，
  直接拒绝 Upgrade，返回 HTTP 426 和普通 JSON 错误体。
- 如果其他模型仍可使用 WS，允许 Upgrade；收到 `response.create` 后，若该模型没有可选
  WS 渠道，则发送 `type: "error"`、`status: 426`、
  `error.code: "websocket_unavailable"` 的错误帧并关闭连接。请求日志记录同样的状态和错误码。
- 未配置 WS 能力、渠道组/渠道停用、自动禁用，以及被动健康冷却或已被占用的半开探针都会影响
  可选性。明确不可用的 Codex Connector 凭证也不会使对应 projection 通过握手预检。预检不占用
  路由 lease、不推进权重轮转、不消耗 RPM/并发，也不发起上游请求；模型消息仍重新执行正常鉴权、
  准入和路由。
- 未知或无权访问的模型在消息阶段仍返回 `404 model_not_found`；Gateway API Key 鉴权/权限、
  请求校验、准入和进程关闭错误不会被改成 426。HTTP 无可选渠道仍使用
  `503 no_healthy_channel`。
- 上游渠道只能在首条消息后确定；若 Connector、目标配置、网络或上游 Upgrade 在发送
  `response.create` 前不可用，已经完成的下游 `101` 无法撤销，Gateway 改为发送同码的
  `426 websocket_unavailable` 错误帧并关闭连接。

Codex CLI 0.130.0 和 0.154.0 已通过纯本地 Mock 验证：握手 426 触发 HTTP fallback；升级后的 426
错误帧经其 WS 重试预算耗尽后回退，**不保证立即回退**。回退由客户端执行，Gateway 不把已发送的
WS 请求自动转换或重放为 HTTP。其他客户端的自动回退取决于其实现；具体错误映射见
[Codex 参考](../reference/codex-responses-websocket.md)。

管理员也可以在用户详情页修改同一个 `websocket_enabled` 个人偏好。该操作走版本化用户资源，
保存后立即重新发布数据面快照；用户仍可随后在自己的个人设置页再次调整。

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
身份、渠道/上游模型候选、目标 URL、代理/TLS 策略和最终上游请求 Header 精确隔离；不同下游
Session 不共享连接级上下文。

携带非空 `previous_response_id` 的请求必须命中承载该 Session 状态的精确上游连接。连接若已因
空闲/总龄、容量、配置或凭证变化、进程重启或多实例漂移而丢失，Gateway 不会把增量请求发送到新连接，
而是返回以下错误帧；客户端应清除增量状态并重发完整请求：

```json
{
  "type": "error",
  "status": 404,
  "error": {
    "type": "invalid_request_error",
    "code": "previous_response_not_found",
    "message": "Previous response was not found. Retrying the full request."
  }
}
```

上游返回 `websocket_connection_limit_reached`（包括服务端 60 分钟连接上限）或
`previous_response_not_found` 时，Gateway 保留该控制错误帧、废弃对应上游连接，并允许客户端用完整
请求建立新连接。这两类状态恢复错误不会触发渠道自动禁用或清除 Session affinity；Gateway 自身仍不
重放已经发送的请求。

每条 WebSocket 连接同时只允许一个 `response.create` 在途。上游握手完成前的连接类失败仍可按全局
重试设置切换未尝试渠道；消息一旦发往上游，就不再自动重试，以避免重复生成。连接期间客户端 API Key
被撤销或过期后，下一条消息会收到 `invalid_api_key` 错误；系统或用户开关在连接期间关闭后，下一条
消息会收到 `426 websocket_unavailable` 并结束连接。

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
- `GET/PUT/DELETE /console/v1/me/api-keys/{id}`
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
撤销 Key 只会把状态永久改为 `revoked`，记录仍在 Console 中可见。带详情 `ETag` 调用
`DELETE /console/v1/me/api-keys/{id}` 会进一步擦除保存的明文 secret、写入墓碑并从普通列表和
详情隐藏该 Key；请求日志和审计历史继续保留原 Key UUID。

Policy 不再保存额度、RPM、并发、格式、权限或最大活动 Key 数，也不会反向修改既有 Key 的实际限制。
未分配策略、策略已禁用或提交了策略范围外的目标时，接口分别返回
`default_api_key_policy_required`、`default_api_key_policy_disabled` 或
`api_key_target_not_allowed`。

用户组还可以授权只读查看指定渠道组内渠道所引用 Codex 账号的额度窗口。同一凭证仅展示一次，
返回的 `channel_ids` 只含可见组中的引用渠道，不因其他渠道复用该凭证而扩大可见范围。
普通用户接口只返回凭证 UUID（`name` 固定使用同一个 UUID）、Provider 报告的
`plan_type`、当前主/次窗口、凭证级周期花费以及窗口周期历史，不返回管理员 label、邮箱、可选
workspace/member 身份、Token、代理、运行状态、错误或 reset-credit 信息。周期花费汇总该
凭证全部调用者，不按当前用户过滤，但不会暴露用户、API Key、请求或渠道明细。接口没有写方法，
也不提供 refresh、reset、编辑或导出操作；未授权凭证与不存在的凭证统一返回 `404`。

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
- 路由：`/console/v1/routing/groups`、`/accesses`、`/logical-channels`、`/capabilities`、`/profiles`、`/operation-rules`
- 能力批量修改/恢复：`POST /console/v1/routing/capabilities/batch`、`POST /console/v1/routing/capabilities/{id}/recover`
- 网络：`/console/v1/network/proxies`、`POST /console/v1/network/proxies/test`
- 变换模板：`/console/v1/transforms/templates`
- 观测事实：`GET /console/v1/request-logs`、`GET /console/v1/audit-logs`
- 花费排行榜：`GET /console/v1/statistics/spend-leaderboard`
- 系统负载：`GET /console/v1/system/load`（当前实例的 CPU、内存、运行时、队列、日志积压、Responses WebSocket Session/连接池和数据库连接池压力；Console 页面位于“运维”下的 `/admin/system-load`）
- 系统转发设置：`GET` / `PUT /console/v1/system/settings`（管理员；`PUT` 使用 `If-Match`，保存后立即发布快照）
- 手动重载：`POST /console/v1/system/reload`

管理员 Console 的“模型与路由”分为上游接入、上游凭证、渠道配置和模型配置四个入口。
`/admin/routing/channels` 提供组配置、按组列出的逻辑渠道及选中渠道的能力配置。
`/admin/models` 左侧选择客户端计价模型，右侧新增操作或直接编辑已有操作路由；首次保存操作
时会先为该模型创建缺失的 routing profile，再保存操作规则。两次写入不是跨资源原子事务，
规则保存失败时已创建的 profile 保留供重试复用。价格同步位于该页面的独立页签。
原有资源详情 URL 仍可直接访问，更新继续携带详情 `ETag`。
旧渠道组、渠道和模型协议管理页面已移除，旧浏览器地址仅跳转到新列表；旧 HTTP CRUD 返回
JSON 404，不提供旧字段兼容写入。模型发现服务仍使用
`POST /console/v1/routing/channels/models/discover`，能力编辑页从所选接入、凭证及当前变换构造
发现请求，选择结果只修改草稿，保存后才发布。

用户详情支持带 `If-Match` 的 `PATCH /console/v1/users/{id}`，只修改请求中出现的字段；
例如仅提交 `balance_amount` 不会重写邮箱、角色、用户组、策略或状态。用户级
`default_api_key_policy_id` 是可选覆盖；显式设为 `null` 后立即恢复继承用户组默认策略。
管理员还可以只提交 `websocket_enabled`，替用户开启或关闭个人设置中的 Responses WebSocket
偏好，而不会改动其他账户字段。
`invited` 是邀请流程拥有的待激活
状态，管理员修改资料或余额时会保持该状态，只有持有邀请令牌的用户完成激活后才会变为
`active`。兼容用的完整 `PUT` 仍保留，但新客户端应使用 `PATCH`。

`POST /console/v1/users/batch` 一次最多接收 100 个用户及各自的 `updated_at` 版本，可原子地统一
修改运行状态、用户组、用户级 API 策略覆盖和余额。余额支持设置绝对值、统一增加或统一扣减；
任一用户版本过期、状态转换非法或引用不存在时，整批修改与审计全部回滚。包含当前管理员时，
批量操作不能暂停或禁用其自己的账户。

`DELETE /console/v1/users/{id}` 需要 `If-Match` 和 Console 二次确认。删除不会物理移除用户主键：
服务会清空邮箱与密码、匿名化显示名称、撤销全部会话和未接受邀请，并软删除该用户的所有普通
API Key。每个 Key 的明文 secret 会被覆盖，用户与 Key 都从管理列表隐藏；请求日志和审计记录继续
保留原 user ID 和 Key ID。管理员不能删除自己，也不能删除最后一个活跃的非系统管理员。匿名化后
原邮箱可重新使用。

用户组通过 `/console/v1/user-groups` 管理。每个组可设置一个默认 API Key Policy，并通过
`visible_codex_quota_group_ids` 选择成员可只读查看额度的 canonical Codex Responses Channel
Group；普通 OpenAI-compatible group、Codex Images 能力、重复 ID 或不存在的 group 都会被
拒绝。`filter_fast_mode = true` 会在客户端白名单通过后静默删除成员请求中的顶层
`service_tier`：请求继续执行且不会返回错误，上游看不到该字段，请求日志不显示 `Fast`，
`/service_tier` 请求计费倍率也不会命中。该策略适用于 Chat Completions、Responses HTTP/SSE
和 Responses WebSocket，且不会影响其他请求字段。修改后，没有用户级覆盖的组成员立即使用新策略，
Codex 额度可见性和 Fast 过滤也立即按当前用户组生效。
删除自定义组会保留其墓碑并释放组名：普通成员迁移到内置默认用户组，管理员迁移到内置默认管理员
组，关联注册邀请码自动禁用，Codex quota 可见性关系被移除。用户现有 API Key 的目标快照不变，
但继承策略和 Fast 过滤会按迁移后的用户组立即重算。内置默认用户组和默认管理员组始终受保护。

普通资源删除使用详情 ETag，不再提供旧 deletion-impact token 或隐式级联清理。
先显式撤销路由候选，再删除能力、逻辑渠道和管理组；存在依赖时拒绝删除。
固定授权来源与历史身份保留，墓碑资源不能选路；删除后不会把权限转给同名新 UUID。
Codex 托管资源必须使用专属凭证生命周期。凭证或接入被其他逻辑渠道引用时不得连带删除。

计价模型详情页也提供统一的危险操作区。`DELETE /console/v1/models/{id}` 使用 `If-Match`，
先停用并隐藏该模型 routing profile 下的全部操作规则并撤销其候选，再成对清空所有能力 对它的定时测试
计价引用，最后保留停用的模型墓碑。原 `source_model_id` 可由新 UUID 复用；手工创建和
models.dev 导入都不会复活旧墓碑。请求日志继续保存旧模型 UUID、协议规则 UUID、客户端/上游
模型字符串和请求时价格快照，因此删除及同名重建不会改写历史查询或结算。

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

模型路由把结构错误与可用性下降分开处理。跨操作能力、未声明 wire model、重复 priority、
空 tier、同层重复能力/模型组合及非正权重等写入均拒绝并回滚。禁用能力或缩减目录可以使
现有路由不可用；没有可用候选时 HTTP 返回 `503 no_healthy_channel`，Responses WebSocket
遵守其 `426 websocket_unavailable` 边界。恢复后下一次快照发布生效，在途请求保持原快照。

代理编辑页可以在保存前测试当前 HTTP、HTTPS 或 SOCKS 代理草稿。测试接口固定通过该代理请求
ip-api.com，并显示观察到的出口 IP、位置、ISP、自治系统、网络类型和请求耗时；它刻意忽略
`enabled` 与 `no_proxy_hosts`，也不会修改渠道健康或运行时快照。已有代理的隐藏凭据只会在代理
端点未改变时复用；更换 scheme、host 或有效 port 后必须重新输入凭据。免费 ip-api.com 接口使用
HTTP、仅允许非商业用途并带独立限流，因此结果只适合作为人工诊断信息。外部语义和限制见
[ip-api.com 代理出口 IP 查询](../reference/ip-api-proxy-test.md)。

能力的 `billing_multiplier` 为非负十进制数，默认 `1`。最终选定渠道的倍率会乘到模型
输入、缓存输入、缓存写入和输出单价上；请求日志保存乘算后的有效价格快照，因此历史费用
不受后续倍率调整影响。批量修改接口一次最多接收 100 个能力及各自的 `updated_at` 版本，
可统一修改启用状态、自动禁用授权和计费倍率。任一版本过期或候选路由
配置无效时，整批修改、审计和运行时发布都会回滚。

结果为 `failed` 或 `cancelled` 的请求费用固定为 `0`，即使终态前已经观察到部分 usage；
只有成功请求按 token、渠道倍率和高级计费规则计算费用。Migration `0061` 会把已有失败/取消
日志改为零费用，并退回此前已经结算到用户余额和 API Key 已用额度的费用。

能力的 `status_statistics_enabled` 控制是否纳入渠道组状态报告。报告按管理组与 API 格式聚合
已启用统计的能力及其可用模型，同组不同格式分别展示；管理组没有统计开关。

模型的 `advanced_billing.request_multipliers` 会在请求变换前，对原始客户端 JSON
请求体执行 JSON Pointer 精确匹配；所有命中的倍率与渠道倍率相乘，并应用到整次请求费用。
`advanced_billing.time_multipliers` 另外支持最多 32 个每周循环的 UTC 价格时段。每个时段包含
唯一名称、一个或多个开始星期、`HH:MM` 开始/结束时间和非负倍率；省略星期的旧配置默认周一至
周日。开始时间包含、结束时间不包含，开始晚于结束表示跨入次日，此时星期表示窗口开始日。不同星期
可以复用相同钟点，但实际周时间不能重叠。例如只把高峰时段应用到周一至周五，周末便会自然使用
基础谷价。逻辑请求按固定的请求开始时间选择至多一个时段，长时间流式响应或故障转移跨过边界不会
改价；未命中时倍率为 `1`。命中的统一倍率同时乘到输入、缓存输入、缓存写入和输出价格，并与
长上下文档位、请求倍率及最终渠道倍率组合。请求日志仍保存最终有效价格快照，因此后续修改星期、
时段或倍率不会重算历史费用。命中倍率大于 `1` 的时段时，日志还会把该次请求持久标记为高峰价格；
倍率等于或小于 `1` 的基础/折扣时段不会标记为高峰。

`time_multipliers` 是新增的严格 JSON 字段。多实例滚动升级时，应先把所有 Gateway 实例升级到
支持该字段的版本，再由 Console 保存任何 UTC 时段；旧版本会拒绝包含该字段的运行时快照。
一旦已经保存非空时段，不应直接回滚到旧二进制。如确需回滚，应先用新版本清空所有模型的时段配置，
确认新快照已发布，再替换二进制。

模型列表提供直接“配置价格”操作，模型详情页标题区也提供主入口。桌面端“模型价格”子页使用
双栏工作区：左侧维护四类 USD 基础价格、生效时间和星期时段，右侧固定显示精确十进制倍率计算器、
配置摘要及保存状态；窄屏自动改为单栏。管理员可输入四类参考价格与一个倍率，在浏览器中使用
十进制字符串精确计算后一次填入基础价格；计算只修改当前草稿，仍须显式保存。该页面还可维护 UTC
星期时段并套用 DeepSeek 参考预设：把基础价格视为空闲时段价格，在 UTC `01:00–04:00` 与
`06:00–10:00` 填入每天生效的 `2` 倍高峰窗口。预设不是自动价格同步，保存前必须核对实际上游
价格和适用星期。外部规则、差异和核对日期见
[DeepSeek 峰谷定价参考](../reference/deepseek-peak-pricing.md)。

从 models.dev 导入或更新模型时，网关会尽力把
`experimental.modes.*.provider.body.service_tier` 及其统一缩放的输入、缓存和输出价格转换为
`/service_tier` 请求倍率。目录没有该信息、价格不是统一倍率或结构无效时，只跳过该可选规则，
不会排除基础模型；显式价格更新会更新相同匹配条件的目录倍率，同时保留其他本地请求倍率和全部
管理员维护的 UTC 时段倍率。

渠道与变换模板的列表接口只返回摘要字段；渠道详情返回 `credential_id` 与
`override_document`，模板详情返回 `document`。上游密钥改在独立凭证详情中供管理员读取和
轮换，渠道不再写入或返回密钥。详情响应使用 `Cache-Control: no-store`，审计继续排除
上游密钥和变换文档。复用、目标范围、停用及旧 Console 脚本迁移见
[上游凭证管理](upstream-credentials.md)。

`GET /console/v1/system/load` 是只读、管理员权限的当前实例快照。Linux 上从 procfs
采样主机与网关进程 CPU、内存、load average、RSS、文件描述符和线程数；不支持的平台将对应字段
返回 `null`。它还返回进程内准入与路由 in-flight 状态、请求日志通知/投影队列、自动禁用队列、
本地 spool pending bytes、PostgreSQL ingress/settlement backlog 以及控制面和请求日志连接池占用。
请求日志连接池占用在快照触发 backlog 查询前采样，避免监控查询自身的短暂连接占用污染结果。
Responses WebSocket 部分返回全局启用状态、活跃下游 Session、空闲和借出的上游连接、空闲池容量/
占用、命中/未命中/丢弃累计计数以及当前空闲超时和连接最长寿命。
`image_body_spool` 另外返回 multipart edit 的活跃临时文件/字节、文件系统可用容量、累计落盘
body/字节和存储失败次数。容量不足或捕获/回放存储失败必须按数据面硬失败告警；上游派发前发现的
失败返回 `503 image_body_spool_unavailable`。
CPU 百分比依赖相邻采样差值，因此进程启动后的首次采样可能为 `null`。这些数据不是多实例集群聚合；
Console 的“系统负载”页默认每 5 秒重新获取一次。
该页面是请求日志队列、spool、backlog 和数据库池的主要实时视图；完整周期 INFO 心跳默认关闭。
后台仍每 10 秒检查一次日志流水线健康状态，只在 backlog 停滞、健康查询不可用、日志数据库池
持续饱和及其恢复时输出一次状态变化日志。

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
客户端请求的独立计量费用，不等待异步结算回执；系统定时渠道测试不会参与排行榜。

## 自动禁用与定时测试

`/console/v1/system/settings` 的完整配置还包含：

- `websocket.enabled`、`max_idle_connections`、`idle_timeout_seconds` 和
  `max_connection_age_seconds`：Responses WebSocket 总开关和进程级上游空闲池策略。
- `request_retry.enabled`：是否启用发送任何下游响应前的自动故障转移，默认启用。
- `request_retry.max_retries`：首次请求失败后的最大自动重试次数，范围 `1..=10`，默认 `1`。
  同一客户端请求不会重复尝试同一精确渠道/模型候选，但仍可尝试同一渠道上的其他模型。
- `request_retry.retryable_status_codes`：可触发候选故障转移的上游 `400..=599` 状态码，最多
  100 个且不能重复，默认空列表。配置状态重试会丢弃该次响应，可能让上游执行和计费重复；
  Images、Codex Connector 和已发送的 Responses WebSocket 消息不使用该策略。
- `automatic_disable.enabled`：自动禁用总开关。关闭时，即使渠道允许自动禁用也不会执行状态变更。
- `automatic_disable.error_status_codes`：触发临时禁用的上游 HTTP 状态码列表。
- `automatic_disable.error_message_keywords`：触发临时禁用的上游错误消息关键字；匹配大小写不敏感。自动禁用扫描器本身不保存正文；请求日志会为失败的非流式 HTTP、SSE 和 Responses WebSocket 请求保存最长 16KiB、已清理控制字符的文本或结构化错误详情。
- `scheduled_testing.mode`：`global` 测试全部启用渠道；`failure_only` 只测试临时自动禁用的渠道。
- `scheduled_testing.auto_recover`：测试成功后是否自动清除临时禁用。
- `scheduled_testing.interval_minutes`：测试间隔，默认 `5`。
- `scheduled_testing.prompt`：测试 prompt，默认 `reply '1'`。

渠道的 `auto_disable_allowed` 必须为 true 才会被自动禁用；`test_model` 必须从该渠道的
`available_models` 中选择，并且必须同时选择一个现有 `test_pricing_model_id`。前者是实际发送
给上游的 wire model，后者独立提供价格快照，两者不要求同名。定时测试按渠道 API 格式发出非流式 Chat Completions 或 Responses 请求，
并复用该渠道的代理、超时、变换和上游鉴权配置。Images 渠道不能配置 `test_model`，
provider-managed Codex Channel 也不能单独配置这两个字段；这些渠道不会被定时测试。手工禁用的
渠道与禁用渠道组不会被测试。删除所选计价模型会自动清空这两个字段；已删除模型不能再次被选择。

定时测试日志写入 `request_logs`，`request_source` 为 `scheduled_test`。它们使用系统内置、
管理员角色的内部 API Key。网关会解析响应中的 token 用量，并按所选计价模型的不可变价格快照、
模型高级计费规则和渠道计费倍率计算成本；结算会扣减该系统管理员账户余额并累计其内部 API Key
的额度用量，不会归属到任何普通用户。系统内部身份不会出现在用户和 API Key 管理列表中。自动
禁用和自动恢复都会写入系统审计日志并立即发布新的路由快照。
管理员也可以在渠道列表中直接启用、禁用或手工恢复渠道。手工恢复只清除
`auto_disabled` 与其原因，不会改变渠道显式的 `enabled` 值，并使用列表中的
`updated_at` 做并发版本检查。

## Session 粘性

`/console/v1/system/settings` 的 `session_affinity` 可以按请求 Header 或 JSON Pointer
提取 Session Key，并优先复用该 Session 最后一次成功请求所使用的渠道/模型候选。规则按配置顺序执行，
第一个成功提取非空标量值的规则生效。

- 缓存 Key 自动按规则、API Key 和模型规则隔离，原始 Session Key 只用于计算 SHA-256，
  不写入数据库、请求日志或审计详情。
- 缓存有 TTL 和最大条目数，只存在于当前 Gateway 进程。
- 命中的候选仍须满足当前授权、模型能力、模型规则最低可用 priority tier 和被动健康状态，否则
  删除旧映射并执行普通选路。
- 只有完整成功的 2xx 请求才写入或刷新映射；上游失败会删除本次命中的旧映射。
- Session 粘性本身不增加尝试次数；如果全局请求故障转移已启用，失败的精确渠道/模型候选会从
  本次请求中排除，并清除命中的旧映射。
- JSON 来源使用 RFC 6901 Pointer，例如 Responses 请求的 `/prompt_cache_key` 和
  standalone web search 请求的 `/id`。

管理员 Console 会按规则显示当前进程中尚未过期的有效缓存数量，并可清理单条规则或
整个进程的 Session 粘性缓存。对应接口是
`GET/DELETE /console/v1/system/session-affinity/cache`；清理只影响当前进程内存，
不会修改数据库中的规则。

多实例部署没有共享粘性缓存；若同一 Session 被负载均衡到不同 Gateway 进程，各实例会独立学习渠道。

## 日志、用量与结算

每次故障转移会产生 `proxy_request_retry` tracing 事件；每个客户端请求仍只产生一个终态 tracing
事件和一条 `request_logs`，其中渠道、上游模型、结果和计费快照对应最终尝试。worker 从三种格式的普通 JSON
以及 Chat Completions/Responses 的 SSE 事件增量提取 usage，在选路时绑定价格快照，并在可结算时以唯一回执幂等更新用户余额
和 API Key 已用额度。usage 同时保留输入、缓存命中、缓存写入、输出总量，以及输出中包含的
reasoning token。Chat Completions 的 `completion_tokens` 始终作为包含 reasoning 的输出总量保存；
OpenAI 的最终空 `choices` usage chunk 和 DeepSeek 将 usage 附在 `finish_reason` chunk 的形式都可解析。
Console 请求日志的 `Tokens` 列将未缓存输入和输出总量作为主数字，并用紧凑标记分别展示缓存命中
与作为输出子集的 reasoning token。

终态先物化独立计量事实，再分别驱动结算与查询日志投影。日志表暂不可写时费用仍可结算，
所以余额/统计可能先于日志页面可见。API 的 `billed_at` 与筛选保持不变，值来自结算回执；
unknown、价格证据异常和账户归属异常不自动扣款，可通过 `ai_gateway::metering_health`
的状态变化日志核对。无费用的拒绝和当前不可计价的独立搜索不是“零额已结算”。

对于未被用户组 Fast 策略过滤的客户端原始请求，日志还会在不改变其他转发校验的前提下提取显式模式元数据：
OpenAI Responses 的 `reasoning.effort`、DeepSeek/OpenAI Chat Completions 兼容的
`reasoning_effort`，以及表示 OpenAI Priority processing 的
`service_tier = "priority"`。开启用户组 `filter_fast_mode` 后，`service_tier` 会在转发前删除，
因此该请求不显示 `Fast`，也不叠加对应请求倍率。Console 模型列按顺序显示思考等级和绿色
`Fast` 标记；未显式提供对应字段时不显示。耗时列第三行显示输出 TPS，计算为
`output_tokens / ((total_duration_ms - ttft_ms) / 1000)`；usage 或 TTFT 不可用时显示空值。

额度是软预检查：不预留金额，已结算额度达到上限后才拒绝后续请求；余额可以为负。
客户端请求在派发前同步日志 intent 并预留终态空间；不可写或容量不足返回
`503 request_log_unavailable`（WS 为同码错误帧），不会派发上游或伪造取消日志。
终态先同步预分配 slot，再同步追加到 spool；后台通知队列满只合并唤醒。
完整 slot 可在重启后按原 UUID 重放并幂等结算。只有 intent 而无完整终态时，
保留待核对，不自动记零或冻结用户；应监控 `request_log_reconciliation_required`。
日志写/同步失败锁定必须保留原目录重启，单纯容量不足可在排空后自动恢复。
详见[恢复、容量与回滚边界](../development/request-log-durability.md)。

Console 请求日志列表会在同一列上下显示 `api_operation` 和请求协议，并支持按
`api_operation` 筛选。操作区分 Chat Completions、Responses、standalone web search、
Images generation 与 Images edit；协议区分非流式 HTTP、SSE 或 Responses WebSocket。
列表同时显示渠道组名称。
个人“请求日志”
始终只查询当前 JWT 用户；即使当前用户是管理员，
服务端也会将用户名称、具体 `channel_id` 和渠道名称置空。个人列表固定显示开始时间、模型、
操作/请求协议、渠道组、结果、Token、成本和耗时；模型旁可显示思考等级和 `Fast` 标记，耗时同时包含
TTFT、总耗时和 TPS。成本列会为请求开始时命中大于 `1` 的 UTC 时段倍率显示 `Peak` 标记，
该标记随日志持久化，不会因后来修改模型价格而变化。详情再显示 HTTP 状态、错误代码、错误详情和完成时间。错误详情
可能包含上游返回的完整结构化错误对象、文本错误正文前缀或网关/传输诊断，并最多保留 16KiB。

只有管理员“系统”栏下的全局“请求日志”可以读取所有用户的日志。该页面调用管理员接口
`GET /console/v1/request-logs`，并在上述字段基础上额外显示
当前用户名称和渠道名称；过长的渠道组、渠道和用户名称会在列表中省略，悬浮后显示完整值。
Console 列表与详情不显示用户、渠道或请求日志 ID。

## 已知边界

- 支持 Chat Completions、Responses、非流式 JSON Images generation 与非流式 multipart
  Images edit；公开 `/v1/images/edits` 不提供 JSON/data URL edit，也不提供图片流式响应、
  embeddings、audio、files、batches、assistants 或 fine-tuning API。
- 所有余额、额度、模型价格和请求费用统一使用 USD；没有跨实例限流、健康状态或 Session
  粘性协调。Chat Completions 与 Responses 的自动重试覆盖收到响应头前的连接失败、连接超时和
  响应头超时；只有显式列入 `retryable_status_codes` 的 HTTP 错误可在下游响应前重试，SSE
  流中断或流空闲超时不会重试；Images generation/edit 不自动重试。
  系统也没有独立财务账本、充值/退款或货币兑换。
- 服务本身不终止 TLS；Console 必须部署在正确配置的 HTTPS 反向代理后。
