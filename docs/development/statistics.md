# Console 统计与聚合口径

> 状态：当前。请求/响应形状以[Console OpenAPI](../openapi/console-v1.yaml)为准。

统计有三种读源：计量事实提供 usage/费用，日志投影提供延迟和成功率，
排行榜读取后台生成的快照。费用可能早于日志页面可见；
“有确定费用”不等于“已经结算”，也不代表提供方账单。
所有聚合都在 Console/后台执行，不进入每个数据面请求路径。

## 目录

- [页面、权限与读源](#页面权限与读源)
- [个人使用情况](#个人使用情况)
- [渠道组状态监控](#渠道组状态监控)
- [花费统计](#花费统计)
- [凭证窗口费用](#凭证窗口费用)
- [花费排行榜](#花费排行榜)
- [接口与验证](#接口与验证)

## 页面、权限与读源

| 视图 | 权限 | 读源与用途 |
| --- | --- | --- |
| 个人使用情况 | 当前 JWT 用户，管理员也只看自己 | request_metering_facts，365 天客户端活动 |
| 渠道组状态 | 任意已登录用户 | request_logs，已纳入统计能力的成功率/TTFT/TPS |
| 个人花费 | 当前 JWT 用户，不接受系统级筛选 | 事实，按时间/Key 聚合；channels 固定空数组 |
| 系统花费 | 管理员 | 事实，可筛 user/Key/channel/Codex credential，含渠道明细 |
| 花费排行榜 | 任意已登录用户 | spend_leaderboard_* 快照，上海自然日/周/月 |

“统计”页默认个人使用情况，花费在独立标签。
系统花费位于 Console 系统分组，管理员在个人标签不获得额外字段/筛选。
系统负载仍是独立管理员 /admin/system-load 页面，不混入个人统计。

## 个人使用情况

端点从 JWT 取 user ID，不接受 user_id。
固定返回截至当前 UTC 日期的 365 个连续自然日，包含首尾日期；
只统计 request_source=client，scheduled_test 不进入贡献图。

每日计数按 started_at 的 UTC 日期归类所有终态，不区分成功、失败、拒绝、取消。
无请求日期补 0，前端始终得到完整日历：

- 总请求数：窗口每日计数之和；
- 活跃天数：计数大于 0 的日期数；
- 当前连续活跃：从当前 UTC 日向前连续非零日期，今天无请求即 0；
- 最长连续活跃：窗口内最长非零日期段。

贡献图以请求数分级，每个日期提供可聚焦提示，不将“缺日期”当作无数据实现。

## 渠道组状态监控

纳入开关属于 **channel_capabilities.status_statistics_enabled**，默认 false，
不是旧 channel_groups 的统一开关。
活动管理组/逻辑渠道中的统计能力提供模型目录与请求范围，
同一组按 (group_id,api_format) 区分，避免混合格式。
模型目录来自纳入能力的 available_models 并集；空闲模型也显示无数据。
开关不影响路由、被动健康、日志采集或自动禁用。

历史 request/channel/group identity 映射回当前 canonical 拓扑，
操作旧名称按协议归一化后匹配能力。
只纳入当前未删除组/渠道/能力且统计开关开启的记录。
不能将旧迁移 0041 的渠道组开关语义作为当前设置入口。

模型标识统一上游优先：COALESCE(upstream_model,client_model)，保留 api_format。
指标定义：

| 指标 | 口径 |
| --- | --- |
| TTFT | succeeded 且 ttft_ms 非 NULL 的 P90 |
| TPS | succeeded 且 output_tokens_per_second 非 NULL 的 P50 |
| 成功率 | succeeded 数 / outcome 非 cancelled 的终态数 |
| 无有效分母 | 只有取消或没有请求时，成功率 NULL |
| 无延迟/吞吐样本 | 对应 TTFT/TPS 为 NULL，不填 0 |

| 窗口 | 桶宽 | 桶数 |
| --- | ---: | ---: |
| 24 小时 | 30 分钟 | 48 |
| 3 天 | 2 小时 | 36 |
| 7 天 | 4 小时 | 42 |

桶使用 UTC。前端状态色：成功率 ≥98% 成功色，≥90% 警告色，低于 90% 失败色，
没有请求为中性色。颜色不是额外健康/选路状态。
SQLite 使用相邻秩与 PG 插值公式实现百分位，见[精确查询](sqlite-metering.md)。

## 花费统计

### 过滤与时间

查询按 started_at 左闭右开区间，granularity=hour/day：
小时最多 31 天，天最多 366 天。
连续 UTC 桶补齐无请求日期/小时，请求数、Token 和花费为 0；
Console 按浏览器本地时区展示。

个人端点仅接受时间、粒度和 api_key_id，不接受 user/channel 等系统筛选；
管理员访问个人端点仍只看本人，channels 为空。
系统端点可按 user_id、api_key_id、channel_id 或 codex_credential_id 限定，
channel_id 与 codex_credential_id 互斥。
费用查询不默认排除 scheduled_test，不能把个人贡献图/排行榜的 client-only 过滤搬过来。

“今天/本周/本月”快捷范围按浏览器本地时间生成，转 RFC 3339 发送；
今天用小时，本周/月用天。浏览器快捷边界与服务端 UTC 桶不是同一概念。

### 指标与维度

| 指标 | 定义 |
| --- | --- |
| 请求数 | 过滤后的全部终态 |
| 已计价请求数 | cost_amount 非 NULL；failed/cancelled 的确定零费用也计入 |
| 总 Token | input_tokens + output_tokens；缓存已包含在输入，不重复相加 |
| 平均 RPM / TPM | 请求数 / 区间分钟，或总 Token / 区间分钟 |
| USD 花费 | 确定 cost_amount 的精确和；无币种选择或换算 |
| 模型 | 上游模型优先，保留 API format |
| 渠道 | 系统视图按请求历史渠道/组身份与 API format 聚合，并返回请求、成功率、各 Token 和费用 |

展示渠道/组名称不重写请求时 UUID。个人响应不返回渠道明细。
费用来源是事实而非日志或回执，不把 amount_state/结算资格过滤偷偷加入统计。
金额必须精确聚合；SQLite 禁止原生 SUM/浮点，最终报告遵守既有 Decimal 结果边界。

## 凭证窗口费用

系统花费与 quota 当前/历史窗口通过 request_credential_identities 使用请求时凭证归属。
新请求不因渠道后来改绑转移费用；旧未知才回退冻结注册表，明确无认证不回退。
不再仅按固定 Responses/Images 两个 managed channel，也不按当前绑定扫描。
凭证窗口汇总不按网关 user/Key 过滤，本人可见性另由用户组授权的渠道组限制。

窗口起点包含、终点不含；历史用实际 ended_at，
当前用 min(now,scheduled_reset_at)。只有 cost 非 NULL 才贡献金额。
主/次窗口重叠，不可相加；这是 Gateway 价格快照 USD，不是 OpenAI 账单，
也不要求已有普通结算回执。

凭证页可跳到系统花费，URL 带周期边界、适合粒度和 codex_credential_id；
未结束窗口到当前时间但不超过计划 reset。统计页从 URL 恢复筛选。

## 花费排行榜

SpendLeaderboardWorker 在 Console 开启时启动，立即刷新，之后每 15 分钟重建。
PG 用事务 advisory lock 避免并发刷新；SQLite 用共享非阻塞 guard/单写事务。
刷新失败保留上一版，取消释放 guard。
API 只读周期/条目快照，不在页面请求时扫描事实。

每次重新计算保留周期，只更新数值/名次变化的条目。
名称/email 不复制进快照，响应 JOIN 当前用户，使匿名化继续生效。

| 规则 | 定义 |
| --- | --- |
| 时区 | Asia/Shanghai；日从 00:00 到次日，周一到下周一，月初到下月初，均左闭右开 |
| period_start / end | 当地日期，结束为排他边界 |
| 请求来源 | 仅 client，scheduled_test 不入榜 |
| 上榜资格 | 所选周期至少一条 cost 非 NULL |
| 用户花费 | 该用户周期全部 cost 之和，不要求结算回执 |
| 请求/Token | 已上榜用户周期全部请求与 input+output，包括未计价请求 |
| 排序 | 费用降序、请求数降序、稳定用户标识；连续名次从 1 开始 |
| total_cost_amount | 全部上榜用户之和，不受 limit 截断 |

省略起点选当前周期，无定价请求也生成零值周期元数据。
历史导航返回前后已保留周期，不接受任意日志区间。
周起点必须周一，月起点必须每月 1 日。
接口 limit=1..100，默认 50，Console 固定前 50，前三名显示领奖台。
首次刷新未完成时 refreshed_at=NULL 且榜单空；
它表示最后成功快照，不是实时账本时间。

## 接口与验证

| 端点（/console/v1 前缀） | 主要参数 / 权限 |
| --- | --- |
| GET /me/usage | 当前用户固定 365 天，无 user 参数 |
| GET /statistics/channel-group-status | window=24h/3d/7d，任意登录用户 |
| GET /statistics/costs | started_after/before、granularity、可选 api_key_id；个人范围 |
| GET /system/statistics/costs | 管理员；再加 user_id/channel_id/codex_credential_id |
| GET /statistics/spend-leaderboard | period=day/week/month（默认 day）、可选 period_start、limit |
| GET /system/load | 管理员，实时单实例指标，不是统计历史 |

无效时间/粒度/周期/limit、个人端点系统筛选或互斥筛选按 OpenAPI 返回 422。
响应类型由 OpenAPI 生成，不能在前端手改。

维护验证重点：

- 本人/管理员隔离，个人接口拒绝系统参数且 channels 为空；
- source、NULL/零费用、已结算与统计金额不同口径；
- UTC 日/小时和上海周/月边界，空桶、闰日/微秒及当前连续活跃；
- 能力统计开关、同组多格式、历史身份、取消分母和空闲模型；
- 凭证改绑前后归属、旧未知/明确无认证、quota 重叠窗口；
- 精确聚合、排行榜并发/取消/保留旧快照、全量总额与 limit；
- 双后端相同 case、Console spec/generated types 和对应组件/浏览器。

实现入口为 postgres_control_plane.rs 的查询/聚合、sqlite/{queries,aggregate,leaderboard}.rs、
workers/spend_leaderboard.rs、http/console.rs。
费用来源约束见[独立计量](independent-metering.md)，前端变更遵循[Console UI](console-ui.md)。
