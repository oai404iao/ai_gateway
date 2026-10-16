# Chat Completions 参考

> 类型：外部参考与网关兼容边界。
>
> 最近核对：2026-08-05。以下外部语义沿用该次核对，不代表第三方最新状态。
>
> 权威来源：[OpenAI Create chat completion](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)、
> [DeepSeek Create Chat Completion](https://api-docs.deepseek.com/api/create-chat-completion)、
> [DeepSeek Thinking Mode](https://api-docs.deepseek.com/guides/thinking_mode)、
> [DeepSeek Context Caching](https://api-docs.deepseek.com/guides/kv_cache)、
> [阿里云百炼深度思考](https://help.aliyun.com/zh/model-studio/deep-thinking)。

使用 `messages` 格式的客户端调用此入口。管理员需为客户端模型配置 `chat_completion`
操作路由，候选逻辑渠道必须具备对应能力；仅配置 Responses 不会自动兼容。

## 最小请求

```bash
curl "$GATEWAY_BASE_URL/v1/chat/completions" \
  -H "Authorization: Bearer $GATEWAY_API_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"model":"your-client-model","messages":[{"role":"user","content":"Reply with OK."}]}'
```

`GATEWAY_BASE_URL` 是公共 listener 地址，不含 `/v1`；模型名是管理员配置的客户端模型名，
不是任意上游 wire model。需要 SSE 时增加 `"stream":true`；上游支持时可再增加
`"stream_options":{"include_usage":true}`。请求会产生正常上游调用和计费。

## 外部接口关键语义

| 方面 | 核对时的外部语义 |
| --- | --- |
| 入口 | `POST /v1/chat/completions`，核心字段为 `model`、`messages` |
| JSON | 返回 Chat Completion 对象；工具、角色、结构化输出取决于模型 |
| SSE | 返回 chunk，OpenAI 风格通常以 `data: [DONE]` 结束 |
| OpenAI usage | `prompt_tokens` 为输入总量，`completion_tokens` 为包含 reasoning 的输出总量 |
| OpenAI 细分 | 缓存命中为 `prompt_tokens_details.cached_tokens`；reasoning 为 `completion_tokens_details.reasoning_tokens` |
| DeepSeek 扩展 | 可返回 `prompt_cache_hit_tokens`、`prompt_cache_miss_tokens` 及 reasoning 细分 |
| 流式 usage | OpenAI 可在 `[DONE]` 前给出 `choices: []` 汇总；DeepSeek 也可能附在最终 `finish_reason` chunk |

`thinking`、`enable_thinking` 等扩展不是跨上游统一开关；字段结构、可用值和模型支持应查
对应供应商文档。

## ai-gateway 行为

**请求处理**：`open_ai_chat_completions` 格式与其他格式隔离。
顶层字段按 `chat_completions.client_body` 检查，未知字段本地返回 `400`；
`messages`、tools 等允许字段内部不递归校验。`thinking`、`enable_thinking` 已列入入口
白名单，但网关不解释值，也不替目标上游实现思考模式。

只有在策略未删字段、Fast 过滤未改 body、无模型别名且无请求变换/适配时，原始 JSON
字节才保持不变。模型别名只改顶层 `model`，不会改嵌套同名字段。

**响应处理**：默认不重写上游状态和业务 body；HTTP 压缩由网关独立协商。
无 SSE 变换时转发解码后的事件字节；有变换时按完整 frame 应用 Chat 专属规则。
显式连接器响应适配属于额外配置，不是默认跨协议桥接。

**usage 与终态**：

- 输入、输出分别记录 `prompt_tokens`、`completion_tokens`；reasoning 作为输出子集保留，不重复扣减。
- 缓存命中优先读 `prompt_cache_hit_tokens`，否则读 `prompt_tokens_details.cached_tokens`；
  未缓存输入由输入总量减命中量得到，不单独存 `prompt_cache_miss_tokens`。
- 支持上述两类最终 usage chunk；两者都出现时采用后到达的最终汇总。
- `[DONE]` 是日志识别成功结束的信号之一，不要求靠底层 EOF 才判断终态。

供应商 usage 归一化与插件职责详见[usage 规范化](../development/usage-normalization.md)。

## 接入检查

| 症状或任务 | 检查 |
| --- | --- |
| 请求到错地址 | base URL 与公共路径拼接，是否重复 `/v1` |
| 本地 `400` | [白名单](request-allowlists.md)，不要先假设上游拒绝 |
| 上游鉴权失败 | Bearer / 自定义 Header 与凭证绑定 |
| 工具或 reasoning 不工作 | 目标上游和 wire model 是否真正支持 |
| SSE 卡住或 usage 缺失 | `text/event-stream`、空行分帧、汇总 chunk 与终止信号 |
| 同模型只在 Responses 可用 | 独立操作能力、模型目录和 `chat_completion` 路由 |

转发实现变更的检查要求见[真实上游 smoke](../development/real-upstream-smoke.md)；
本页示例不是自动授权执行付费测试。整体限制见[兼容性总览](openai-compatibility.md)。
