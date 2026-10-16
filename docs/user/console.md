# 使用 Console 与管理 API

> 状态：当前。

Console 面向登录用户和管理员，使用独立监听器上的 JWT，不接受数据面 API Key。浏览器可使用嵌入式管理界面，自动化脚本直接调用 `/console/v1/*`；请求形状和状态码以 [OpenAPI](../openapi/console-v1.yaml) 为准。

## 登录前准备

首次部署先按[快速开始](quickstart.md)生成 Ed25519 密钥并创建管理员。浏览器生产访问必须经过 HTTPS 反向代理；嵌入 UI 需要 `embedded-console-ui` 构建和 `[console].ui_enabled = true`，只在 Console listener 提供。未匹配的 `/console/v1/*` 返回 JSON 404，不会回退为 SPA。

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

### 协助用户恢复密码

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

### 创建或邀请账户

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

### 创建与撤销 API Key

用户的有效 `api_key_policy` 按“用户覆盖优先，否则继承用户组默认策略”解析。Policy 只定义用户
可选择的渠道组和单独渠道。用户通过
`GET /console/v1/me/api-key-options` 获取当前可选列表；创建或更新 API Key 时，从该列表中选择
`allowed_group_ids` / `allowed_channel_ids`，并为该 Key 独立配置 RPM、最大并发和可选额度上限。
API 格式由所选目标自动推导，自助创建 Key 的权限固定为 `proxy` 和 `models.read`。
撤销 Key 只会把状态永久改为 `revoked`，记录仍在 Console 中可见。带详情 `ETag` 调用
`DELETE /console/v1/me/api-keys/{id}` 会进一步擦除保存的明文 secret、写入墓碑并从普通列表和
详情隐藏该 Key；请求日志和审计历史继续保留原 Key UUID。

Policy 不保存额度、RPM、并发、格式、权限或最大活动 Key 数，也不会反向修改既有 Key 的实际限制。
未分配或停用 Policy 时，`GET /console/v1/me/api-key-options` 仍可读取，
但不提供可选渠道组或渠道。自助创建 Key 或修改授权目标时，
未分配策略、策略已禁用或提交了策略范围外的目标分别返回
`default_api_key_policy_required`、`default_api_key_policy_disabled` 或
`api_key_target_not_allowed`。

### 查看 Codex 额度

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
发现请求，选择结果只修改草稿，保存后才发布。普通凭证请求上游 OpenAI 兼容的 `GET /v1/models`；
Codex 托管凭证则用该凭证的 OAuth Token、代理与出站身份实时读取 Codex 后端模型目录，两者都不
持久化任何凭证或渠道改动。

### 修改和删除用户

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

### 管理用户组与默认策略

用户组通过 `/console/v1/user-groups` 管理。每个组可设置一个默认 API Key Policy，并通过
`visible_codex_quota_group_ids` 选择成员可只读查看 Codex 额度的管理组；只展示这些组内逻辑渠道引用的 Codex 凭证。重复或不存在的组 ID 会被拒绝。`filter_fast_mode = true` 会在客户端白名单通过后静默删除成员请求中的顶层
`service_tier`：请求继续执行且不会返回错误，上游看不到该字段，请求日志不显示 `Fast`，
`/service_tier` 请求计费倍率也不会命中。该策略适用于 Chat Completions、Responses HTTP/SSE
和 Responses WebSocket，且不会影响其他请求字段。修改后，没有用户级覆盖的组成员立即使用新策略，
Codex 额度可见性和 Fast 过滤也立即按当前用户组生效。
删除自定义组会保留其墓碑并释放组名：普通成员迁移到内置默认用户组，管理员迁移到内置默认管理员
组，关联注册邀请码自动禁用，Codex quota 可见性关系被移除。用户现有 API Key 的目标快照不变，
但继承策略和 Fast 过滤会按迁移后的用户组立即重算。内置默认用户组和默认管理员组始终受保护。

### 删除资源与保留历史

普通资源删除使用详情 ETag，不使用 deletion-impact token；只有明确说明的依赖清理才会在同一事务执行。
删除能力会在同一事务自动撤销其候选、删除空层并把无候选规则停用为草稿。删除逻辑渠道、管理组、凭证或接入前仍需解除相应依赖，不能假定所有资源都级联删除。
固定授权来源与历史身份保留，墓碑资源不能选路；删除后不会把权限转给同名新 UUID。
Codex 托管资源必须使用专属凭证生命周期。凭证或接入被其他逻辑渠道引用时不得连带删除。

计价模型详情页也提供统一的危险操作区。`DELETE /console/v1/models/{id}` 使用 `If-Match`，
先停用并隐藏该模型 routing profile 下的全部操作规则并撤销其候选，再成对清空所有能力对它的定时测试
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

### 保存、并发与发布

其他大多数可更新资源遵循 `GET` 返回 `ETag`、`PUT` 携带 `If-Match` 的乐观并发模型。控制面
写入在 serializable 事务中再次确认 actor 仍为 active admin，校验完整候选快照、写入脱敏
审计记录，并在提交后立即发布运行时快照。

模型路由把结构错误与可用性下降分开处理。跨操作能力、未声明 wire model、重复 priority、
空 tier、同层重复能力/模型组合及非正权重等写入均拒绝并回滚。禁用能力或缩减目录可以使
现有路由不可用；没有可用候选时 HTTP 返回 `503 no_healthy_channel`，Responses WebSocket
遵守其 `426 websocket_unavailable` 边界。恢复后下一次快照发布生效，在途请求保持原快照。

### 测试代理出口

代理编辑页可以在保存前测试当前 HTTP、HTTPS 或 SOCKS 代理草稿。测试接口固定通过该代理请求
ip-api.com，并显示观察到的出口 IP、位置、ISP、自治系统、网络类型和请求耗时；它刻意忽略
`enabled` 与 `no_proxy_hosts`，也不会修改渠道健康或运行时快照。已有代理的隐藏凭据只会在代理
端点未改变时复用；更换 scheme、host 或有效 port 后必须重新输入凭据。免费 ip-api.com 接口使用
HTTP、仅允许非商业用途并带独立限流，因此结果只适合作为人工诊断信息。外部语义和限制见
[ip-api.com 代理出口 IP 查询](../reference/ip-api-proxy-test.md)。

渠道与变换模板的列表接口只返回摘要字段；逻辑渠道详情返回 `credential_id`，能力详情返回
`override_document`，模板详情返回 `document`。上游密钥在独立凭证详情中供管理员读取和
轮换，渠道本身不写入或返回密钥。详情响应使用 `Cache-Control: no-store`，审计继续排除
上游密钥和变换文档。复用、目标范围、停用及旧 Console 脚本迁移见
[上游凭证管理](upstream-credentials.md)。

### 查看渠道状态报告

能力的 `status_statistics_enabled` 控制是否纳入渠道组状态报告。报告按管理组与 API 格式聚合
已启用统计的能力及其可用模型，同组不同格式分别展示；管理组没有统计开关。

## 接下来做什么

- [配置模型与路由](routing.md)：接入、能力、候选、Key 授权和健康测试。
- [管理上游凭证](upstream-credentials.md)：范围、复用、轮换和删除。
- [计费与额度](billing.md)：价格、倍率和软额度；[日志与监控](observability.md)：查询与统计。
- [安全边界](security.md)：Cookie、凭证和数据库保护；[管理员紧急恢复](security.md#紧急重置管理员密码)。
