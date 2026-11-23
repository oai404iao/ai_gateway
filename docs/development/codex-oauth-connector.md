# Codex OAuth Connector

> 状态：当前。描述 `connector_kind = codex` 的已实现架构与扩展约束；凭证子类型仍为 `codex_oauth`。

Codex 通过管理员安装并启用的原生插件接入；网关负责凭证生命周期、网络与财务恢复，
插件负责订阅后端协议。安装步骤见[连接器插件使用指南](../user/connector-plugins.md)。

## 本页内容

- [区分客户端格式与上游连接器](#格式与连接器)
- [跟踪一次上游尝试](#运行时边界)
- [管理凭证身份与维护](#持久化模型)
- [判断 quota 与粘性行为](#quota-与-session-粘性)
- [检查各操作的协议和重试限制](#请求与重试边界)
- [保护敏感数据](#安全边界)

## 格式与连接器

客户端协议决定校验、授权和选路，不决定凭证类型。Codex 能力属于 Responses HTTP（SSE）、Responses WS、独立搜索、Images generation 或 Images edit；网关不把其他客户端协议转成 Codex 格式。上游协议计划由已启用的 `codex` 插件生成，插件不可用时不能借用内置 `general` 代替。

## 运行时边界

[`UpstreamConnectorRegistry`](../../src/application/connector.rs) 根据所选渠道准备 attempt。Codex 使用绑定的凭证 UUID，从内存快照取得 Token、可用性及 generation；`PreparedCodexAttempt` 固定插件、设置、凭证和请求上下文。通用候选准备、Header 最后校验和网络派发仍在[宿主转发路径](relay-boundaries.md)。

OAuth、模型发现、额度查询和刷新也固定代际。插件或设置变更会使旧 OAuth state 失效，但不会使结果未知的兑换、刷新变得可以重放。装载、设置迁移与代际发布见[插件生命周期](connector-plugins.md)。

## 持久化模型

`upstream_credentials` 是公共凭证身份，`codex_oauth_credentials` 是同 UUID 的 OAuth 扩展。
扩展表保留历史主键名称 `channel_id`，其含义是凭证 UUID，不再要求等于任一逻辑渠道 UUID。
凭证拥有 Token、quota、维护代理和账号模型目录，没有组/pool 归属。
未绑定任何渠道的凭证也能导入、刷新、查询或删除。

`routing_groups → upstream_channels → credential_id` 是所有连接器共用的组织关系。
逻辑渠道另行引用 `upstream_accesses`，后者决定 Connector、目标和转发代理；
各 `channel_capabilities` 显式声明操作、启用状态、模型目录、变换及健康配置。
同一凭证可由多个兼容渠道复用，但不共享授权、连接池、路由赋值或能力开关。
通用渠道和能力 CRUD 同样管理 Codex，不存在 provider-managed 渠道写入例外。

导入账号只创建或更新凭证，绝不隐式创建接入、渠道、能力、授权或路由。
权重、priority、selection strategy 和上游 wire model 属于客户端计价模型下的操作规则 candidate。
管理员需要显式配置 Images 能力及其操作路由；凭证恢复不打开已关闭的能力。

凭证的持久身份不是单独的 workspace account ID，而是
`(account_id?, user_id)`，并要求 account ID 与 user ID 至少存在一个。
`user_id` 优先来自 `chatgpt_user_id`，兼容同 namespace 的 `user_id` 和 JWT 顶层 `sub`
fallback。这样同一 Business workspace 中的多个成员可以分别成为凭证，而没有 workspace
account ID 的个人凭证按 user ID 独立保存。缺少 user claim 的旧 workspace Token 仍按
account/email 回退匹配，服务启动时会从已保存 Token 尽力补齐旧记录的 user ID。
身份匹配全局进行，不以渠道组为范围；历史重复身份保留，匹配不唯一时拒绝导入，不静默合并金融身份。

删除前必须解除所有活动渠道引用，且不能存在未完成维护操作。
删除在一个事务内关闭凭证、清除三个 OAuth Token、释放维护 proxy、记录墓碑；
不删除接入、渠道、能力或金融历史。凭证与渠道改绑均通过完整快照验证及原子发布。

OAuth PKCE 临时状态单独保存在 `codex_oauth_flows`，按 actor、过期时间和
`completed_at` 限定。数据库只保存 `state` 的 SHA-256；`code_verifier` 在一次性 flow
完成或清理前保存。

## 凭证快照与维护

Access token 不编入完整 `CompiledRuntimeConfig`，而是保存在独立
`CodexCredentialRuntime` / `ArcSwap<HashMap<credential_id, credential>>` 中。
各操作从选中渠道取得实际 credential UUID，数据面每次 attempt 只执行内存读取。

worker 每分钟加载数据库记录并先替换本地凭证快照，使其他实例完成的 enable、quota 或 token
更新最终收敛。需要维护的凭证以有界并发执行：

- access token 在过期前 5 分钟刷新；没有 `exp` 时使用保守 fallback age；
- quota 默认 15 分钟刷新；
- 完成或过期的 OAuth flow 清理；
- 单凭证维护由宿主进程内互斥和 `refresh_generation` 协调；PostgreSQL 的 refresh/reset guard
  保留覆盖外部 HTTP 的数据库行锁；
- SQLite 不在全局写锁下等待 HTTP，改用每凭证互斥、generation/version CAS 和派发前耐久
  intent fencing；这不表示 PostgreSQL 采用同一 intent 机制；
- 事务回滚不能证明上游操作未发生，结果不确定时不能盲目重发 rotating refresh token 或兑换；
- 成功持久化后更新 Token、generation、identity 和错误状态，并发布独立凭证快照。

PostgreSQL 与 SQLite 共享维护操作接口，而非完全相同的故障恢复实现；
详见[维护操作接口](persistence-interfaces.md#codex-外部副作用)和[SQLite Codex 仓储](sqlite-codex.md)。

有效额度观测与 `codex_quota_window_periods` 在同一事务更新。请求发起时间是观测版本，晚返回的旧快照不能覆盖新快照。主次窗口各自最多一个当前周期；在首次使用前，提供方可能不断推迟 `reset_at`，这些零使用率快照更新同一未锚定周期，不记为实际换窗。

窗口内 USD 费用按不可变的请求时凭证归属汇总，不随渠道改绑转移，主次窗口重叠时不能相加。[统计口径](statistics.md#凭证窗口费用)说明时间区间以及零费用和未知窗口的区别。

worker、上游 `401` 恢复和多实例并发均传递 observed generation；如果其他执行者已经成功轮换，
后续执行者直接结束，不能再次消费旧 refresh token。管理员显式手动刷新不带 observed generation，
因此表示一次强制刷新。

永久 refresh 失败设置持久的 `reauth_required`，maintenance 不再自动重复消费该 Token，quota
成功和普通设置更新也不能清除状态。再次 OAuth 或导入相同
workspace/member，或相同 accountless personal user ID 的新 Token
会事务内更新原 credential、递增 generation 并清除 `reauth_required`，不修改渠道和操作配置。

原生凭证导入和导出使用 version 3，包含 Token、身份、启用状态、quota 和可选维护代理。导入只写入凭证；渠道绑定、模型和路由权重由管理员单独配置。

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
PostgreSQL 调用期间持有该凭证数据库行锁，使其他实例的 quota 持久化必须在 reset 事件提交后
再分类换窗；SQLite 使用其单实例 intent/CAS 协议，不在 HTTP 期间持有全局 writer 锁。
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

Standalone web search 可从 Codex 请求 body 的 `/id` 取得与触发 Responses 相同的 Session 值，
规则可同时配置 `/prompt_cache_key` 和 `/id` 作为提取入口，但 affinity cache 还包含操作规则 ID。
Responses 和 Search 使用不同规则，不能据相同 Session 值推断会共享账号绑定；多个凭证时可能
选中不同账号，不保证跨操作连续性。Search 在自身操作规则内命中 affinity 后仍 fail closed，
不因当前凭证不可用而自动换账户。

Images 不使用 Session affinity。Images 请求在发送前遇到 draining/unavailable/disabled 凭证时，
可以排除当前能力候选并按操作路由选择其他凭证；一旦发送则不再换账户。

客户端 `session-id` / `thread-id` 优先保留。缺失时，HTTP 请求若匹配 affinity，则从 session
hash 加 domain separation 派生稳定 opaque UUID；没有 affinity 的 HTTP 请求生成本次请求 UUID。
WebSocket Session 从下游握手身份派生稳定 seed，使同一条下游连接及其可复用上游连接始终使用
一致的 Codex Session/thread identity。

## 请求与重试边界

| 操作 | 宿主执行与约束 |
| --- | --- |
| Responses HTTP | Codex 仅声明 SSE。无支持非流式请求的候选时，选路返回 `503 no_healthy_channel`，不派发。适配后才按能力配置压缩；成功流由宿主作 SSE 处理。 |
| Responses WebSocket | 每条 `response.create` 独立准入，固定凭证、插件代际和上游连接。`previous_response_id` 需要命中干净的原连接；握手失败与消息已发送后的失败分开处理。 |
| 独立搜索 | 独立操作、非流式 JSON，不使用 Responses 的 body 变换或压缩；没有可识别 usage 时不估算费用。 |
| 图片生成与编辑 | 均不使用 affinity。edit 在宿主的有界回放存储上将图片流式编码，插件校验可表达的字段和图片数量；见[Images 实现](openai-images.md#codex-images)。 |

插件在管理员变换后执行请求准备和凭证级隐私归一化；宿主负责最终 Header、目标范围、传输和[原始上游用量](usage-normalization.md)。公共字段以[机器策略](../reference/request-allowlists.json)为准，供应商字段与默认值归插件维护。

派发前未绑定的请求可以排除不可用候选。已绑定会话不改派到其他账号；HTTP 请求或 WS 消息一旦发出，不因 pre-header error、`401` 或连接丢失而重发。`401` 触发按凭证及 observed generation 去重的异步刷新，用于后续请求，不表示本次请求被上游忽略。

## 安全边界

- 普通 Console 凭证视图只返回 Token 元数据。管理员显式调用凭证导出 API 时会得到敏感
  ID/access/refresh token Bundle；它不是普通详情响应，也不能写入日志或浏览器持久化。
- reset-credit 操作只返回结果、窗口数和 correlation ID，不返回 credit 或 OAuth secret。
- credential `Debug`、audit before/after 和错误摘要必须脱敏。
- 删除必须在提交前清除持久化 OAuth Token；tombstone 不得继续引用 proxy 或出现在凭证列表/导出。
- callback URL、authorization code 和导入 token 不进入日志或浏览器持久化状态。
- Codex 客户端原始 installation ID、本地 workspace 路径、Git remote、commit 和 dirty 状态不
  发往订阅后端。安装 ID 按逻辑凭证稳定，workspace 使用插件配置的合成工作区；其他
  `client_metadata`、turn metadata 和 W3C trace/baggage 不在本功能中改写。
- Token 和普通上游 Key 持久化于数据库，保护数据库文件、备份、主机和 Console 管理权限；避免把秘密写入日志或放进测试报告。
- OAuth 外部语义不是网关保证；见
  [Codex OAuth 与订阅后端接入参考](../reference/codex-oauth-connect.md)。

其他连接器的能力声明、命令与装载契约见[原生插件](connector-plugins.md)，宿主与插件的信任边界见[架构决策](../adr/connector-host-boundary.md)。
