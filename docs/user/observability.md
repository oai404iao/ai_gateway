# 查看日志、负载与用量

> 状态：当前。

用请求日志定位一次调用，用系统负载判断当前实例是否堵塞，用花费报表核对已计价的请求。三者更新节奏不同，日志可见不代表已经结算。

## 先选择视图

| 想查什么 | Console 入口与范围 |
| --- | --- |
| 当前实例 CPU、连接池或 spool 压力 | 管理区 → 系统负载（`/admin/system-load`） |
| 单次调用结果 | 个人请求日志（`/usage/request-logs`）；管理员排查其他用户时用全局请求日志（`/admin/request-logs`） |
| 个人花费、用量和活跃天数 | 个人概览（`/`）或用量报表（`/statistics`）；即使是管理员也只含本人 |
| 按用户或逻辑渠道核对费用 | 管理区 → 全局花费（`/admin/cost-statistics`） |
| 按自然日、周、月比较花费 | 花费排行榜（`/leaderboard`），所有登录用户可见 |

## 观察当前实例

系统负载页每 5 秒读取 `GET /console/v1/system/load`，不是集群汇总。Linux 提供主机与进程 CPU、内存、文件描述符等指标；不支持的字段以及首次采样的 CPU 百分比可为 `null`。

- **请求是否在排队？** 对比准入与路由的在途数、数据库池占用、日志队列、持久 spool 待处理字节和数据库入口/结算积压。默认关闭周期性 INFO 心跳；后台仍报告停滞、查询不可用、日志池持续饱和及恢复的状态变化。
- **WebSocket 是否在复用？** 查看活跃下游会话、借出和空闲的上游连接以及命中、未命中与丢弃次数，结合[连接池容量和时限](websocket.md#配置反向代理与连接池)判断。
- **图片临时目录空间是否足够？** 监控 `image_body_spool` 的可用容量、活跃字节和存储失败次数。派发前的捕获或回放存储故障返回 `503 image_body_spool_unavailable`；它和持久请求日志 spool 是[两类存储](production-configuration.md#区分两类-spool)。

## 查询请求日志

模型筛选精确匹配**用户请求的模型**，不是上游别名；例如 `public-a → provider-b` 应按 `public-a` 查。个人日志显示渠道组，但隐藏用户和具体渠道；全局日志可查看两者。再按操作、传输协议和结果筛选。请求 ID 前缀可过滤当前结果，完整 UUID 可直接打开有权访问的详情。

| 列 | 如何解释 |
| --- | --- |
| 模型 | 思考等级来自请求显式指定的 `reasoning.effort` 或 `reasoning_effort`；`Fast` 表示有效的 `service_tier = "priority"`。用户组过滤掉的 `service_tier` 不显示，也不参与倍率匹配。 |
| Tokens | 主数字是未缓存输入和输出总量，缓存命中与 reasoning 单列。reasoning 属于输出，不要重复相加。 |
| 耗时 | TTFT 是首 Token 耗时；输出 TPS 为 `output_tokens / ((total_duration_ms - ttft_ms) / 1000)`。证据不足时不显示。 |
| 成本 | 使用请求时价格；`Peak` 表示请求开始时命中的 UTC 时段倍率大于 1。未计价不同于已知零费用。 |

详情中的状态码、错误代码和错误详情用于排障；最多保留 16 KiB 的错误详情，可能含上游原始文本片段或结构化错误，分享前请检查。

## 跟踪请求日志与结算

将 Console 请求的 `id` 与进程日志的 `request_id` 对齐。同一 HTTP 请求或 WS `response.create` 可以尝试多个候选，但只形成一条逻辑请求终态。

| 进程事件 | 说明 |
| --- | --- |
| `proxy_request_retry` / `proxy_websocket_retry`（WARN） | 切换候选的原因和阶段。`candidate_attempt` 从 1 开始，准备阶段排除的候选也计数。 |
| `proxy_upstream_dispatch`（DEBUG） | 开始发送 HTTP 请求或 WS 消息。`dispatch_attempts` 不含 WS 握手，也不能证明上游收到了请求或收费。 |
| `proxy_request_completed`（INFO） | 一次逻辑请求的最终结果；网关按最终候选的 usage 和有效价格计费。 |

这些逐次尝试不是持久账单。必要时临时把[日志过滤器](production-configuration.md#日志级别)中的 `ai_gateway::application::proxy` 调为 `debug`。请求终态先形成独立计量事实；结算与日志投影各自推进，余额可能先于日志更新。`billed_at` 来自唯一结算回执。未知结果、价格证据异常或归属异常不会自动扣款，核对 `ai_gateway::metering_health` 的状态日志。拒绝或无法计价的请求也不能当作已按零额结算。

## 处理日志入口故障

派发前不能持久写入 intent 或预留终态空间时，请求返回 `503 request_log_unavailable`（WS 为同码错误帧），不发往上游。容量不足可在排空后恢复；写入或同步失败锁定时，须保留原 spool 目录并重启。

同一 `journal-v2/` 中的完整终态可用原 UUID 重放并幂等结算。发现 `request_log_reconciliation_required` 时，保留只有 intent、没有完整终态的证据待核对；不能据此记零、重试或退款。系统负载的 spool 待处理字节不包括旧布局文件，升级后显示为零不代表旧费用已结清。参见[目录切换](upgrades.md#请求日志目录切换)与[日志恢复边界](../development/request-log-durability.md)。

## 查询花费

个人概览可选 24 小时或 7 天，从 UTC 趋势时段打开对应日志。概览和花费报表按请求模型及 API 格式分组，不因上游别名不同而拆行。用量报表按时间、API Key 和小时/天粒度查看费用；「个人使用情况」显示截至当前 UTC 日期连续 365 天的请求数与活跃天数，不含系统定时测试。

管理员在全局花费叠加用户、Key 或逻辑渠道筛选。选某渠道会计入它的全部能力，不会包含共用凭证的其他渠道。要查凭证窗口花费，请到 [Codex 额度历史](codex.md#刷新-token-与查看额度)。

## 查看花费排行榜

按 `Asia/Shanghai` 的自然日、周（周一开始）或月选择榜单，最多展示 50 名有已计价客户端请求的用户，含 USD 金额、占比、已计价请求数和 Token。后台每 15 分钟刷新快照，不是实时排名。取数基于计量事实，不等待结算，也不包含定时测试。判断榜单新旧时看快照时间，不用个人 UTC 趋势的日期边界推算。
