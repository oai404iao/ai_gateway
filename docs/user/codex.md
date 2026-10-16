# 接入 Codex 账号

> 状态：当前。

Codex 通过独立安装的可信原生插件接入 ChatGPT 订阅。先[安装并启用 Codex 插件](connector-plugins.md)，再导入账号、配置能力和授权；仅有订阅凭证不能代理请求。

## 连接账号

管理员可把 ChatGPT Codex 订阅作为 Connector 接入，无需 sidecar。每个凭证保留稳定
credential UUID，可由多个兼容的逻辑渠道引用，没有组或池归属。渠道可显式配置 Responses HTTP、
Responses WS、独立 Search、Images generation、Images edit 能力。Token、quota 和维护代理属于凭证，
转发代理属于接入，健康、目录和路由按能力分离，访问授权属于逻辑渠道。

按以下顺序配置：

1. 打开 **上游凭证 → Codex**：`/admin/routing/upstream-credentials?connector=codex`。
2. 使用 **Connect account** 的 PKCE 流程，或 **Import tokens** / **Advanced import**。
   PKCE 回调地址仍为 `http://localhost:1455/auth/callback`，复制完整地址回 Console 完成交换。
   高级导入支持原生 Bundle、CPA 和 Sub2API；草稿与 Token 只留在当前页面内存。
   workspace/member 或无 workspace 的个人 user ID 用于原位重授权，不按 Token 文本去重。
3. 配置 Codex 上游接入、渠道组及引用该凭证的逻辑渠道，再在渠道详情添加操作能力和模型目录。
   导入不会自动创建渠道或能力；旧配置迁移保留 Images 的停用状态，重导入不覆盖渠道的独立目录。
   HTTP Responses 的 zstd 压缩只在 Responses 能力配置，不影响 Search、Images 或 WebSocket。
4. 建立计价模型/profile，并为需要的每个操作分别保存能力/模型候选。Search 不隐式借用
   Responses 规则；Images generation/edit 也各有规则。新增凭证不会自动加入候选。
5. 显式给 API Key/Policy 添加所需组或逻辑渠道；Key 需要 `proxy` 权限。
   新导入渠道不会自动加入已有组授权；已授权渠道的全部能力无需逐项重新授权。
6. 客户端 Codex provider 的 base URL 指向 Gateway `/v1`；独立搜索仍需客户端
   `supports_standalone_web_search = true`。需要操作内会话粘性时配置 Session affinity；缓存按模型规则隔离，
   Responses 与 Search 使用独立规则，相同 Session Key 不保证两者选中同一账号。跨操作使用同一账号
   必须在各操作路由中明确选择绑定同一凭证的渠道能力，不能仅依赖 affinity。

Codex 认证生命周期通过 `/console/v1/routing/upstream-credentials/codex` 及其子路径管理。
逻辑渠道与能力使用统一 CRUD；普通静态密钥接口不能替代 OAuth 认证流程。
删除凭证前必须解除全部未删除渠道的引用，包括停用渠道；删除只清除 Token 和维护代理，
不级联删除接入、渠道、能力或金融事实。维护代理修改不改写渠道接入的转发代理。

## 识别凭证状态

凭证状态含义：

- `active`：可接收新的 Responses Session、standalone web search 或 Images generation/edit；
- `draining`：primary/secondary quota 用量达到 threshold；只允许已命中 affinity 的既有
  Responses Session，Images 能力在发送前被排除；
- `unavailable`：quota 不允许、额度耗尽或 refresh token 永久失效；
- `disabled`：管理员关闭。

永久 refresh 失败会设置持久的重新授权状态；后续 quota 成功或普通设置编辑不会把该凭证重新置为
`active`。重新执行 OAuth 或导入同一 workspace/member 身份，或同一无 workspace 个人 user ID
的新 Token，会复用原凭证并清除该状态，不更改已绑定渠道和能力。

## 导入、导出与删除

凭证列表支持多选后批量启用、停用、删除和导出选中项。单条和批量删除都使用乐观并发版本：
删除前必须解除全部未删除逻辑渠道的引用。成功后凭证从列表消失，保存的 ID/access/refresh token 被清除，维护代理引用被释放；保留不含敏感秘密的身份墓碑和历史请求引用，不级联删除其他共享资源。
该删除不会调用
OpenAI token revocation endpoint，若还需要使外部 Token 失效，应在账户侧另行撤销授权。

凭证页的 **Export credentials** 需要显式确认，并下载 ai-gateway 原生 JSON Bundle。当前 Bundle
为 version 3，包含原始 ID/access/refresh token、可选 workspace/member 身份和 enable/quota
设置，并可选包含被这些凭证引用的代理定义及代理认证信息；不包含组/渠道归属或 routing weight。它必须按密钥或
未加密备份处理。常规凭证列表和详情接口仍不会返回已保存 Token，只有管理员显式调用导出接口时
才会读取这些敏感字段。高级导入会保留 Bundle 中的 enable 状态；如果 `id_token` 缺失，则验证
阶段从 `access_token` 读取身份声明。旧原生或外部 JSON 中若带 `weight`，前端会忽略并显示
warning。新凭证不会自动加入现有模型规则；管理员必须在协议路由编辑器中逐条添加其
能力候选。

