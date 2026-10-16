# 配置模型与路由

> 状态：当前。

要让一个客户端模型可用，需要把上游凭证、接入、逻辑渠道、操作能力、候选路由和 Key 授权连起来。创建凭证或启动空数据库都不会自动完成这些步骤。

## 建立第一条路由

1. 在“上游凭证”创建通用静态凭证，并填写准确的允许 Base URL；无需认证的服务可在渠道中明确选择无凭证。Codex 改走[专用接入流程](codex.md)。
2. 在“上游接入”创建 `general` 接入，填写服务的 Base URL、代理和超时。Base URL 必须在凭证允许范围内。
3. 在“渠道配置”创建管理组和逻辑渠道，绑定接入与凭证。
4. 在渠道详情新增操作能力，例如 `chat_completion`，填入能力实际支持的 upstream model；模型发现只填充草稿，必须保存。
5. 在“模型配置”创建客户端计价模型，设置 USD 价格；为该模型添加同操作规则，选择能力、upstream model 和正权重，启用后保存。
6. 向 Gateway API Key 授予所属逻辑渠道的访问范围及 `proxy`；需要模型列表时还要有 `models.read`。普通用户自助 Key 必须受有效 Policy 允许。
7. 使用该 Key 按[快速开始](quickstart.md#5-发送第一个代理请求)发送请求。上游调用可能收费。

## 理解模型、能力和候选

`models.source_model_id` 是客户端模型身份，最多绑定一个 routing profile。每个 profile
按操作分别配置 `chat_completion`、`responses`、`responses-ws`、`web_search`、
`images_edit`、`images_generation` 规则；不会跨格式转换、降级或借用另一操作的路由。
停用规则可以是空草稿，启用规则必须包含非空 tier。

每个 tier 保存 `capability_id + upstream_model + weight` 显式候选。较低 priority
优先；在当前最低可用层内使用 `weighted_random` 或 `weighted_round_robin`。
同一能力可以使用不同 wire model 或出现在不同层；同层完全相同的能力/模型组合不能重复。
保存时 wire model 必须在能力目录中。计费始终使用 profile 的计价模型，转发使用候选模型。
后续移除目录模型、禁用组/接入/凭证/渠道/能力会影响可用性，但不会替换候选。

管理组只组织渠道，不拥有格式或连接器；接入拥有 Base URL、连接器、代理和超时；逻辑渠道绑定管理组、
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
已授权逻辑渠道的现有和后续新增能力均共享授权，不逐能力或按 API 格式限制。
自助 Key 的目标必须属于有效 Policy 的固定渠道范围。
候选配置、能力启用和渠道授权仍是三个独立步骤。
旧管理 API 的 `allowed_api_formats` 作为废弃兼容字段保留，输入可省略且不参与授权；
活跃 Key 返回全部受支持格式。协议验证仍按请求操作执行，不能跨格式转换。

删除普通渠道能力时，后端在同一事务中解绑所有引用它的路由候选，删除空层，并把无候选的
规则停用为草稿；仍有候选的规则保留原开关、权重和策略。受影响规则的 ETag 更新。
版本冲突或审计失败会整体回滚；渠道、计价模型、固定渠道授权及历史计费事实不变。
Codex 使用相同的渠道与能力管理接口；删除能力同样自动解绑候选，不删除凭证或历史金额。

## 管理故障转移、自动禁用与定时测试

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
- `scheduled_testing.mode`：`global` 测试全部已配置探测且启用的合资格能力；`failure_only` 只测试临时自动禁用的渠道。
- `scheduled_testing.auto_recover`：测试成功后是否自动清除临时禁用。
- `scheduled_testing.interval_minutes`：测试间隔，默认 `5`。
- `scheduled_testing.prompt`：测试 prompt，默认 `reply '1'`。

能力的 `auto_disable_allowed` 必须为 true 才会被自动禁用；`test_model` 必须从该能力的
`available_models` 中选择，并且必须同时选择一个现有 `test_pricing_model_id`。前者是实际发送
给上游的 wire model，后者独立提供价格快照，两者不要求同名。定时测试只对支持探测的操作能力发出非流式 Chat Completions 或 Responses 请求，
并复用该渠道的代理、超时、变换和上游鉴权配置。图片能力和 Codex 能力不能配置这两个字段，不会被定时测试。手工禁用的
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

## 保存与验证

控制面写入校验完整快照，提交后在当前进程发布；其他进程通过周期重载收敛。在途请求继续使用取得的旧快照。详情 `ETag` 必须用于更新；版本冲突时重新读取再编辑，不覆盖他人的修改。更多管理接口见[Console](console.md)，金额规则见[计费](billing.md)。
