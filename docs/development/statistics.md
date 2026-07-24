# 统计页面设计

> 状态：当前。接口形状以 `docs/openapi/console-v1.yaml` 为准。

## 目标

Console 提供统计页面：

1. **渠道状态**
   - 所有已登录用户均可查看。
   - 展示纳入状态统计的渠道。
   - 展示上游模型整体和“渠道 × 上游模型”的 TTFT、TPS、成功率。
   - 支持最近 24 小时、3 天、7 天三个窗口。
2. **花费统计**
   - 普通用户只能查看自己的统计；管理员可查看全局或按用户筛选。
   - 支持日期时间区间、用户、API Key 过滤；管理员还可按渠道过滤。
   - 支持按小时、按天聚合。
   - 展示请求数、已计价请求数、总 Token、平均 RPM、平均 TPM、USD 花费、
     时间趋势和模型明细；管理员额外看到渠道明细。

所有统计均从 append-only `request_logs` 聚合，不在数据平面请求路径中同步查询
PostgreSQL。

## 渠道状态开关

`channels.status_statistics_enabled` 是独立的展示开关，默认 `false`：

- `false`：渠道不进入渠道状态统计。
- `true`：渠道及其 `available_models` 进入状态页；即使当前没有请求，也会显示无数据状态。
- 开关不影响路由、被动健康、日志采集或自动禁用。

该字段通过 Channel Console API 读写，并进入审计快照。

## 指标定义

渠道状态统一以 `COALESCE(request_logs.upstream_model, client_model)` 作为模型标识，
并保留 `api_format` 维度，避免混合 Chat Completions 与 Responses。

- **TTFT**：成功请求且 `ttft_ms IS NOT NULL` 的 P90。
- **TPS**：成功请求且 `output_tokens_per_second IS NOT NULL` 的 P50。
- **成功率**：`outcome = 'succeeded'` 的请求数 / 该维度中
  `outcome <> 'cancelled'` 的终态请求数。客户端取消不参与分子或分母；
  若该维度只有客户端取消请求，成功率为 `null`。
- **无数据**：请求数为 0 时，成功率、TTFT、TPS 均为 `null`。

时间窗口和状态条桶宽：

| 窗口 | 桶宽 | 桶数 |
|---|---:|---:|
| 24 小时 | 30 分钟 | 48 |
| 3 天 | 2 小时 | 36 |
| 7 天 | 4 小时 | 42 |

前端状态条颜色仅用于快速阅读：

- 成功率 `>= 98%`：成功色
- 成功率 `>= 90%`：警告色
- 成功率 `< 90%`：失败色
- 无请求：中性色

## 花费统计定义

- **请求数**：过滤区间内全部终态请求。
- **已计价请求数**：`cost_amount IS NOT NULL` 的请求。
- **总 Token**：`input_tokens + output_tokens`；缓存 Token 已包含在输入 Token 内，
  不重复相加。
- **平均 RPM**：请求数 / 过滤区间分钟数。
- **平均 TPM**：总 Token / 过滤区间分钟数。
- **花费**：所有模型价格和请求费用均为 USD，直接对 `cost_amount` 求和，不提供币种选择或换算。
- **模型维度**：与渠道状态相同，使用上游模型优先的模型标识并保留 API 格式。
- **渠道维度**：管理员响应按渠道 ID、请求发生时的渠道组 ID 和 API 格式聚合，
  展示当前渠道/渠道组名称、请求数、成功率、各类 Token 与 USD 花费。
  普通用户响应中的 `channels` 固定为空数组。
- **时间趋势**：响应包含所选区间内连续的 UTC 小时/天桶；没有请求的桶也会返回，
  其请求数、Token 和花费均为零，避免图表只显示有数据的单个时间点。

Console 提供“今天 / 本周 / 本月”快捷范围。“今天”使用小时粒度，“本周”和“本月”
使用天粒度；范围边界按浏览器本地时间生成，再转换为 RFC 3339 传给 API。

聚合桶固定使用 UTC 边界；Console 按浏览器本地时区显示时间。

为限制单次聚合规模：

- 小时粒度最大 31 天。
- 天粒度最大 366 天。

## Console API

Console 端点：

- `GET /console/v1/statistics/channel-status?window=24h|3d|7d`
  - 任意已登录用户可访问。
- `GET /console/v1/statistics/costs`
  - `started_after`
  - `started_before`
  - `granularity=hour|day`
  - `user_id`（可选）
  - `api_key_id`（可选）
  - `channel_id`（可选，仅管理员）
  - 普通用户的查询会强制限定为当前用户；指定其他 `user_id` 返回 `403`。
  - 普通用户指定 `channel_id` 返回 `403`。
  - 管理员可不指定用户查看全局统计，也可按用户、API Key 和渠道筛选。

系统负载仍为管理员专属：`GET /console/v1/system/load`。

响应契约由 `docs/openapi/console-v1.yaml` 定义，前端类型由该规范生成。