高级导入页允许删除代理，但服务端要求 `If-Match`，并且只有当代理未被上游接入、Codex 凭证的
维护配置或仍保留的 OAuth 流引用时才会删除；否则返回 `proxy_in_use`。已分配给导入草稿的代理还必须存在且已启用。

## 刷新 Token 与查看额度

后台 worker 每分钟检查凭证；access token 接近过期时刷新，quota 默认每 15 分钟重新读取。手动
refresh token / quota 也可从凭证页执行。同一凭证的 refresh 使用实例内互斥与 generation 校验，
避免 rotating refresh token 被并发重复使用。PostgreSQL 使用行锁事务；SQLite 使用数据库 owner
独占、单写事务与持久 intent，不跨上游 HTTP 持有写事务，未知结果不会自动重试，恢复边界见
[SQLite 指南](sqlite.md#升级与未知结果)。上游请求返回 `401` 时会触发一次 generation 去重的后台刷新。

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

## 调用 Responses

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
为 Console“插件管理”中配置的单一合成 Git 工作区。其 `path`、
`associated_remote_urls.origin` 等值以所安装插件版本的设置 schema 和已保存配置为准，不是
Gateway 固定默认值；修改方法见[插件设置](connector-plugins.md#3-调整插件设置)。本地路径、真实
Git remote、workspace 数量、commit 和 dirty 状态不会发送给订阅后端。
管理员可在 Console“插件管理”的 Codex 设置中修改 `originator`、`client_version` 和
`User-Agent`。默认值和校验以所安装插件提供的设置 schema 为准；需要精确模拟原生 CLI 时，可把带操作系统、架构与终端后缀的完整
User-Agent 写入该设置。`client_version` 同时用于 `version` Header 和 Models 查询参数，因此
上游提高最小客户端版本时无需重新发布 Gateway。保存会发布新快照；已经在处理的请求以及正在执行的
Models/quota 管理请求保留取得身份时的设置。
Responses HTTP/WebSocket 缺少 `client_metadata` 时，Gateway 会创建并补齐 installation、
session、thread、turn、window、turn metadata 和 `prompt_cache_key`；已有非空身份值保留。
Gateway 不推测 request kind、sandbox、beta、subagent、attestation、turn-state 或 residency。
其他 metadata 和 W3C trace/baggage 保持不变。
Codex HTTP 成功响应即使缺少或错误声明上游 `Content-Type`，Gateway 也会向客户端规范化为
`text/event-stream`；非成功 JSON 错误响应仍保留原内容类型。
Responses WebSocket 需要独立能力、路由以及系统与用户开关；WebSocket `response.create` 同样强制 `stream: true` 和
`store: false`，但保留 `previous_response_id`、`generate` 与 `client_metadata`，使同一条
上游连接可以使用 Codex 增量状态。首次派发前凭证不可用时，未命中 affinity 或 WebSocket pin
的请求可以换到同一操作规则内其他合格候选；HTTP 请求或 WebSocket 消息一旦发送到 Codex，不做跨凭证自动重试。
已命中 affinity 的凭证处于 `unavailable`、`disabled` 或 Token 过期时，会在 affinity TTL
内持续 fail closed；已经固定到 managed channel 的 WebSocket Session 也不会改用其他订阅账户。

## 调用独立搜索

Codex standalone web search 使用独立 Search 能力和同一凭证，公共目标为
`POST /v1/alpha/search`，Connector 将上游目标改为 managed base URL 下的 `/alpha/search`。
该请求固定为非流式 JSON；保留合法的 `x-codex-turn-metadata`，缺失时由 Connector 安全补齐，
并对其应用相同 installation/workspace 归一化；客户端 `originator` 和 `User-Agent` 始终替换为
插件设置中的 Codex Connector 身份。随后注入共享 Bearer、可选 account/FedRAMP 和版本，
并按 Responses 相同规则保留会话身份 Header、仅补全缺失值。发送前不可用且未命中 affinity 时可以重选凭证；命中
affinity 后 fail closed；请求发送后不重试。上游没有返回可识别 usage 时，日志不估算 token
或费用。

## 生成和编辑图片

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

## 会话身份与秘密保护

客户端已有的合法 `session-id` / `thread-id` 会转发。缺少时，Responses/Search HTTP 请求若匹配 Session affinity，
会从不可逆 session hash 派生稳定 opaque UUID；未匹配 affinity 的 HTTP 请求仅使用本次请求
UUID。WebSocket Session 从下游握手身份派生稳定 seed，使顺序请求和池化重连使用一致身份。

OAuth token 不会进入 audit/debug 输出；除显式管理员导出接口外，也不会由常规 Console API 返回。
与普通 upstream API key 一样依赖受保护的数据库、备份和主机访问边界，未额外实施列级
静态加密。外部接口与限制见
[Codex OAuth 与订阅后端接入参考](../reference/codex-oauth-connect.md)和
[Codex 凭证导入格式兼容性](../reference/codex-credential-portability.md)。
