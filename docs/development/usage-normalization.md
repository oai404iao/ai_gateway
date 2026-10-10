# 上游 usage 规范化

> 状态：当前。按连接器代际和实际上游接口选择解析器；不改变历史事实、存储字段或计费公式。

## 统一计数

请求日志、计量事实与结算继续使用 `RequestUsage` 的五个字段：

| 字段 | 规范含义 |
| --- | --- |
| `input_tokens` | 完整输入总量，包含缓存读取与缓存写入部分 |
| `cached_input_tokens` | 输入总量中命中缓存的部分 |
| `cache_write_tokens` | 输入总量中写入缓存的部分 |
| `output_tokens` | 完整输出总量，包含 reasoning 部分 |
| `reasoning_tokens` | 输出总量中的 reasoning 部分 |

计数必须是非负 `i64`，缓存子项不能超过输入总量，reasoning 不能超过输出总量。
usage 或必要总量缺失、已知字段损坏或溢出时为 unknown，不能伪造全零或估算计数。
通用 OpenAI 解析对缺失的可选缓存/reasoning 明细保留既有零值缺省。
本轮没有迁移、重算或重新结算已有事实。

## 连接器与实际上游接口

公共客户端 `ApiFormat` 决定入口策略和路由格式，不一定决定 usage 字段语义。
例如 Codex Images 内部消费 Responses 的 usage，因此声明的是
`open_ai_responses`，而不是由客户端 Images 格式推断。

协议 3 的 `attempt.describe/v1` 可声明：

```json
{"usage":{"parser":"general","format":"open_ai_responses"}}
```

通用解析器支持 `open_ai_chat_completions`、`open_ai_responses`、
`open_ai_images` 和 `anthropic_messages`。连接器复用它时无需实现解析命令。
选择是显式的，不按 URL、供应商域名或 `cached > input` 猜测语义。
未声明该字段的协议 1/2/3 插件保持原有 OpenAI 兼容默认。

需要专有字段解析时，插件可声明：

```json
{"usage":{"parser":"plugin","interface":"provider.interface/v1"}}
```

宿主验证插件声明了 `usage.parse/v1`，并传入至多 64 KiB 的原始 usage JSON
对象，而不是完整响应、图片、对话内容、凭证或价格。插件返回五项规范计数或
`usage:null`；输出 body 必须为空，不允许附带金额、结算或其他字段。
错误、无效计数和超限输出成为 unknown，不能回退另一种解析器。
详细类型与命令见 [SDK 契约](../../crates/connector-sdk/docs/commands.md)。

## 包含与拆分

OpenAI 通用解析保留输入总量，不再加一次缓存命中数。
DeepSeek 的 `prompt_cache_hit_tokens` 优先于标准嵌套缓存明细；
`prompt_cache_miss_tokens` 不额外累加到 `prompt_tokens`。
Anthropic 通用解析则使用 checked arithmetic：

```text
规范 input_tokens = 原始 input_tokens
                  + cache_read_input_tokens
                  + cache_creation_input_tokens
```

例如 OpenAI `input=5, cached=3` 与 Anthropic `input=2, cache_read=3`
都得到规范 `input=5, cached=3`，不会遗漏或重复计算缓存部分。
上述供应商语义于 2026-10-10 核对：
[OpenAI 缓存](https://developers.openai.com/api/docs/guides/prompt-caching)、
[DeepSeek usage](https://api-docs.deepseek.com/api/create-chat-completion/)、
[Anthropic 缓存](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)。

Anthropic SSE 的初始输入/缓存计数与后续累计输出计数，由宿主按请求合并；
不是把每个 `message_delta` 的输出计数相加。
完整 usage 被拒绝后会丢弃旧的部分快照，不能用后续输出更新恢复过时输入。
该累计语义也于上述日期核对
[Anthropic streaming](https://platform.claude.com/docs/en/build-with-claude/streaming)。
自定义解析命令接收未合并的原始 usage 对象，不承诺任意供应商的增量合并规则。

## 宿主边界

HTTP JSON/SSE、Responses WebSocket 与计划探测都固定所选代际和 usage 配置。
响应模式切换保留解析器，但重置请求级状态；重试或替换路由后重新选择解析器。
普通 JSON 增量扫描 usage，不为图片或其他大响应缓冲整个 body。
无效终态 usage 不能沿用之前的有效计数；真实终态之后不再接收新的计量摘要。

规范化不修改下游响应，也不调用响应适配命令。响应适配输出仍不参与计量；
usage 解析只解释原始上游计数，不能决定路由、响应成败、价格或收费政策。
当前既有计费公式仍为：

```text
((input - cached) × input_price
 + cached × cached_price
 + cache_write × cache_write_price
 + output × output_price) / price_unit_tokens
```

缓存写入价格仍是既有额外项，没有改成扣除写入后的另一套规则。
失败/取消请求继续零收费；成功 unknown usage 依现有结算策略处理，
Codex 拼车保持 pending 和 fail closed，不清除不确定财务状态。

新 Codex 插件使用协议 3，所有响应模式为 `passthrough`，usage 复用
`general/open_ai_responses`。HTTP Responses 只声明 SSE；无其他可用普通渠道时，
non-stream 请求在派发前返回 `503 no_healthy_channel`，不破坏已建立的 SSE affinity。
旧协议 2 的请求验证路径保持兼容。

本轮没有增加 Anthropic 数据面路由、请求转换或跨格式桥接。
系统 E2E 使用 OpenAI 兼容响应包络模拟不同 usage 方言，不宣称原生 Anthropic 转发。

## 验证

- SDK：包含/拆分格式、数值边界、兼容默认、专有命令协商。
- 宿主：JSON/SSE/WS 原始 usage 隔离、unknown、不回退、累计状态与终态优先级。
- [系统 E2E](system-e2e.md)：双后端验证相同规范计数、原始响应逐字节透传、
  独立 API Key/余额基线和一次持久结算。
- [插件职责](connector-plugins.md)、[响应适配](connector-response-adapters.md)
  与 [独立计量](independent-metering.md) 保持不同边界。
