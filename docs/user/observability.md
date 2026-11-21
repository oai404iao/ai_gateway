# 查看日志、负载与用量

> 状态：当前。

使用 Console 的“系统负载”判断当前进程压力，用请求日志定位调用结果，用花费统计核对已记录费用。三者更新节奏不同：健康检查成功、日志可见和结算完成不是同一件事。

## 先选择视图

| 要回答的问题 | 入口 |
| --- | --- |
| 当前实例 CPU、连接池或 spool 是否接近上限 | 管理区的系统负载（`/admin/system-load`） |
| 我的调用为何失败、花了多少 | 个人请求日志（`/usage/request-logs`）及详情 |
| 其他用户或某个渠道的问题 | 管理区的全局请求日志（`/admin/request-logs`） |
| 时间段内已记录花费 | 个人用量报表（`/statistics`）或管理区的全局花费（`/admin/cost-statistics`） |
| 排名和长期活跃情况 | 花费排行榜（`/leaderboard`）和用量报表中的个人使用情况 |

个人概览（`/`）适合从趋势追到单次调用：选择 24 小时或 7 天，在请求数、Token 或费用之间切换，选中图中的时段后打开对应的个人日志。趋势使用 UTC；未计价与已知的零费用不是一回事。按模型查看的是**相关请求**：报表按上游模型（缺失时使用客户端模型）和 API 格式分组，而日志的模型筛选匹配客户端或上游名称，因此笔数不一定与模型行相等。

## 观察当前实例

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

## 查看个人使用情况

所有已登录用户可在个人用量报表的“个人使用情况”标签查看自己的长期活动。它固定展示截至当前
UTC 日期的连续 365 天客户端请求数，并使用类似 GitHub 贡献图的日期网格显示每日强度；没有请求的
日期也会保留。摘要同时显示总请求数、活跃天数、当前连续活跃天数和最长连续活跃天数。该接口只从
JWT 主体推导用户 ID，管理员在此也只能看到自己的数据；系统定时渠道测试不会计入。

## 查询花费

个人用量报表默认显示花费分析，范围限定为当前 JWT 用户，包括管理员。它只支持时间区间、
API Key 和小时/天聚合粒度，不提供用户或渠道筛选，响应中的渠道明细固定为空。
管理员需要查看全局、指定用户或指定渠道的花费时，使用管理区的全局花费页面；
该页面调用 `GET /console/v1/system/statistics/costs`，可按用户、API Key、单一渠道或 Codex
逻辑凭证筛选并显示渠道明细。Codex 凭证筛选会同时覆盖其 Responses 与 Images managed channels，
并与单一渠道筛选互斥。凭证页的任一主/次窗口周期都可以直接跳转到这里，自动带入周期时间范围和
该 Codex 凭证筛选。

## 查看花费排行榜

所有已登录用户可在 Console 独立的“花费排行榜”页面查看用户花费排名。页面固定提供自然日、
自然周和自然月榜，均按 `Asia/Shanghai` 时区切分：日榜为当日 00:00 至次日 00:00，周榜为周一至周日，
月榜为每月 1 日至月底；并可前后浏览已保留的历史榜单；不提供任意日志时间范围筛选。后台每 15 分钟汇总一次排行榜快照，因此当前
数据不是实时数据，除刷新间隔外还会受到请求日志投影和刷新耗时影响。页面先显示周期总花费和快照时间，
再按名次列出最多 50 名用户的已记录 USD 花费、占周期总额的比例、已计价请求数和 Token。
排行榜仅包含该周期内至少有一个已定价请求的用户；其总花费来自
客户端请求的独立计量费用，不等待异步结算回执；系统定时渠道测试不会参与排行榜。

## 跟踪请求日志与结算

要区分“客户发了几次请求”和“网关尝试了几个上游”，用 Console 请求日志 API 返回的 `id` 匹配进程 tracing 的 `request_id`。一次 HTTP 请求或一条 WebSocket `response.create` 是一个逻辑请求：

- `proxy_request_retry` / `proxy_websocket_retry`（WARN）记录切换前后的候选、切换原因和阶段。`candidate_attempt` 从 1 开始，候选可能在准备阶段就被排除。
- `proxy_upstream_dispatch`（DEBUG）记录每次开始发送 HTTP 请求或 WS 请求消息。`dispatch_attempts` 只统计发送尝试，不把 WS 握手当成发送，也不证明服务商收到了请求或产生了费用。
- `proxy_request_completed`（INFO）是逻辑请求的终态。例如 A 返回可重试的 503、B 成功时，两次发送共用同一 `request_id`。Console 只有一条终态日志，客户费用使用最终候选的 usage 和有效价格，不将 A 当作另一次扣款。

逐次尝试仅保存在按级别启用的进程 tracing 中，不是持久账单。需要查看所有阶段时，临时将[日志过滤器](production-configuration.md#日志级别)中的 `ai_gateway::application::proxy` 调为 `debug`。财务流水线在可结算时以唯一回执幂等更新用户余额和 API Key 已用额度。usage 同时保留输入、缓存命中、缓存写入、输出总量，以及输出中包含的
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

额度是软预检查，完整价格和支出边界见[计费与额度](billing.md)。
## 处理日志入口故障

客户端请求在派发前同步日志 intent 并预留终态空间；不可写或容量不足返回
`503 request_log_unavailable`（WS 为同码错误帧），不会派发上游或伪造取消日志。
终态先同步预分配 slot，再同步追加到 spool；后台通知队列满只合并唤醒。
完整 slot 可在重启后按原 UUID 重放并幂等结算。只有 intent 而无完整终态时，
保留待核对，不自动记零或冻结用户；应监控 `request_log_reconciliation_required`。
日志写/同步失败锁定必须保留原目录重启，单纯容量不足可在排空后自动恢复。
详见[恢复、容量与回滚边界](../development/request-log-durability.md)。

## 查询个人与系统请求日志

Console 请求日志列表会在同一列上下显示 `api_operation` 和请求协议，并支持按
`api_operation` 筛选。操作区分 Chat Completions、Responses、standalone web search、
Images generation 与 Images edit；协议区分非流式 HTTP、SSE 或 Responses WebSocket。
列表同时显示渠道组名称。
个人“请求日志”
始终只查询当前 JWT 用户；即使当前用户是管理员，
服务端也会将用户名称、具体 `channel_id` 和渠道名称置空。个人列表可查看开始时间、模型、
操作/请求协议、渠道组、结果、Token、成本和耗时；模型旁可显示思考等级和 `Fast` 标记，耗时同时包含
TTFT、总耗时和 TPS。成本列会为请求开始时命中大于 `1` 的 UTC 时段倍率显示 `Peak` 标记，
该标记随日志持久化，不会因后来修改模型价格而变化。详情再显示 HTTP 状态、错误代码、错误详情和完成时间。错误详情
可能包含上游返回的完整结构化错误对象、文本错误正文前缀或网关/传输诊断，并最多保留 16KiB。

只有管理区的全局请求日志可以读取所有用户的日志。该页面调用管理员接口
`GET /console/v1/request-logs`，并在上述字段基础上额外显示
当前用户名称和渠道名称；过长的渠道组、渠道和用户名称会在列表中省略，悬浮后显示完整值。

需要与进程日志核对时，在请求 ID 查找框中输入前缀可筛选当前结果，输入完整 UUID 可直接打开本视图有权访问的请求。桌面在列表旁显示详情面板，移动端使用抽屉；面板显示完整请求 ID。复制当前地址可保留已应用的服务端筛选和打开的请求，不会授予收件人额外权限。
